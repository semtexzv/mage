use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Automatically detects the current host's target triple.
pub struct HostTarget;

impl HostTarget {
    /// Returns the current host's target triple string.
    ///
    /// # Errors
    /// Returns an error if the current architecture/OS combination is unsupported.
    pub fn current() -> Result<&'static str> {
        let arch = std::env::consts::ARCH;
        let os = std::env::consts::OS;

        match (arch, os) {
            ("x86_64", "linux") => Ok("x86_64-unknown-linux-gnu"),
            ("aarch64", "linux") => Ok("aarch64-unknown-linux-gnu"),
            ("x86_64", "macos") => Ok("x86_64-apple-darwin"),
            ("aarch64", "macos") => Ok("aarch64-apple-darwin"),
            ("x86_64", "windows") => Ok("x86_64-pc-windows-msvc"),
            _ => Err(Error::Toolchain(format!(
                "Unsupported host architecture/OS: {arch}-{os}"
            ))),
        }
    }
}

/// Downloads and extracts Rust toolchain archives for a specific version and host target.
///
/// Supports multiple HTTP backends selected via Cargo features:
/// - `tokio` — async download using `reqwest` (preferred for async runtimes)
/// - `actix` — async download using `awc` (for actix-web applications)
/// - `ureq` — synchronous download wrapped in an `async fn` for API compatibility
///
/// Only one backend is compiled in; feature flags are mutually exclusive by priority
/// (`tokio` > `actix` > `ureq`).
pub struct ToolchainDownloader {
    version: String,
    target: String,
}

impl ToolchainDownloader {
    /// Creates a new downloader for the given Rust version.
    ///
    /// # Errors
    /// Returns an error if the host target cannot be detected.
    pub fn new(version: impl Into<String>) -> Result<Self> {
        let target = HostTarget::current()?;
        Ok(Self {
            version: version.into(),
            target: target.to_string(),
        })
    }

    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = target.into();
        self
    }

    /// Downloads and extracts the toolchain into the given cache directory.
    ///
    /// # Errors
    /// Returns an error if the download or extraction fails.
    pub async fn download_and_extract(&self, cache_dir: &Path) -> Result<PathBuf> {
        let toolchain_name = format!("{}-{}", self.version, self.target);
        let dest_dir = cache_dir.join(&toolchain_name);

        if dest_dir.exists() && dest_dir.join("bin/cargo").exists() {
            return Ok(dest_dir);
        }

        let url = format!(
            "https://static.rust-lang.org/dist/rust-{}-{}.tar.xz",
            self.version, self.target
        );

        let dest_dir_clone = dest_dir.clone();
        let target_clone = self.target.clone();

        let extract_closure = move |stream: Box<dyn Read + Send>| -> Result<()> {
            let decoder = xz2::read::XzDecoder::new(stream);
            let mut archive = tar::Archive::new(decoder);

            let rust_std_component = format!("rust-std-{target_clone}");
            let components_to_keep = ["cargo", "rustc", rust_std_component.as_str()];

            for entry_res in archive
                .entries()
                .map_err(|e| Error::Extraction(e.to_string()))?
            {
                let mut file = entry_res.map_err(|e| Error::Extraction(e.to_string()))?;
                let path = file
                    .path()
                    .map_err(|e| Error::Extraction(e.to_string()))?
                    .to_path_buf();
                let comps: Vec<_> = path
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();

                if comps.len() > 2 {
                    let comp = &comps[1];
                    if components_to_keep.contains(&comp.as_str()) {
                        let mut target_path = PathBuf::new();
                        for c in comps.iter().skip(2) {
                            target_path.push(c);
                        }

                        let dest_file = dest_dir_clone.join(target_path);
                        if let Some(parent) = dest_file.parent() {
                            fs::create_dir_all(parent).map_err(Error::Io)?;
                        }

                        file.unpack(&dest_file).map_err(|e| {
                            Error::Extraction(format!(
                                "Failed to unpack to {}: {e}",
                                dest_file.display(),
                            ))
                        })?;
                    }
                }
            }

            Ok(())
        };

        self.execute_download(url, extract_closure).await?;

        Ok(dest_dir)
    }
}

// --- TOKIO/REQWEST IMPLEMENTATION ---
#[cfg(feature = "tokio")]
impl ToolchainDownloader {
    async fn execute_download<F>(&self, url: String, extract_fn: F) -> Result<()>
    where
        F: FnOnce(Box<dyn Read + Send>) -> Result<()> + Send + 'static,
    {
        use tokio::io::AsyncWriteExt;

        let mut response = reqwest::get(&url)
            .await
            .map_err(|e| Error::Network(format!("Failed to download toolchain from {url}: {e}")))?;

        if !response.status().is_success() {
            return Err(Error::Network(format!(
                "Failed to download toolchain, server returned status: {}",
                response.status()
            )));
        }

        let temp_file = tempfile::NamedTempFile::new().map_err(Error::Io)?;
        let temp_path = temp_file.path().to_path_buf();

        let mut file = tokio::fs::File::create(&temp_path)
            .await
            .map_err(Error::Io)?;

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::Network(e.to_string()))?
        {
            file.write_all(&chunk).await.map_err(Error::Io)?;
        }

