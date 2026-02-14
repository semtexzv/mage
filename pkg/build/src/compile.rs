use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::toolchain::ToolchainMetadata;

/// A diagnostic message from the Rust compiler.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Diagnostic {
    pub message: String,
    pub code: Option<DiagnosticCode>,
    pub level: String,
    pub rendered: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiagnosticCode {
    pub code: String,
}

/// Intermediate compilation diagnostics collected during cargo execution.
pub struct CompilationDiagnostics {
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    pub artifact_path: Option<PathBuf>,
    pub cargo_stderr: String,
}

/// Structured outcome of a compilation.
#[derive(Debug, Serialize, Deserialize)]
pub struct CompilationResult {
    pub success: bool,
    pub executable_path: Option<PathBuf>,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    /// Raw unformatted stderr output from Cargo (progress, crate downloads, cargo-level errors).
    pub cargo_stderr: String,
    /// Information about the toolchain used to compile this binary.
    pub toolchain_metadata: ToolchainMetadata,
    /// Source files this build depended on, extracted from cargo's `.d` dep-info file.
    #[serde(default)]
    pub dep_info_files: Vec<PathBuf>,
}

impl CompilationResult {
    /// Formats all errors into a single string mimicking rustc output.
    #[must_use]
    pub fn format_errors(&self) -> String {
        self.errors
            .iter()
            .map(|e| e.rendered.clone().unwrap_or_else(|| e.message.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Formats all warnings into a single string.
    #[must_use]
    pub fn format_warnings(&self) -> String {
        self.warnings
            .iter()
            .map(|w| w.rendered.clone().unwrap_or_else(|| w.message.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl fmt::Display for CompilationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.success {
            writeln!(f, "Compilation successful.")?;
            writeln!(
                f,
                "Toolchain: {} ({}) for {}",
                self.toolchain_metadata.version,
                self.toolchain_metadata
                    .commit_hash
                    .chars()
                    .take(7)
                    .collect::<String>(),
                self.toolchain_metadata
                    .target
                    .as_deref()
                    .unwrap_or(&self.toolchain_metadata.host)
            )?;
            writeln!(f, "Sysroot: {}", self.toolchain_metadata.sysroot)?;
        } else {
            writeln!(f, "Compilation failed.")?;
        }

        let warnings = self.format_warnings();
        if !warnings.is_empty() {
            write!(f, "\nWarnings:\n{warnings}")?;
        }

        let errors = self.format_errors();
        if !errors.is_empty() {
            write!(f, "\nErrors:\n{errors}")?;
        }

        Ok(())
    }
}

/// Parses a cargo `.d` (dep-info) file and returns the list of dependency source paths.
///
/// The format is: `/path/to/output: /path/to/src1.rs /path/to/src2.rs ...`
/// Paths with spaces are backslash-escaped. Lines may end with `\` for continuation.
///
/// # Errors
/// Returns an error if the file cannot be read.
pub fn parse_dep_info(path: &Path) -> crate::error::Result<Vec<PathBuf>> {
    let content = fs::read_to_string(path)?;

    // Find the first `:` — everything after it is dependency paths.
    let rhs = match content.find(':') {
        Some(idx) => &content[idx + 1..],
        None => return Ok(Vec::new()),
    };

    // Join continuation lines (trailing backslash + newline) into one logical line,
    // then parse space-separated paths with backslash-escaped spaces.
    let joined = rhs.replace("\\\n", " ").replace("\\\r\n", " ");

    let mut paths = Vec::new();
    let mut current = String::new();
    let mut chars = joined.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                // Backslash-escaped character (typically a space).
                if let Some(&next) = chars.peek() {
                    current.push(next);
                    chars.next();
                }
            }
            ' ' | '\t' | '\n' | '\r' => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    paths.push(PathBuf::from(trimmed));
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        paths.push(PathBuf::from(trimmed));
    }

    Ok(paths)
}
