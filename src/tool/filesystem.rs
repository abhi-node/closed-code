use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::FunctionDeclaration;

use super::{ParamBuilder, Tool};

const MAX_FILE_SIZE: u64 = 100 * 1024; // 100KB

// ── ReadFileTool ──

#[derive(Debug)]
pub struct ReadFileTool {
    working_directory: PathBuf,
}

impl ReadFileTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }

    /// Resolve a path relative to working_directory.
    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let resolved = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.working_directory.join(path)
        };
        let canonical = resolved.canonicalize().map_err(ClosedCodeError::Io)?;
        Ok(canonical)
    }

    /// Check if file content appears to be binary (contains null bytes).
    pub fn is_binary(content: &[u8]) -> bool {
        let check_len = content.len().min(8192);
        content[..check_len].contains(&0)
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the file content with line numbers. \
         Supports optional start_line and end_line to read a specific range. \
         Large files (>100KB) are truncated with a warning."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "path",
                    "Path to the file to read (relative to working directory)",
                    true,
                )
                .integer(
                    "start_line",
                    "First line to read (1-indexed, inclusive). Omit to start from beginning.",
                    false,
                )
                .integer(
                    "end_line",
                    "Last line to read (1-indexed, inclusive). Omit to read to end.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "read_file".into(),
                message: "Missing required parameter 'path'".into(),
            })?;

        let path = self.resolve_path(path_str)?;

        // Check file size
        let metadata = fs::metadata(&path)
            .await
            .map_err(|e| ClosedCodeError::ToolError {
                name: "read_file".into(),
                message: format!("Cannot read '{}': {}", path_str, e),
            })?;

        let file_size = metadata.len();
        let truncated = file_size > MAX_FILE_SIZE;

        // Read file bytes
        let bytes = if truncated {
            let mut buf = vec![0u8; MAX_FILE_SIZE as usize];
            let mut file = fs::File::open(&path).await?;
            use tokio::io::AsyncReadExt;
            let n = file.read(&mut buf).await?;
            buf.truncate(n);
            buf
        } else {
            fs::read(&path)
                .await
                .map_err(|e| ClosedCodeError::ToolError {
                    name: "read_file".into(),
                    message: format!("Cannot read '{}': {}", path_str, e),
                })?
        };

        // Binary detection
        if Self::is_binary(&bytes) {
            return Ok(json!({
                "error": format!("Binary file detected: {}", path_str),
                "file_size": file_size,
            }));
        }

        let content = String::from_utf8_lossy(&bytes);

        // Apply line range
        let start_line = args["start_line"].as_u64().map(|n| n as usize);
        let end_line = args["end_line"].as_u64().map(|n| n as usize);

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = start_line.unwrap_or(1).saturating_sub(1).min(total_lines);
        let end = end_line.unwrap_or(total_lines).min(total_lines).max(start);

        let selected: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4}| {}", start + i + 1, line))
            .collect();

        let output = selected.join("\n");

        let mut result = json!({
            "content": output,
            "path": path_str,
            "total_lines": total_lines,
            "lines_shown": format!("{}-{}", start + 1, end),
        });

        if truncated {
            result["warning"] = json!(format!(
                "File truncated: showing first {}KB of {}KB",
                MAX_FILE_SIZE / 1024,
                file_size / 1024,
            ));
        }

        Ok(result)
    }
}

// ── ListDirectoryTool ──

#[derive(Debug)]
pub struct ListDirectoryTool {
    working_directory: PathBuf,
}

