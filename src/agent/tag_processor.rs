use std::path::Path;
use std::sync::LazyLock;
use regex::Regex;
use base64::Engine;
use crate::error::Result;
use crate::gemini::types::Part;

/// Maximum image size: 20 MB (Gemini's inline data limit).
const MAX_IMAGE_SIZE: usize = 20 * 1024 * 1024;

static TAG_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@([a-zA-Z0-9_\-\./\\]+)").unwrap());

/// Intercepts a user prompt string and parses it for `@path/to/file` tags.
/// 
/// Replaces the tags with `[Attached File: path]` or `[Attached Image: path]`
/// and returns a `Vec<Part>` containing the modified text prompt followed by
/// the actual file data parts.
/// 
/// - Text files are capped at 1MB and inlined as `Part::Text`.
/// - Images are base64 encoded and attached as `Part::InlineData`.
pub async fn process_tags(input: &str, working_dir: &Path) -> Result<Vec<Part>> {
    let mut parts = Vec::new();
    
    // We process the matches in reverse order so that we can safely replace
    // the text in the original string without invalidating the indices of earlier matches.
    let mut modified_input = input.to_string();
    let matches: Vec<_> = TAG_REGEX.captures_iter(input).collect();
    
    for cap in matches.iter().rev() {
        let full_match = cap.get(0).unwrap();
        let path_str = cap.get(1).unwrap().as_str();
        
        let path = if Path::new(path_str).is_absolute() {
            Path::new(path_str).to_path_buf()
        } else {
            working_dir.join(path_str)
        };
        
        if path.exists() && path.is_file() {
            // Check if it's an image based on extension
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ["png", "jpg", "jpeg", "webp", "gif"].iter().any(|&e| ext.eq_ignore_ascii_case(e)) {
                // Image handling: Direct InlineData injection
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        if bytes.len() > MAX_IMAGE_SIZE {
                            tracing::warn!("Image {} exceeds 20MB limit", path.display());
                            let replacement = format!("[Failed to read image: {} (Exceeds 20MB limit)]", path_str);
                            modified_input.replace_range(full_match.start()..full_match.end(), &replacement);
                            continue;
                        }
                        
                        let mime_type = match ext.to_lowercase().as_str() {
                            "png" => "image/png",
                            "jpg" | "jpeg" => "image/jpeg",
                            "webp" => "image/webp",
                            "gif" => "image/gif",
                            _ => "image/png", // fallback
                        };
                        
                        let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        
                        let replacement = format!("[Attached Image: {}]", path_str);
                        modified_input.replace_range(full_match.start()..full_match.end(), &replacement);
                        
                        parts.push(Part::InlineData {
                            mime_type: mime_type.to_string(),
                            data: base64_data,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to read image {}: {}", path.display(), e);
                        let replacement = format!("[Failed to read image: {}]", path_str);
                        modified_input.replace_range(full_match.start()..full_match.end(), &replacement);
                    }
                }
            } else {
                // Text file handling: Read up to 1MB and inline as text
                if let Ok(metadata) = std::fs::metadata(&path) {
                    let mut text_part = String::new();
                    text_part.push_str(&format!("\n--- {} ---\n", path_str));
                    
                    let max_size = 1024 * 1024; // 1MB
                    if metadata.len() > max_size {
                        let mut buf = vec![0; max_size as usize];
                        use std::io::Read;
                        if let Ok(mut f) = std::fs::File::open(&path) {
                            if f.read_exact(&mut buf).is_ok() {
                                text_part.push_str(&String::from_utf8_lossy(&buf));
                                text_part.push_str("\n\n[TRUNCATED: File exceeds 1MB limit]\n");
                            } else {
                                let replacement = format!("[Failed to read file: {}]", path_str);
                                modified_input.replace_range(full_match.start()..full_match.end(), &replacement);
                                continue;
                            }
                        } else {
                            let replacement = format!("[Failed to read file: {}]", path_str);
                            modified_input.replace_range(full_match.start()..full_match.end(), &replacement);
                            continue;
                        }
                    } else if let Ok(contents) = std::fs::read_to_string(&path) {
                        text_part.push_str(&contents);
                        if !contents.ends_with('\n') {
                            text_part.push('\n');
                        }
                    } else {
                        tracing::warn!("Failed to read text file {} (might be binary)", path.display());
                        let replacement = format!("[Failed to read file: {} (might be binary)]", path_str);
                        modified_input.replace_range(full_match.start()..full_match.end(), &replacement);
                        continue; // Skip appending if read fails
                    }
                    
                    text_part.push_str(&format!("--- end {} ---\n", path_str));
                    
                    let replacement = format!("[Attached File: {}]", path_str);
                    modified_input.replace_range(full_match.start()..full_match.end(), &replacement);
                    
                    parts.push(Part::Text(text_part));
                } else {
                    let replacement = format!("[Failed to access file: {}]", path_str);
                    modified_input.replace_range(full_match.start()..full_match.end(), &replacement);
                }
            }
        } else {
            tracing::warn!("Tag path not found or not a file: {}", path.display());
            // Leave the raw @path in the text if it doesn't resolve
        }
    }
    
    // The parts were collected in reverse order (because we iterated backwards over matches)
    parts.reverse();
    
    // Insert the modified text as the very first part
    let mut final_parts = vec![Part::Text(modified_input)];
    final_parts.extend(parts);
    
    Ok(final_parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_process_tags_text_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"Hello from test.txt").unwrap();

        let input = "Check out @test.txt";
        let parts = process_tags(input, dir.path()).await.unwrap();

        assert_eq!(parts.len(), 2);
        
        // Check modified input
        if let Part::Text(t) = &parts[0] {
            assert_eq!(t, "Check out [Attached File: test.txt]");
        } else {
            panic!("Expected Text part for modified input");
        }

        // Check file content
        if let Part::Text(t) = &parts[1] {
            assert!(t.contains("Hello from test.txt"));
        } else {
            panic!("Expected Text part for file content");
        }
    }

    #[tokio::test]
    async fn test_process_tags_image_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.png");
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"fake image data").unwrap();

        let input = "Look at @test.png image";
        let parts = process_tags(input, dir.path()).await.unwrap();

        assert_eq!(parts.len(), 2);
        
        // Check modified input
        if let Part::Text(t) = &parts[0] {
            assert_eq!(t, "Look at [Attached Image: test.png] image");
        } else {
            panic!("Expected Text part for modified input");
        }

        // Check image content
        if let Part::InlineData { mime_type, data } = &parts[1] {
            assert_eq!(mime_type, "image/png");
            let expected_b64 = base64::engine::general_purpose::STANDARD.encode(b"fake image data");
            assert_eq!(data, &expected_b64);
        } else {
            panic!("Expected InlineData part for image content");
        }
    }

    #[tokio::test]
    async fn test_process_tags_file_not_found() {
        let dir = tempdir().unwrap();
        let input = "Missing @does_not_exist.txt file";
        let parts = process_tags(input, dir.path()).await.unwrap();

        assert_eq!(parts.len(), 1);
        if let Part::Text(t) = &parts[0] {
            assert_eq!(t, "Missing @does_not_exist.txt file");
        } else {
            panic!("Expected Text part");
        }
    }

    #[tokio::test]
    async fn test_process_tags_truncation() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("large.txt");
        let mut f = File::create(&file_path).unwrap();
        
        let chunk = vec![b'A'; 1024];
        for _ in 0..1025 {
            f.write_all(&chunk).unwrap();
        }
        f.flush().unwrap();

        let input = "Read @large.txt";
        let parts = process_tags(input, dir.path()).await.unwrap();

        assert_eq!(parts.len(), 2);
        if let Part::Text(t) = &parts[1] {
            assert!(t.contains("[TRUNCATED: File exceeds 1MB limit]"));
        } else {
            panic!("Expected Text part for file content");
        }
    }
}