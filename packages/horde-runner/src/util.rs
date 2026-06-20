//! Small shared helpers.

use std::env;
use std::path::PathBuf;

/// Resolve an executable on PATH, like `command -v`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Quote one token for readable `--dry-run` output: bare when it contains only
/// safe characters, single-quoted otherwise.
pub fn shell_word(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '.' | '/' | ':' | '=' | '@' | '%' | '+' | ',' | '-')
        });
    if safe {
        s.to_string()
    } else {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for ch in s.chars() {
            if ch == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(ch);
            }
        }
        out.push('\'');
        out
    }
}
