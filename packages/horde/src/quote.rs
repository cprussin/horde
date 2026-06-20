//! Shell quoting for the remote ssh command string (`attach::remote_command`).
//!
//! Single-quote wrapping: a single-quoted string reproduces its bytes verbatim
//! (the only character it can't contain is `'` itself, handled by closing the
//! quote, inserting an escaped `'`, and reopening), so it's a safe shell word
//! for the project name and the inner `horde-runner serve` invocation passed
//! through the remote login shell.

/// Quote `s` as a single POSIX shell word.
pub fn bash_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn empty_string() {
        assert_eq!(bash_quote(""), "''");
    }

    #[test]
    fn embedded_single_quote() {
        assert_eq!(bash_quote("it's"), "'it'\\''s'");
    }

    /// The real test: whatever we emit, a shell must parse it back to exactly
    /// the original bytes.  Round-trip each case through `bash -c`.
    #[test]
    fn round_trips_through_bash() {
        let cases = [
            "plain",
            "a b c",
            "it's a test",
            "a\"b",
            "$(rm -rf /)",
            "`backticks`",
            "semi;colon|pipe&amp",
            "tab\there",
            "new\nline",
            "-leading-dash",
            "[\"json\",\"array\"]",
            "${HORDE_TMUX:-tmux}",
        ];
        for case in cases {
            let script = format!("printf %s {}", bash_quote(case));
            let out = Command::new("bash")
                .arg("-c")
                .arg(&script)
                .output()
                .unwrap();
            assert!(out.status.success(), "bash failed for {case:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                case,
                "round-trip mismatch for {case:?}"
            );
        }
    }
}
