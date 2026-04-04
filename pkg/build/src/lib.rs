use std::path::PathBuf;

/// Returns the default `.mage` application root directory for the current user.
#[must_use]
pub fn default_approot() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mage")
}

pub mod bundle;
pub mod compile;
pub mod deps;
pub mod downloader;
pub mod error;
pub mod module;
pub mod template;
pub mod toolchain;
