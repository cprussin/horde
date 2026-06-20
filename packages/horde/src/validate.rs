//! Project-name validation, matching `horde.sh`'s `check_name`.

use std::path::Path;

use anyhow::{bail, Result};

/// Validate a project name's charset and that it resolves to a directory under
/// `projects_dir`.  Error messages are kept identical to the bash version.
pub fn check_name(projects_dir: &Path, name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        bail!("invalid project name: {name}");
    }
    if !projects_dir.join(name).is_dir() {
        bail!("no such project: {}/{}", projects_dir.display(), name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rejects_bad_names() {
        let dir = PathBuf::from("/nonexistent");
        for bad in ["", ".hidden", "foo/bar", "foo bar", "foo*", "naïve"] {
            let err = check_name(&dir, bad).unwrap_err().to_string();
            assert_eq!(err, format!("invalid project name: {bad}"), "for {bad:?}");
        }
    }

    #[test]
    fn accepts_charset_but_reports_missing_dir() {
        let dir = PathBuf::from("/nonexistent");
        // Valid charset, so it passes the name check and fails on existence.
        let err = check_name(&dir, "foo.bar_baz-1").unwrap_err().to_string();
        assert_eq!(err, "no such project: /nonexistent/foo.bar_baz-1");
    }
}
