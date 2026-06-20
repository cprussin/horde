//! Render the per-owner `gh` wrapper script that shadows `gh` inside the
//! sandbox.  It must reproduce, byte-for-byte, the script `horde-run.sh`
//! generates with its here-doc (lines 300-332): the wrapper strips its own
//! directory from PATH, derives the repo owner from `git remote`, resolves the
//! credential via `git credential fill`, sets GH_TOKEN, and delegates to gh.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// `@REAL_BASH@`/`@WRAPPER_DIR@` stand in for the here-doc's expanded
/// `$real_bash`/`$wrapper_dir`; every other `$` is literal (the wrapper shell
/// expands it at git-time inside the sandbox).
const TEMPLATE: &str = r#"#!@REAL_BASH@
set -u
new_path=""
IFS=:
for d in $PATH; do
  [ "$d" = "@WRAPPER_DIR@" ] && continue
  new_path="${new_path:+$new_path:}$d"
done
unset IFS
export PATH="$new_path"
remote="$(git remote get-url origin 2>/dev/null || true)"
case "$remote" in
  *github.com[:/]*)
    rest="${remote##*github.com}"
    rest="${rest#[:/]}"
    rest="${rest%.git}"
    owner="${rest%%/*}"
    repo="${rest#*/}"
    repo="${repo%%/*}"
    ;;
  *) owner=""; repo="" ;;
esac
if [ -n "$owner" ] && [ -n "$repo" ]; then
  token="$(printf 'protocol=https\nhost=github.com\npath=%s/%s\n\n' "$owner" "$repo" | git credential fill 2>/dev/null | sed -n 's/^password=//p' | head -n1)"
  if [ -n "$token" ]; then
    GH_TOKEN="$token"
    GITHUB_TOKEN="$token"
    export GH_TOKEN GITHUB_TOKEN
  fi
fi
exec gh "$@"
"#;

pub fn render(real_bash: &str, wrapper_dir: &str) -> String {
    TEMPLATE
        .replace("@REAL_BASH@", real_bash)
        .replace("@WRAPPER_DIR@", wrapper_dir)
}

/// Write the wrapper to `<state_dir>/bin/gh`, executable.
pub fn write(state_dir: &Path, real_bash: &str, wrapper_dir: &str) -> io::Result<()> {
    let bin = state_dir.join("bin");
    fs::create_dir_all(&bin)?;
    let path = bin.join("gh");
    fs::write(&path, render(real_bash, wrapper_dir))?;
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Golden test: the rendered wrapper must equal what bash's here-doc
    /// produces for the same `$real_bash`/`$wrapper_dir` (copied verbatim from
    /// horde-run.sh).  This guards the `$`-escaping that's easy to get wrong.
    #[test]
    fn matches_bash_heredoc() {
        let real_bash = "/nix/store/xxxx-bash/bin/bash";
        let wrapper_dir = "/home/horde/bin";
        let script = format!(
            r#"real_bash={real_bash}
wrapper_dir={wrapper_dir}
cat << EOF
#!$real_bash
set -u
new_path=""
IFS=:
for d in \$PATH; do
  [ "\$d" = "$wrapper_dir" ] && continue
  new_path="\${{new_path:+\$new_path:}}\$d"
done
unset IFS
export PATH="\$new_path"
remote="\$(git remote get-url origin 2>/dev/null || true)"
case "\$remote" in
  *github.com[:/]*)
    rest="\${{remote##*github.com}}"
    rest="\${{rest#[:/]}}"
    rest="\${{rest%.git}}"
    owner="\${{rest%%/*}}"
    repo="\${{rest#*/}}"
    repo="\${{repo%%/*}}"
    ;;
  *) owner=""; repo="" ;;
esac
if [ -n "\$owner" ] && [ -n "\$repo" ]; then
  token="\$(printf 'protocol=https\nhost=github.com\npath=%s/%s\n\n' "\$owner" "\$repo" | git credential fill 2>/dev/null | sed -n 's/^password=//p' | head -n1)"
  if [ -n "\$token" ]; then
    GH_TOKEN="\$token"
    GITHUB_TOKEN="\$token"
    export GH_TOKEN GITHUB_TOKEN
  fi
fi
exec gh "\$@"
EOF
"#
        );
        let out = Command::new("bash")
            .arg("-c")
            .arg(&script)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "bash failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let expected = String::from_utf8(out.stdout).unwrap();
        assert_eq!(render(real_bash, wrapper_dir), expected);
    }
}