impl ListDirectoryTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }
}

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List the contents of a directory. Returns file names, sizes, and types. \
         Respects .gitignore rules. Use recursive=true to list subdirectories."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "path",
                    "Directory path (relative to working directory). Defaults to '.'",
                    false,
                )
                .boolean(
                    "recursive",
                    "If true, list subdirectories recursively. Defaults to false.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let path_str = args["path"].as_str().unwrap_or(".");
        let recursive = args["recursive"].as_bool().unwrap_or(false);

        let dir_path = if Path::new(path_str).is_absolute() {
            PathBuf::from(path_str)
        } else {
            self.working_directory.join(path_str)
        };

        if !dir_path.is_dir() {
            return Ok(json!({
                "error": format!("Not a directory: {}", path_str),
            }));
        }

        let entries = tokio::task::spawn_blocking(move || {
            use ignore::WalkBuilder;

            let mut builder = WalkBuilder::new(&dir_path);
            if !recursive {
                builder.max_depth(Some(1));
            }
            builder
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true);

            let mut entries = Vec::new();
            for entry in builder.build().flatten() {
                if entry.path() == dir_path {
                    continue;
                }

                let relative = entry
                    .path()
                    .strip_prefix(&dir_path)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .to_string();

                let file_type = if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    "directory"
                } else {
                    "file"
                };

                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

                entries.push(json!({
                    "name": relative,
                    "type": file_type,
                    "size": size,
                }));
            }
            entries
        })
        .await
        .map_err(|e| ClosedCodeError::ToolError {
            name: "list_directory".into(),
            message: format!("Failed to list directory: {}", e),
        })?;

        Ok(json!({
            "path": path_str,
            "entries": entries,
            "count": entries.len(),
        }))
    }
}

// ── SearchFilesTool ──

#[derive(Debug)]
pub struct SearchFilesTool {
    working_directory: PathBuf,
}

impl SearchFilesTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }
}

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }

    fn description(&self) -> &str {
        "Search for files matching a glob pattern (e.g., '**/*.rs', 'src/**/*.toml'). \
         Returns matching file paths relative to the working directory. Respects .gitignore."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "pattern",
                    "Glob pattern to match files (e.g., '**/*.rs')",
                    true,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "search_files".into(),
                message: "Missing required parameter 'pattern'".into(),
            })?;

        let wd = self.working_directory.clone();
        let pattern = pattern.to_string();

        let matches = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let full_pattern = wd.join(&pattern).to_string_lossy().to_string();

            let paths: Vec<String> = glob::glob(&full_pattern)
                .map_err(|e| ClosedCodeError::GlobError(e.to_string()))?
                .filter_map(|entry| entry.ok())
                .filter_map(|path| {
                    path.strip_prefix(&wd)
                        .ok()
                        .map(|rel| rel.to_string_lossy().to_string())
                })
                .collect();

            Ok(paths)
        })
        .await
        .map_err(|e| ClosedCodeError::ToolError {
            name: "search_files".into(),
            message: format!("Search failed: {}", e),
        })??;

        Ok(json!({
            "pattern": args["pattern"],
            "matches": matches,
            "count": matches.len(),
        }))
    }
}

// ── GrepTool ──

#[derive(Debug)]
pub struct GrepTool {
    working_directory: PathBuf,
}

const MAX_MATCHES: usize = 100;

