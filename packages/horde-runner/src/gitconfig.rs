//! Generate the managed GitHub credential gitconfig and the `[include]` that
//! pulls it into the sandbox HOME's `.gitconfig`, mirroring lines 255-288 of
//! `horde-run.sh`.
//!
//! Helpers are emitted most-specific-first (per-owner PATs → App → default
//! catch-all); the first to answer wins.  PAT values stay in the environment,
//! referenced by name (`$VAR`), expanded by the helper shell inside the
//! sandbox — never written to disk here.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::secrets::Secrets;

/// Render the contents of `horde-github.gitconfig`.
pub fn render(secrets: &Secrets, gh_app_helper: Option<&str>) -> String {
    let mut out = String::new();
    let any = !secrets.gh_owners.is_empty() || secrets.have_gh_app;

    // useHttpPath makes git send the repo path, which the per-owner and App
    // helpers need to know the owner.
    if any {
        out.push_str("[credential \"https://github.com\"]\n\tuseHttpPath = true\n");
    }
    for (owner, var) in &secrets.gh_owners {
        out.push_str(&format!(
            "[credential \"https://github.com/{owner}\"]\n\thelper = \"!f() {{ echo username=x-access-token; echo \\\"password=${var}\\\"; }}; f\"\n"
        ));
    }
    if secrets.have_gh_app {
        let helper = gh_app_helper.expect("gh app helper path required when have_gh_app");
        out.push_str(&format!(
            "[credential \"https://github.com\"]\n\thelper = \"!{helper}\"\n"
        ));
    }
    if secrets.have_gh_default {
        for host in ["https://github.com", "https://gist.github.com"] {
            out.push_str(&format!(
                "[credential \"{host}\"]\n\thelper = \"!f() {{ echo username=x-access-token; echo \\\"password=$GH_TOKEN\\\"; }}; f\"\n"
            ));
        }
    }
    out
}

/// Write `horde-github.gitconfig` (overwritten each run) and ensure the
/// sandbox `.gitconfig` includes it (appended once, idempotently).
pub fn write(
    state_dir: &Path,
    sandbox_home: &Path,
    secrets: &Secrets,
    gh_app_helper: Option<&str>,
) -> io::Result<()> {
    fs::write(
        state_dir.join("horde-github.gitconfig"),
        render(secrets, gh_app_helper),
    )?;

    let gitconfig = state_dir.join(".gitconfig");
    let include_path = sandbox_home.join("horde-github.gitconfig");
    let include_str = include_path.to_string_lossy();
    let existing = fs::read_to_string(&gitconfig).unwrap_or_default();
    if !existing.contains(include_str.as_ref()) {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitconfig)?;
        write!(f, "[include]\n\tpath = {include_str}\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_owner_app_and_default_in_order() {
        let secrets = Secrets {
            vars: vec![],
            gh_owners: vec![("acme-corp".into(), "HORDE_GH_TOKEN_acme_corp".into())],
            have_gh_default: true,
            have_gh_app: true,
        };
        let out = render(&secrets, Some("/nix/store/x/bin/horde-gh-app-credential"));
        let expected = "\
[credential \"https://github.com\"]
\tuseHttpPath = true
[credential \"https://github.com/acme-corp\"]
\thelper = \"!f() { echo username=x-access-token; echo \\\"password=$HORDE_GH_TOKEN_acme_corp\\\"; }; f\"
[credential \"https://github.com\"]
\thelper = \"!/nix/store/x/bin/horde-gh-app-credential\"
[credential \"https://github.com\"]
\thelper = \"!f() { echo username=x-access-token; echo \\\"password=$GH_TOKEN\\\"; }; f\"
[credential \"https://gist.github.com\"]
\thelper = \"!f() { echo username=x-access-token; echo \\\"password=$GH_TOKEN\\\"; }; f\"
";
        assert_eq!(out, expected);
    }

    #[test]
    fn empty_when_no_github() {
        let out = render(&Secrets::default(), None);
        assert_eq!(out, "");
    }
}
