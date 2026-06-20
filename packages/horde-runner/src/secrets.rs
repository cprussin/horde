//! Read secret token files and turn them into the environment variables
//! injected into the sandbox, mirroring the secrets block of `horde-run.sh`.
//!
//! Secrets are returned as a name→value list and placed in the launched
//! process's environment (never on bwrap's argv), so they are not visible in
//! `/proc/<pid>/cmdline`.

use std::fs;

use anyhow::{anyhow, bail, Result};

use crate::config::Config;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Secrets {
    /// Environment variable name → value, to inject into the sandbox process.
    pub vars: Vec<(String, String)>,
    /// (owner, env-var-name) for per-owner GitHub PATs.
    pub gh_owners: Vec<(String, String)>,
    pub have_gh_default: bool,
    pub have_gh_app: bool,
}

pub fn collect(config: &Config) -> Result<Secrets> {
    let mut s = Secrets::default();

    if let Some(file) = &config.claude_token_file {
        let token =
            read_token(file).map_err(|_| anyhow!("cannot read claude token file: {file}"))?;
        if token.starts_with("sk-ant-oat") {
            s.vars.push(("CLAUDE_CODE_OAUTH_TOKEN".into(), token));
        } else {
            s.vars.push(("ANTHROPIC_API_KEY".into(), token));
        }
    }

    if let Some(json) = &config.github_token_files {
        for (owner, file) in parse_obj(json, "HORDE_GITHUB_TOKEN_FILES")? {
            let token =
                read_token(&file).map_err(|_| anyhow!("cannot read github token file: {file}"))?;
            if owner == "default" {
                s.vars.push(("GH_TOKEN".into(), token.clone()));
                s.vars.push(("GITHUB_TOKEN".into(), token));
                s.have_gh_default = true;
            } else {
                if owner.is_empty()
                    || owner.starts_with('-')
                    || !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    bail!("invalid github owner in HORDE_GITHUB_TOKEN_FILES: {owner}");
                }
                // GitHub logins contain no underscores, so hyphen→underscore
                // yields a unique, valid environment-variable name.
                let var = format!("HORDE_GH_TOKEN_{}", owner.replace('-', "_"));
                s.vars.push((var.clone(), token));
                s.gh_owners.push((owner, var));
            }
        }
    }

    if let (Some(id), Some(keyfile)) = (&config.gh_app_id, &config.gh_app_key_file) {
        let key = read_token(keyfile)
            .map_err(|_| anyhow!("cannot read github app key file: {keyfile}"))?;
        s.vars.push(("HORDE_GH_APP_ID".into(), id.clone()));
        s.vars.push(("HORDE_GH_APP_KEY".into(), key));
        s.have_gh_app = true;
    }

    if let Some(json) = &config.token_files {
        for (var, file) in parse_obj(json, "HORDE_TOKEN_FILES")? {
            let first_ok = var
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic() || c == '_')
                .unwrap_or(false);
            if !first_ok || !var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                bail!("invalid variable name in HORDE_TOKEN_FILES: {var}");
            }
            let value = read_token(&file).map_err(|_| anyhow!("cannot read token file: {file}"))?;
            s.vars.push((var, value));
        }
    }

    Ok(s)
}

/// Read a token file, stripping trailing newlines to match bash `$(cat file)`
/// (interior newlines, e.g. in a PEM key, are preserved).
fn read_token(path: &str) -> Result<String> {
    let raw = fs::read_to_string(path)?;
    Ok(raw.trim_end_matches('\n').to_string())
}

/// Parse a JSON object of string→string, like `jq -r 'to_entries[]'`.
fn parse_obj(json: &str, ctx: &str) -> Result<Vec<(String, String)>> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| anyhow!("invalid JSON in {ctx}: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("{ctx} must be a JSON object"))?;
    let mut out = Vec::with_capacity(obj.len());
    for (k, v) in obj {
        let s = v
            .as_str()
            .ok_or_else(|| anyhow!("{ctx} values must be strings"))?;
        out.push((k.clone(), s.to_string()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &std::path::Path, name: &str, contents: &str) -> String {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn cfg(dir: &std::path::Path) -> Config {
        // A bare config (not from_env, so the ambient HORDE_* vars of the dev
        // environment don't leak in); tests set the fields they exercise.
        Config {
            projects_dir: dir.to_path_buf(),
            state_dir: dir.to_path_buf(),
            sandbox_path: None,
            claude_token_file: None,
            github_token_files: None,
            gh_app_id: None,
            gh_app_key_file: None,
            token_files: None,
            ro_paths: None,
            rw_paths: None,
            allow_nix: None,
            settings: String::new(),
            user_name: "horde".into(),
        }
    }

    #[test]
    fn classifies_claude_token_and_strips_newline() {
        let dir = std::env::temp_dir().join(format!("horde-secrets-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let oat = write_file(&dir, "oat", "sk-ant-oat-abc\n\n");
        let api = write_file(&dir, "api", "some-api-key\n");

        let mut c = cfg(&dir);
        c.claude_token_file = Some(oat);
        assert_eq!(
            collect(&c).unwrap().vars,
            vec![(
                "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                "sk-ant-oat-abc".to_string()
            )]
        );

        c.claude_token_file = Some(api);
        assert_eq!(
            collect(&c).unwrap().vars,
            vec![("ANTHROPIC_API_KEY".to_string(), "some-api-key".to_string())]
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn github_owners_and_default() {
        let dir = std::env::temp_dir().join(format!("horde-secrets-gh-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let def = write_file(&dir, "def", "deftok\n");
        let acme = write_file(&dir, "acme", "acmetok\n");

        let mut c = cfg(&dir);
        c.github_token_files = Some(format!(r#"{{"default":"{def}","acme-corp":"{acme}"}}"#));
        let s = collect(&c).unwrap();
        assert!(s.have_gh_default);
        assert_eq!(
            s.gh_owners,
            vec![(
                "acme-corp".to_string(),
                "HORDE_GH_TOKEN_acme_corp".to_string()
            )]
        );
        assert!(s
            .vars
            .contains(&("GH_TOKEN".to_string(), "deftok".to_string())));
        assert!(s.vars.contains(&(
            "HORDE_GH_TOKEN_acme_corp".to_string(),
            "acmetok".to_string()
        )));
        fs::remove_dir_all(&dir).ok();
    }
}