impl GrepTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents using a regex pattern. Returns matching lines with \
         file paths and line numbers. Optionally filter by file glob pattern. \
         Respects .gitignore. Results capped at 100 matches."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string("pattern", "Regex pattern to search for", true)
                .string(
                    "file_pattern",
                    "Optional glob to filter files (e.g., '*.rs'). Defaults to all files.",
                    false,
                )
                .boolean(
                    "case_insensitive",
                    "If true, search case-insensitively. Defaults to false.",
                    false,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "grep".into(),
                message: "Missing required parameter 'pattern'".into(),
            })?;

        let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
        let file_pattern = args["file_pattern"].as_str().map(|s| s.to_string());

        let regex_pattern = if case_insensitive {
            format!("(?i){}", pattern)
        } else {
            pattern.to_string()
        };

        let re = regex::Regex::new(&regex_pattern)
            .map_err(|e| ClosedCodeError::RegexError(e.to_string()))?;

        let wd = self.working_directory.clone();

        let matches = tokio::task::spawn_blocking(move || -> Result<Vec<Value>> {
            use ignore::WalkBuilder;
            use std::fs::File;
            use std::io::{BufRead, BufReader};

            let mut results = Vec::new();

            let walker = WalkBuilder::new(&wd).hidden(false).git_ignore(true).build();

            for entry in walker {
                if results.len() >= MAX_MATCHES {
                    break;
                }

                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                if entry.file_type().is_none_or(|ft| ft.is_dir()) {
                    continue;
                }

                let path = entry.path();

                // Apply file pattern filter
                if let Some(ref fp) = file_pattern {
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if let Ok(glob_pattern) = glob::Pattern::new(fp) {
                        if !glob_pattern.matches(file_name) {
                            continue;
                        }
                    }
                }

                let file = match File::open(path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                let reader = BufReader::new(file);
                for (line_num, line) in reader.lines().enumerate() {
                    if results.len() >= MAX_MATCHES {
                        break;
                    }

                    let line = match line {
                        Ok(l) => l,
                        Err(_) => continue,
                    };

                    if re.is_match(&line) {
                        let relative = path
                            .strip_prefix(&wd)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .to_string();

                        results.push(json!({
                            "file": relative,
                            "line": line_num + 1,
                            "content": line.trim(),
                        }));
                    }
                }
            }

            Ok(results)
        })
        .await
        .map_err(|e| ClosedCodeError::ToolError {
            name: "grep".into(),
            message: format!("Search failed: {}", e),
        })??;

        let truncated = matches.len() >= MAX_MATCHES;

        Ok(json!({
            "pattern": args["pattern"],
            "matches": matches,
            "count": matches.len(),
            "truncated": truncated,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Mode;

    // ── ReadFileTool tests ──

    #[tokio::test]
    async fn read_file_basic() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "line one\nline two\nline three\n").unwrap();

        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"path": "test.txt"})).await.unwrap();

        assert!(result["content"].as_str().unwrap().contains("line one"));
        assert!(result["content"].as_str().unwrap().contains("line two"));
        assert_eq!(result["total_lines"], 3);
        assert_eq!(result["path"], "test.txt");
    }

    #[tokio::test]
    async fn read_file_with_line_range() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("lines.txt");
        let content: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&file_path, content).unwrap();

        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(json!({"path": "lines.txt", "start_line": 5, "end_line": 10}))
            .await
            .unwrap();

        let output = result["content"].as_str().unwrap();
        assert!(output.contains("line 5"));
        assert!(output.contains("line 10"));
        assert!(!output.contains("line 4"));
        assert!(!output.contains("line 11"));
        assert_eq!(result["lines_shown"], "5-10");
    }

    #[tokio::test]
    async fn read_file_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"path": "nonexistent.txt"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_binary_detection() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("binary.bin");
        std::fs::write(&file_path, b"hello\x00world").unwrap();

        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"path": "binary.bin"})).await.unwrap();

        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("Binary file detected"));
    }

    #[tokio::test]
    async fn read_file_missing_path_arg() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err
            .to_string()
            .contains("Missing required parameter 'path'"));
    }

    #[tokio::test]
    async fn read_file_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("file.txt"), "content").unwrap();

        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"path": "sub/file.txt"})).await.unwrap();
        assert!(result["content"].as_str().unwrap().contains("content"));
    }

    #[test]
    fn is_binary_detects_null_bytes() {
        assert!(ReadFileTool::is_binary(b"hello\x00world"));
        assert!(ReadFileTool::is_binary(b"\x00"));
    }

    #[test]
    fn is_binary_passes_text() {
        assert!(!ReadFileTool::is_binary(b"hello world"));
        assert!(!ReadFileTool::is_binary(b"fn main() {}"));
        assert!(!ReadFileTool::is_binary(b""));
    }

    // ── ListDirectoryTool tests ──

    #[tokio::test]
    async fn list_directory_basic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file1.txt"), "a").unwrap();
        std::fs::write(dir.path().join("file2.rs"), "b").unwrap();

        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({})).await.unwrap();

        assert!(result["count"].as_u64().unwrap() >= 2);
        let entries = result["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"file1.txt"));
        assert!(names.contains(&"file2.rs"));
    }

    #[tokio::test]
    async fn list_directory_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("nested.txt"), "x").unwrap();

        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"recursive": true})).await.unwrap();

        let entries = result["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.iter().any(|n| n.contains("nested.txt")));
    }

    #[tokio::test]
    async fn list_directory_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(json!({"path": "nonexistent_dir"}))
            .await
            .unwrap();
        assert!(result["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn list_directory_defaults_to_working_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("root_file.txt"), "x").unwrap();

        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({})).await.unwrap();
        assert_eq!(result["path"], ".");
        assert!(result["count"].as_u64().unwrap() >= 1);
    }

    // ── SearchFilesTool tests ──

    #[tokio::test]
    async fn search_files_glob_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main()").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "pub mod").unwrap();
        std::fs::write(dir.path().join("config.toml"), "[package]").unwrap();

        let tool = SearchFilesTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"pattern": "*.rs"})).await.unwrap();

        assert!(result["count"].as_u64().unwrap() >= 2);
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.iter().any(|m| m.as_str().unwrap().ends_with(".rs")));
    }

    #[tokio::test]
    async fn search_files_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();

        let tool = SearchFilesTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"pattern": "*.xyz"})).await.unwrap();
        assert_eq!(result["count"], 0);
    }

    #[tokio::test]
    async fn search_files_invalid_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let tool = SearchFilesTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"pattern": "[invalid"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn search_files_missing_pattern_arg() {
        let dir = tempfile::tempdir().unwrap();
        let tool = SearchFilesTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    // ── GrepTool tests ──

    #[tokio::test]
    async fn grep_basic_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        let tool = GrepTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"pattern": "println"})).await.unwrap();

        assert!(result["count"].as_u64().unwrap() >= 1);
        let matches = result["matches"].as_array().unwrap();
        assert!(matches[0]["content"].as_str().unwrap().contains("println"));
    }

    #[tokio::test]
    async fn grep_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "Hello World\nhello world\n").unwrap();

        let tool = GrepTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(json!({"pattern": "HELLO", "case_insensitive": true}))
            .await
            .unwrap();

        assert_eq!(result["count"], 2);
    }

    #[tokio::test]
    async fn grep_file_pattern_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("readme.md"), "fn main() {}\n").unwrap();

        let tool = GrepTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(json!({"pattern": "fn main", "file_pattern": "*.rs"}))
            .await
            .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert!(matches
            .iter()
            .all(|m| m["file"].as_str().unwrap().ends_with(".rs")));
        assert!(result["count"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn grep_max_matches_cap() {
        let dir = tempfile::tempdir().unwrap();
        let content: String = (0..200).map(|i| format!("match_line_{i}\n")).collect();
        std::fs::write(dir.path().join("big.txt"), content).unwrap();

        let tool = GrepTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(json!({"pattern": "match_line"}))
            .await
            .unwrap();

        assert_eq!(result["count"], MAX_MATCHES);
        assert_eq!(result["truncated"], true);
    }

    #[tokio::test]
    async fn grep_invalid_regex() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GrepTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"pattern": "[invalid"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn grep_missing_pattern_arg() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GrepTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let tool = GrepTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(json!({"pattern": "zzz_nonexistent"}))
            .await
            .unwrap();
        assert_eq!(result["count"], 0);
        assert_eq!(result["truncated"], false);
    }

    // ── Tool trait method tests ──

    #[test]
    fn tool_names_and_descriptions() {
        let dir = PathBuf::from("/tmp");
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(ReadFileTool::new(dir.clone())),
            Box::new(ListDirectoryTool::new(dir.clone())),
            Box::new(SearchFilesTool::new(dir.clone())),
            Box::new(GrepTool::new(dir)),
        ];
        for tool in &tools {
            assert!(!tool.name().is_empty());
            assert!(!tool.description().is_empty());
            assert_eq!(tool.declaration().name, tool.name());
            assert_eq!(
                tool.available_modes(),
                vec![
                    Mode::Explore,
                    Mode::Plan,
                    Mode::Guided,
                    Mode::Execute,
                    Mode::Auto
                ]
            );
        }
    }
}
