use std::path::Path;

/// File/directory path completion state.
#[derive(Debug, Clone)]
pub struct FileCompletion {
    /// Sorted candidate paths (directories have trailing `/`).
    pub candidates: Vec<String>,
    /// Index of the currently selected candidate.
    pub selected: usize,
    /// The prefix that was matched against.
    pub prefix: String,
    /// Column offset in the input line where the prefix starts.
    pub start_col: usize,
}

impl FileCompletion {
    /// Build completions from a prefix path relative to `working_dir`.
    ///
    /// Returns `None` if no candidates match.
    pub fn from_prefix(prefix: &str, working_dir: &Path) -> Option<Self> {
        let (dir_part, file_part) = match prefix.rfind('/') {
            Some(pos) => (&prefix[..=pos], &prefix[pos + 1..]),
            None => {
                if prefix == "~" {
                    ("~/", "") // Force home dir expansion instead of local file match
                } else {
                    ("", prefix)
                }
            }
        };

        // Resolve the directory to search in
        let search_dir = if dir_part.is_empty() {
            working_dir.to_path_buf()
        } else if dir_part.starts_with('~') {
            if let Some(home) = dirs_home(working_dir) {
                home.join(&dir_part[2..]) // skip "~/"
            } else {
                working_dir.join(dir_part)
            }
        } else {
            working_dir.join(dir_part)
        };

        let entries = std::fs::read_dir(&search_dir).ok()?;
        let mut candidates: Vec<String> = Vec::new();

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Skip hidden files unless the prefix explicitly starts with '.'
            if name.starts_with('.') && !file_part.starts_with('.') {
                continue;
            }
            if name.starts_with(file_part) {
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                let full = if is_dir {
                    format!("{}{}/", dir_part, name)
                } else {
                    format!("{}{}", dir_part, name)
                };
                candidates.push(full);
            }
        }

        if candidates.is_empty() {
            return None;
        }

        candidates.sort();

        Some(Self {
            candidates,
            selected: 0,
            prefix: prefix.to_string(),
            start_col: 0, // set by caller
        })
    }

    /// Cycle to the next candidate.
    pub fn next(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = (self.selected + 1) % self.candidates.len();
        }
    }

    /// Cycle to the previous candidate.
    pub fn prev(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = if self.selected == 0 {
                self.candidates.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    /// Get the currently selected candidate, if any.
    pub fn selected_candidate(&self) -> Option<&str> {
        self.candidates.get(self.selected).map(|s| s.as_str())
    }
}

/// Returns true if the token looks like a file path (contains `/`, starts with `.` or `~`).
pub fn is_path_like(s: &str) -> bool {
    s.contains('/') || s.starts_with('.') || s.starts_with('~')
}

/// Attempt to resolve home directory. Falls back to None.
fn dirs_home(_working_dir: &Path) -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn is_path_like_matches() {
        assert!(is_path_like("src/main.rs"));
        assert!(is_path_like("./foo"));
        assert!(is_path_like("~/bar"));
        assert!(!is_path_like("hello"));
        assert!(!is_path_like("world"));
    }

    #[test]
    fn completion_from_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        fs::create_dir(base.join("src")).unwrap();
        fs::write(base.join("Cargo.toml"), "").unwrap();
        fs::write(base.join("README.md"), "").unwrap();

        let fc = FileCompletion::from_prefix("", base).unwrap();
        assert!(fc.candidates.len() >= 3);

        let fc = FileCompletion::from_prefix("C", base).unwrap();
        assert!(fc.candidates.contains(&"Cargo.toml".to_string()));

        let fc = FileCompletion::from_prefix("sr", base).unwrap();
        assert_eq!(fc.candidates, vec!["src/"]);
    }

    #[test]
    fn completion_cycling() {
        let mut fc = FileCompletion {
            candidates: vec!["a".into(), "b".into(), "c".into()],
            selected: 0,
            prefix: String::new(),
            start_col: 0,
        };
        assert_eq!(fc.selected_candidate(), Some("a"));
        fc.next();
        assert_eq!(fc.selected_candidate(), Some("b"));
        fc.next();
        assert_eq!(fc.selected_candidate(), Some("c"));
        fc.next();
        assert_eq!(fc.selected_candidate(), Some("a")); // wraps
        fc.prev();
        assert_eq!(fc.selected_candidate(), Some("c")); // wraps back
    }

    #[test]
    fn no_matches_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(FileCompletion::from_prefix("zzz_nonexistent", dir.path()).is_none());
    }
}