        file.sync_all().await.map_err(Error::Io)?;
        drop(file);

        tokio::task::spawn_blocking(move || {
            let std_file = fs::File::open(&temp_path).map_err(Error::Io)?;
            let reader = Box::new(std::io::BufReader::new(std_file));
            extract_fn(reader)?;
            drop(temp_file); // Ensure temp file lives until extraction is complete
            Ok::<(), Error>(())
        })
        .await
        .map_err(|e| Error::Extraction(format!("Blocking task failed: {e}")))??;

        Ok(())
    }
}

// --- ACTIX IMPLEMENTATION ---
#[cfg(all(feature = "actix", not(feature = "tokio")))]
impl ToolchainDownloader {
    async fn execute_download<F>(&self, url: String, extract_fn: F) -> Result<()>
    where
        F: FnOnce(Box<dyn Read + Send>) -> Result<()> + Send + 'static,
    {
        use std::future::poll_fn;
        use std::io::Write;
        use std::pin::pin;
        use std::task::Poll;

        let response =
            awc::Client::default().get(&url).send().await.map_err(|e| {
                Error::Network(format!("Failed to download toolchain from {url}: {e}"))
            })?;

        if !response.status().is_success() {
            return Err(Error::Network(format!(
                "Failed to download toolchain, server returned status: {}",
                response.status()
            )));
        }

        let temp_file = tempfile::NamedTempFile::new().map_err(Error::Io)?;
        let temp_path = temp_file.path().to_path_buf();

        let mut file = fs::File::create(&temp_path).map_err(Error::Io)?;

        // Poll the response body stream chunk-by-chunk using futures_core::Stream
        // directly, avoiding any tokio or futures_util imports. Each chunk is small
        // (typically 8-64KB), so the synchronous write per iteration is negligible.
        let mut stream = pin!(response);
        while let Some(chunk_res) =
            poll_fn(
                |cx| match futures_core::Stream::poll_next(stream.as_mut(), cx) {
                    Poll::Ready(item) => Poll::Ready(item),
                    Poll::Pending => Poll::Pending,
                },
            )
            .await
        {
            let chunk = chunk_res.map_err(|e| Error::Network(e.to_string()))?;
            file.write_all(&chunk).map_err(Error::Io)?;
        }

        file.sync_all().map_err(Error::Io)?;
        drop(file);

        actix_rt::task::spawn_blocking(move || {
            let std_file = fs::File::open(&temp_path).map_err(Error::Io)?;
            let reader = Box::new(std::io::BufReader::new(std_file));
            extract_fn(reader)?;
            drop(temp_file);
            Ok::<(), Error>(())
        })
        .await
        .map_err(|e| Error::Extraction(format!("Blocking task failed: {e}")))??;

        Ok(())
    }
}

// --- UREQ IMPLEMENTATION ---
/// Synchronous fallback using `ureq`. The `async fn` signature is retained for API
/// compatibility with the `tokio` and `actix` backends; no `.await` points exist in
/// this implementation.
#[cfg(all(feature = "ureq", not(feature = "tokio"), not(feature = "actix")))]
impl ToolchainDownloader {
    /// Downloads a toolchain archive using the synchronous `ureq` HTTP client.
    ///
    /// This method is declared `async` **only** for API compatibility with the `tokio`
    /// and `actix` backends — it contains no `.await` points and is fully synchronous.
    /// It **will block the calling thread** for the duration of the HTTP request and
    /// response body read.
    ///
    /// If called from within a `tokio` or `actix` async runtime (e.g. during tests or
    /// mixed-backend setups), this will block the runtime thread. Use
    /// `spawn_blocking` to move the call off the async executor if needed.
    ///
    /// This is intentional: the `ureq` crate is synchronous by design and serves as a
    /// lightweight fallback when no async runtime is available.
    async fn execute_download<F>(&self, url: String, extract_fn: F) -> Result<()>
    where
        F: FnOnce(Box<dyn Read + Send>) -> Result<()> + Send + 'static,
    {
        let response = ureq::get(&url)
            .call()
            .map_err(|e| Error::Network(format!("Failed to download toolchain from {url}: {e}")))?;

        let reader = response.into_reader();
        extract_fn(Box::new(reader))
    }
}

// --- FALLBACK ---
#[cfg(not(any(feature = "tokio", feature = "ureq", feature = "actix")))]
impl ToolchainDownloader {
    async fn execute_download<F>(&self, _url: String, _extract_fn: F) -> Result<()>
    where
        F: FnOnce(Box<dyn Read + Send>) -> Result<()> + Send + 'static,
    {
        Err(Error::Network(
            "No download backend enabled. Compile with 'tokio', 'actix', or 'ureq' feature."
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_target() {
        let target = HostTarget::current().unwrap();
        assert!(!target.is_empty());
        assert!(
            target.contains("unknown-linux")
                || target.contains("apple")
                || target.contains("windows")
        );
    }
}
