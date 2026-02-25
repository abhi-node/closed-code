use std::sync::Arc;
use tokio::sync::RwLock;
use std::path::PathBuf;

/// A background indexer that recursively scans the workspace and provides fast fuzzy matching.
/// Uses the `nucleo` crate for high-performance fuzzy matching without blocking the TUI.
pub struct FileIndexer {
    matcher: nucleo::Nucleo<String>,
    is_indexing: Arc<RwLock<bool>>,
    working_dir: PathBuf,
}

impl FileIndexer {
    /// Initialize the file indexer and spawn a background thread to walk the working directory.
    pub fn new(working_dir: std::path::PathBuf) -> Self {
        let matcher = nucleo::Nucleo::new(
            nucleo::Config::DEFAULT,
            Arc::new(|| {}),
            None,
            1,
        );
        let injector = matcher.injector();
        let is_indexing = Arc::new(RwLock::new(true));
        
        let indexing_flag = is_indexing.clone();
        let dir = working_dir.clone();
        
        // Spawn the indexing task on a blocking thread because ignore::WalkBuilder is synchronous
        tokio::task::spawn_blocking(move || {
            Self::walk_and_index(&dir, injector);
            
            // Mark indexing as complete
            let mut flag = indexing_flag.blocking_write();
            *flag = false;
        });

        Self { matcher, is_indexing, working_dir }
    }

    /// Drops the old matcher, creates a new one, and respawns the background indexing thread.
    pub fn refresh(&mut self) {
        let matcher = nucleo::Nucleo::new(
            nucleo::Config::DEFAULT,
            Arc::new(|| {}),
            None,
            1,
        );
        let injector = matcher.injector();
        let is_indexing = Arc::new(RwLock::new(true));
        
        let indexing_flag = is_indexing.clone();
        let dir = self.working_dir.clone();
        
        tokio::task::spawn_blocking(move || {
            Self::walk_and_index(&dir, injector);
            
            // Mark indexing as complete
            let mut flag = indexing_flag.blocking_write();
            *flag = false;
        });

        self.matcher = matcher;
        self.is_indexing = is_indexing;
    }

    fn walk_and_index(working_dir: &std::path::Path, injector: nucleo::Injector<String>) {
        let walker = ignore::WalkBuilder::new(working_dir)
            .hidden(true)
            .git_ignore(true)
            .ignore(true)
            // If a .closed-code-ignore exists, WalkBuilder will pick it up automatically if added as standard ignore
            .add_custom_ignore_filename(".closed-code-ignore")
            .build();
            
        for result in walker.flatten() {
            if let Some(file_type) = result.file_type() {
                if file_type.is_file() {
                    if let Ok(rel_path) = result.path().strip_prefix(working_dir) {
                        let path_str = rel_path.to_string_lossy().into_owned();
                        injector.push(path_str, |item, chars| {
                            chars[0] = item.as_str().into();
                        });
                    }
                }
            }
        }
    }

    /// Search the index for the given query and return the top `limit` matches.
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<String> {
        self.matcher.pattern.reparse(
            0,
            query,
            nucleo::pattern::CaseMatching::Smart,
            nucleo::pattern::Normalization::Smart,
            false,
        );
        
        self.matcher.tick(10); // allow nucleo to process pending items

        let snapshot = self.matcher.snapshot();
        let count = snapshot.matched_item_count().min(limit as u32);
        
        let mut results = Vec::with_capacity(count as usize);
        for i in 0..count {
            if let Some(item) = snapshot.get_matched_item(i) {
                results.push(item.data.clone());
            }
        }
        
        results
    }
    
    /// Returns true if the background thread is still indexing files.
    pub async fn is_indexing(&self) -> bool {
        *self.is_indexing.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;
    use std::io::Write;

    #[tokio::test]
    async fn test_file_indexer() {
        let dir = tempdir().unwrap();
        let path1 = dir.path().join("file1.txt");
        let path2 = dir.path().join("hidden.txt");
        let ignore_path = dir.path().join(".gitignore");
        
        let mut f1 = File::create(&path1).unwrap();
        f1.write_all(b"test").unwrap();
        
        let mut f2 = File::create(&path2).unwrap();
        f2.write_all(b"test").unwrap();
        
        let mut ig = File::create(&ignore_path).unwrap();
        ig.write_all(b"hidden.txt\n").unwrap();
        
        // Wait for OS to flush files
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut indexer = FileIndexer::new(dir.path().to_path_buf());
        
        // Wait for indexing to complete
        while indexer.is_indexing().await {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        
        let matches = indexer.search("file1", 10);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].contains("file1.txt"));
        
        let matches_hidden = indexer.search("hidden", 10);
        assert_eq!(matches_hidden.len(), 0); // Should be ignored by .gitignore
    }
}
