use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::downloader::HostTarget;
use crate::error::{Error, Result};

/// Metadata extracted from the toolchain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainMetadata {
    pub cargo_path: PathBuf,
    pub rustc_path: PathBuf,
    pub version: String,
    pub host: String,
    pub commit_hash: String,
    pub commit_date: String,
    pub sysroot: String,
    pub target: Option<String>,
}

/// Represents the resolved Rust toolchain configured for compilation.
#[derive(Debug, Clone)]
pub struct Toolchain {
    pub cargo_path: PathBuf,
    pub rustc_path: PathBuf,
    /// The default target triple to compile for (for cross-compilation).
    pub target: Option<String>,
}

impl Toolchain {
    /// Attempts to find the toolchain automatically using the system PATH.
    ///
    /// # Errors
    /// Returns an error if `cargo` or `rustc` cannot be found on the system PATH.
    pub fn resolve_system() -> Result<Self> {
        let cargo_path = which::which("cargo")
            .map_err(|_| Error::Toolchain("Cargo not found in system PATH.".to_string()))?;
        let rustc_path = which::which("rustc")
            .map_err(|_| Error::Toolchain("Rustc not found in system PATH.".to_string()))?;

        Ok(Self {
            cargo_path,
            rustc_path,
            target: None,
        })
    }

    /// Explicitly sets the toolchain by providing a custom `sysroot`.
    /// `cargo` and `rustc` are expected to exist inside `<sysroot>/bin/`.
    ///
    /// # Errors
    /// Returns an error if the expected binaries do not exist at the sysroot path.
    pub fn from_sysroot(sysroot: impl Into<PathBuf>) -> Result<Self> {
        let sysroot_path = sysroot.into();
        let bin_dir = sysroot_path.join("bin");

        let cargo_exe = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let rustc_exe = if cfg!(windows) { "rustc.exe" } else { "rustc" };

        let cargo_path = bin_dir.join(cargo_exe);
        let rustc_path = bin_dir.join(rustc_exe);

        if !cargo_path.exists() {
            return Err(Error::Toolchain(format!(
                "Cargo not found in custom sysroot: {}",
                cargo_path.display()
            )));
        }
        if !rustc_path.exists() {
            return Err(Error::Toolchain(format!(
                "Rustc not found in custom sysroot: {}",
                rustc_path.display()
            )));
        }

        Ok(Self {
            cargo_path,
            rustc_path,
            target: None,
        })
    }

    /// Creates a toolchain from explicit binary paths.
    ///
    /// Use this when you know exactly where `cargo` and `rustc` are located.
    /// Both paths are validated to exist on disk at construction time.
    ///
    /// # Errors
    /// Returns an error if either binary path does not exist.
    pub fn from_paths(cargo_path: impl Into<PathBuf>, rustc_path: impl Into<PathBuf>) -> Result<Self> {
        let cargo_path = cargo_path.into();
        let rustc_path = rustc_path.into();

        if !cargo_path.exists() {
            return Err(Error::Toolchain(format!(
                "Cargo binary not found at specified path: {}",
                cargo_path.display()
            )));
        }
        if !rustc_path.exists() {
            return Err(Error::Toolchain(format!(
                "Rustc binary not found at specified path: {}",
                rustc_path.display()
            )));
        }

        Ok(Self {
            cargo_path,
            rustc_path,
            target: None,
        })
    }

    /// Sets the default target for cross-compilation.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Extracts detailed metadata about the current compiler.
    ///
    /// # Errors
    /// Returns an error if `rustc` cannot be executed or its output cannot be parsed.
    pub fn extract_metadata(&self) -> Result<ToolchainMetadata> {
        let output = Command::new(&self.rustc_path)
            .arg("-vV")
            .output()
            .map_err(|e| Error::Toolchain(format!("Failed to run rustc -vV: {e}")))?;

        let text = String::from_utf8_lossy(&output.stdout);
        let mut version = String::new();
        let mut host = String::new();
        let mut commit_hash = String::new();
        let mut commit_date = String::new();

        for line in text.lines() {
            if line.starts_with("rustc ") {
                version = line.trim().to_string();
            } else if let Some(h) = line.strip_prefix("host: ") {
                host = h.trim().to_string();
            } else if let Some(h) = line.strip_prefix("commit-hash: ") {
                commit_hash = h.trim().to_string();
            } else if let Some(d) = line.strip_prefix("commit-date: ") {
                commit_date = d.trim().to_string();
            }
        }

        let sysroot_out = Command::new(&self.rustc_path)
            .arg("--print")
            .arg("sysroot")
            .output()
            .map_err(|e| Error::Toolchain(format!("Failed to query sysroot: {e}")))?;
        let sysroot = String::from_utf8_lossy(&sysroot_out.stdout)
            .trim()
            .to_string();

        Ok(ToolchainMetadata {
            cargo_path: self.cargo_path.clone(),
            rustc_path: self.rustc_path.clone(),
            version,
            host,
            commit_hash,
            commit_date,
            sysroot,
            target: self.target.clone(),
        })
    }

    /// Attempts to load the toolchain from the standard cache directory for a given version.
    /// Uses `~/.mr/toolchains/` by default. If the toolchain doesn't exist, it will NOT download it.
    /// Use `ToolchainDownloader` to download it first.
    ///
    /// # Errors
    /// Returns an error if the toolchain is not found in the cache or the sysroot is invalid.
    pub fn from_cache(version: impl AsRef<str>) -> Result<Self> {
        let approot = crate::default_approot();
        let cache_dir = approot.join("toolchains");
        Self::from_cache_dir(version, cache_dir)
    }

    /// Attempts to load the toolchain from a specific cache directory.
    ///
    /// # Errors
    /// Returns an error if the toolchain is not found in the cache directory.
    pub fn from_cache_dir(version: impl AsRef<str>, cache_dir: impl Into<PathBuf>) -> Result<Self> {
        let target = HostTarget::current()?;
        let toolchain_name = format!("{}-{}", version.as_ref(), target);
        let sysroot = cache_dir.into().join(toolchain_name);

        if !sysroot.exists() {
            return Err(Error::Toolchain(format!(
                "Toolchain {} not found in cache {}",
                version.as_ref(),
                sysroot.display()
            )));
        }

        Self::from_sysroot(sysroot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_system_toolchain() {
        let tc = Toolchain::resolve_system().expect("Cargo/rustc must be in PATH for tests");
        assert!(tc.cargo_path.exists());
        assert!(tc.rustc_path.exists());
    }

    #[test]
    fn test_extract_metadata() {
        let tc = Toolchain::resolve_system().unwrap();
        let meta = tc.extract_metadata().unwrap();
        assert!(!meta.host.is_empty());
        assert!(!meta.sysroot.is_empty());
        assert!(meta.version.starts_with("rustc"));
    }
}
