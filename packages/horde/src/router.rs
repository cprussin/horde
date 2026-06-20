//! Project routing: catalog the projects, ask a cheap Claude model which
//! one(s) the prompt refers to, and parse its answer.  Mirrors the routing
//! block of `horde.sh`.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Result};

use crate::config::Config;

/// Run the headless routing call and return the selected project names.
pub fn route(config: &Config, prompt: &str) -> Result<Vec<String>> {
    let catalog = build_catalog(&config.projects_dir)?;
    if catalog.is_empty() {
        bail!("no projects found in {}", config.projects_dir.display());
    }

    let routing_prompt = format!(
        "You route requests to projects.  Below is a list of projects in the \
form 'name :: description'.

Projects:
{catalog}
Request: {prompt}

Respond with ONLY a JSON array of the project directory names the request \
refers to, most relevant first, e.g. [\"foo\"] or [\"api\",\"worker\"].  Use \
the names exactly as listed.  If nothing matches, respond with []."
    );

    let mut cmd = Command::new("claude");
    cmd.args([
        "--print",
        "--output-format",
        "json",
        "--model",
        &config.router_model,
        &routing_prompt,
    ])
    .stdin(Stdio::null());
    if let Some((key, value)) = authenticate_router(config)? {
        cmd.env(key, value);
    }

    let output = cmd
        .output()
        .map_err(|e| anyhow!("routing call failed:\n{e}"))?;
    if !output.status.success() {
        // Surface stderr (auth, model, network) rather than swallow it.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr.trim().is_empty() {
            stdout.trim_end()
        } else {
            stderr.trim_end()
        };
        bail!("routing call failed:\n{msg}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // `.result // empty`: invalid JSON or a missing field reads as empty,
    // which then fails array extraction with the same message bash gives.
    let router_text = serde_json::from_str::<serde_json::Value>(&stdout)
        .ok()
        .and_then(|v| v.get("result").and_then(|r| r.as_str()).map(str::to_string))
        .unwrap_or_default();

    let array_json = extract_array(&router_text)
        .ok_or_else(|| anyhow!("router did not return a project list: {router_text}"))?;

    // A parse failure (non-string elements, malformed array) yields an empty
    // selection, which surfaces as "no project matched" upstream — matching
    // bash's `jq` behaviour.
    Ok(serde_json::from_str(&array_json).unwrap_or_default())
}

/// Catalog projects from their `CLAUDE.md` headers: one `name :: description`
/// line per directory, sorted by path.
fn build_catalog(projects_dir: &Path) -> Result<String> {
    let mut dirs: Vec<_> = fs::read_dir(projects_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let mut catalog = String::new();
    for dir in dirs {
        let name = dir.file_name().unwrap_or_default().to_string_lossy();
        let desc = fs::read_to_string(dir.join("CLAUDE.md"))
            .ok()
            .map(|c| c.lines().take(5).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        catalog.push_str(&format!("{name} :: {desc}\n"));
    }
    Ok(catalog)
}

/// Authenticate the routing call from the configured token file when no Claude
/// token is already in the environment.  Returns the env var to set on the
/// `claude` subprocess.
fn authenticate_router(config: &Config) -> Result<Option<(&'static str, String)>> {
    for var in [
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
    ] {
        if std::env::var(var).map(|v| !v.is_empty()).unwrap_or(false) {
            return Ok(None);
        }
    }
    let file = match &config.claude_token_file {
        Some(f) => f,
        None => return Ok(None),
    };
    let token =
        fs::read_to_string(file).map_err(|_| anyhow!("cannot read claude token file: {file}"))?;
    let token = token.trim().to_string();
    let key = if token.starts_with("sk-ant-oat") {
        "CLAUDE_CODE_OAUTH_TOKEN"
    } else {
        "ANTHROPIC_API_KEY"
    };
    Ok(Some((key, token)))
}

/// Extract the first `[...]` JSON array from the router's free-text reply.
/// Like bash's `grep -o '\[.*\]' | head -n1`: per line, greedy from the first
/// `[` to the last `]`.
fn extract_array(text: &str) -> Option<String> {
    for line in text.lines() {
        if let (Some(start), Some(end)) = (line.find('['), line.rfind(']')) {
            if start <= end {
                return Some(line[start..=end].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bare_array() {
        assert_eq!(extract_array("[\"foo\"]").as_deref(), Some("[\"foo\"]"));
    }

    #[test]
    fn extracts_array_amid_prose() {
        assert_eq!(
            extract_array("Here you go: [\"api\",\"worker\"] cheers").as_deref(),
            Some("[\"api\",\"worker\"]")
        );
    }

    #[test]
    fn greedy_to_last_bracket() {
        assert_eq!(
            extract_array("[\"a\"] and [\"b\"]").as_deref(),
            Some("[\"a\"] and [\"b\"]")
        );
    }

    #[test]
    fn first_matching_line_wins() {
        assert_eq!(
            extract_array("no array here\n[\"x\"]\n[\"y\"]").as_deref(),
            Some("[\"x\"]")
        );
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(extract_array("nothing to see"), None);
        assert_eq!(extract_array(""), None);
    }

    #[test]
    fn empty_array_parses_to_empty_vec() {
        let v: Vec<String> = serde_json::from_str(&extract_array("[]").unwrap()).unwrap();
        assert!(v.is_empty());
    }
}
