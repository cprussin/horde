//! Persistent, cross-run prompt history backing the interactive input.
//!
//! Entries are stored one per line.  Multi-line prompts are flattened to a
//! single space-joined line on write so they round-trip through the file as
//! one history entry (matching the old bash readline behaviour).

use std::fs;
use std::path::Path;

pub struct History {
    entries: Vec<String>,
    /// Cursor into `entries`; `entries.len()` means "the fresh, unsaved line".
    idx: usize,
}

impl History {
    pub fn load(path: &Path) -> Self {
        let entries: Vec<String> = fs::read_to_string(path)
            .ok()
            .map(|c| {
                c.lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let idx = entries.len();
        History { entries, idx }
    }

    /// Step to an older entry (bounded at the oldest).
    pub fn prev(&mut self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        self.idx = self.idx.saturating_sub(1);
        Some(self.entries[self.idx].clone())
    }

    /// Step to a newer entry; past the newest returns the empty fresh line.
    pub fn next(&mut self) -> String {
        if self.idx + 1 >= self.entries.len() {
            self.idx = self.entries.len();
            return String::new();
        }
        self.idx += 1;
        self.entries[self.idx].clone()
    }

    /// Record a submitted prompt and persist it (best-effort).
    pub fn append(&mut self, prompt: &str, path: &Path) {
        let flat = prompt.replace('\n', " ");
        let flat = flat.trim();
        if flat.is_empty() {
            return;
        }
        // Skip consecutive duplicates.
        if self.entries.last().map(|l| l == flat).unwrap_or(false) {
            return;
        }
        self.entries.push(flat.to_string());
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, self.entries.join("\n") + "\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn hist(entries: &[&str]) -> History {
        History {
            entries: entries.iter().map(|s| s.to_string()).collect(),
            idx: entries.len(),
        }
    }

    #[test]
    fn prev_walks_back_and_clamps() {
        let mut h = hist(&["one", "two"]);
        assert_eq!(h.prev().as_deref(), Some("two"));
        assert_eq!(h.prev().as_deref(), Some("one"));
        assert_eq!(h.prev().as_deref(), Some("one")); // clamped at oldest
    }

    #[test]
    fn next_returns_to_empty() {
        let mut h = hist(&["one", "two"]);
        h.prev();
        h.prev();
        assert_eq!(h.next(), "two");
        assert_eq!(h.next(), ""); // past newest → fresh line
    }

    #[test]
    fn prev_on_empty_is_none() {
        let mut h = hist(&[]);
        assert_eq!(h.prev(), None);
    }

    #[test]
    fn append_flattens_and_dedupes() {
        let mut h = hist(&[]);
        let path = PathBuf::from("/nonexistent/dir/file"); // write fails silently
        h.append("multi\nline", &path);
        h.append("multi line", &path); // duplicate of the flattened form
        assert_eq!(h.entries, vec!["multi line"]);
    }
}
