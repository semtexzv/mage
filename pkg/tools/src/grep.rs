//! Grep tool — search file contents with regex.

use std::rc::Rc;

use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use walkdir::WalkDir;

use mage_core::module::Module;
use mage_core::tool::{ToolContext, ToolDef, ToolHandler};
use mage_core::types::ToolResult;

/// Directories to skip.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    "dist",
    "build",
    ".next",
    ".nuxt",
];

/// Binary file extensions to skip.
const BINARY_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp", "mp3", "mp4",
    "avi", "mov", "pdf", "zip", "tar", "gz", "bz2", "xz", "7z", "rar",
    "wasm", "o", "so", "dylib", "dll", "exe", "class", "pyc", "woff",
    "woff2", "ttf", "eot",
];

pub struct GrepModule;

#[async_trait(?Send)]
impl Module for GrepModule {
    fn name(&self) -> &str { "grep" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            schema: llm::Tool {
                name: "Grep".into(),
                description: "Search file contents using regex. Returns matching file paths \
                    by default, or matching lines with context.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Regex pattern to search for"
                        },
                        "path": {
                            "type": "string",
                            "description": "File or directory to search (default: current directory)"
                        },
                        "glob": {
                            "type": "string",
                            "description": "Glob pattern to filter files (e.g. \"*.rs\")"
                        },
                        "output_mode": {
                            "type": "string",
                            "enum": ["content", "files_with_matches", "count"],
                            "description": "Output mode (default: files_with_matches)"
                        },
                        "case_insensitive": {
                            "type": "boolean",
                            "description": "Case insensitive search (default: false)"
                        },
                        "context": {
                            "type": "integer",
                            "description": "Lines of context around each match"
                        },
                        "head_limit": {
                            "type": "integer",
                            "description": "Limit number of results (default: 250)"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
            handler: Rc::new(GrepHandler),
        }]
    }
}

struct GrepHandler;

#[async_trait(?Send)]
impl ToolHandler for GrepHandler {
    async fn execute(&self, args: serde_json::Value, _ctx: ToolContext) -> ToolResult {
        let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p.to_owned(),
            None => return ToolResult::failure("pattern is required"),
        };

        let search_path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_owned();

        let output_mode = args
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches")
            .to_owned();

        let case_insensitive = args
            .get("case_insensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let context_lines = args
            .get("context")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let head_limit = args
            .get("head_limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(250) as usize;

        let glob_filter = args
            .get("glob")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        // Build regex
        let regex_pattern = if case_insensitive {
            format!("(?i){pattern}")
        } else {
            pattern.clone()
        };
        let re = match Regex::new(&regex_pattern) {
            Ok(r) => r,
            Err(e) => return ToolResult::failure(format!("Invalid regex: {e}")),
        };

        // Build glob matcher if specified
        let glob_matcher = match &glob_filter {
            Some(g) => match globset::GlobBuilder::new(g).build() {
                Ok(gb) => Some(gb.compile_matcher()),
                Err(e) => return ToolResult::failure(format!("Invalid glob filter: {e}")),
            },
            None => None,
        };

        // Run search (blocking)
        let result = tokio::task::spawn_blocking(move || {
            let path = std::path::Path::new(&search_path);

            // Single file
            if path.is_file() {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
                };
                return Ok(search_file(
                    &path.display().to_string(),
                    &content,
                    &re,
                    &output_mode,
                    context_lines,
                    head_limit,
                ));
            }

            // Directory
            let mut results = Vec::new();
            let mut count = 0;

            for entry in WalkDir::new(path)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    if e.file_type().is_dir() {
                        let name = e.file_name().to_string_lossy();
                        return !SKIP_DIRS.contains(&&*name);
                    }
                    true
                })
            {
                if count >= head_limit {
                    break;
                }

                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                if !entry.file_type().is_file() {
                    continue;
                }

                // Skip binary files by extension
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                    if BINARY_EXTS.contains(&ext.to_lowercase().as_str()) {
                        continue;
                    }
                }

                // Apply glob filter
                if let Some(ref matcher) = glob_matcher {
                    let name = entry.file_name().to_string_lossy();
                    if !matcher.is_match(&*name) {
                        // Also try matching the relative path
                        let rel = entry.path().strip_prefix(path).unwrap_or(entry.path());
                        if !matcher.is_match(rel) {
                            continue;
                        }
                    }
                }

                let content = match std::fs::read_to_string(entry.path()) {
                    Ok(c) => c,
                    Err(_) => continue, // skip unreadable files
                };

                if !re.is_match(&content) {
                    continue;
                }

                let file_path = entry.path().display().to_string();

                match output_mode.as_str() {
                    "files_with_matches" => {
                        results.push(file_path);
                        count += 1;
                    }
                    "count" => {
                        let n = re.find_iter(&content).count();
                        results.push(format!("{file_path}:{n}"));
                        count += 1;
                    }
                    "content" => {
                        let file_results = format_matches(
                            &file_path,
                            &content,
                            &re,
                            context_lines,
                        );
                        let lines: Vec<&str> = file_results.lines().collect();
                        let remaining = head_limit.saturating_sub(count);
                        let take = lines.len().min(remaining);
                        for line in &lines[..take] {
                            results.push(line.to_string());
                            count += 1;
                        }
                    }
                    _ => {
                        results.push(file_path);
                        count += 1;
                    }
                }
            }

            Ok(results)
        })
        .await;

        let results = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return ToolResult::failure(e),
            Err(e) => return ToolResult::failure(format!("Search failed: {e}")),
        };

        if results.is_empty() {
            return ToolResult::success("No matches found.");
        }

        let output = results.join("\n");
        ToolResult::success(output)
    }

    fn is_concurrent_safe(&self, _args: &serde_json::Value) -> bool {
        true
    }
}

/// Search a single file and return formatted results.
fn search_file(
    path: &str,
    content: &str,
    re: &Regex,
    mode: &str,
    context_lines: usize,
    _limit: usize,
) -> Vec<String> {
    match mode {
        "files_with_matches" => {
            if re.is_match(content) {
                vec![path.to_string()]
            } else {
                vec![]
            }
        }
        "count" => {
            let n = re.find_iter(content).count();
            if n > 0 {
                vec![format!("{path}:{n}")]
            } else {
                vec![]
            }
        }
        "content" => {
            let formatted = format_matches(path, content, re, context_lines);
            formatted.lines().map(|l| l.to_string()).collect()
        }
        _ => {
            if re.is_match(content) {
                vec![path.to_string()]
            } else {
                vec![]
            }
        }
    }
}

/// Format matching lines with optional context.
fn format_matches(
    path: &str,
    content: &str,
    re: &Regex,
    context: usize,
) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut match_lines: Vec<usize> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if re.is_match(line) {
            match_lines.push(i);
        }
    }

    if match_lines.is_empty() {
        return String::new();
    }

    // Build set of lines to show (matches + context).
    let mut show: Vec<bool> = vec![false; lines.len()];
    for &m in &match_lines {
        let start = m.saturating_sub(context);
        let end = (m + context + 1).min(lines.len());
        for i in start..end {
            show[i] = true;
        }
    }

    let mut output = String::new();
    let mut in_group = false;

    for (i, line) in lines.iter().enumerate() {
        if show[i] {
            if !in_group && !output.is_empty() {
                output.push_str("--\n");
            }
            in_group = true;
            output.push_str(&format!("{path}:{}:{line}\n", i + 1));
        } else {
            in_group = false;
        }
    }

    output
}
