//! Credential persistence — load/save OAuth tokens to disk.
//!
//! Storage location: `~/.mage/auth.json`
//! Format: JSON object keyed by provider id.
//! File permissions: 0o600 (owner read/write only).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// On-disk credential entry. Currently only OAuth is supported.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Credential {
    #[serde(rename = "oauth")]
    OAuth {
        refresh_token: String,
        access_token: String,
        expires_at_ms: u64,
    },
}

/// All stored credentials, keyed by provider id (e.g. "anthropic").
pub type CredentialStore = HashMap<String, Credential>;

/// Default credential file path: `~/.mage/auth.json`
pub fn default_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".mage").join("auth.json"))
}

/// Load credentials from disk. Returns empty map if file doesn't exist or is invalid.
pub fn load(path: &std::path::Path) -> CredentialStore {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return CredentialStore::new(),
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Save credentials to disk. Creates parent directories if needed.
/// Sets file permissions to 0o600 on Unix.
pub fn save(path: &std::path::Path, store: &CredentialStore) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, &json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Save a single provider's OAuth credential. Loads existing store, merges, saves.
pub fn save_oauth(
    path: &std::path::Path,
    provider: &str,
    refresh_token: &str,
    access_token: &str,
    expires_at_ms: u64,
) -> std::io::Result<()> {
    let mut store = load(path);
    store.insert(
        provider.to_string(),
        Credential::OAuth {
            refresh_token: refresh_token.to_string(),
            access_token: access_token.to_string(),
            expires_at_ms,
        },
    );
    save(path, &store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    fn temp_path() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("mage_cred_test_{}_{}", std::process::id(), n));
        p.push("auth.json");
        p
    }

    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let store = load(std::path::Path::new("/tmp/nonexistent_mage_test_file.json"));
        assert!(store.is_empty());
    }

    #[test]
    fn load_corrupt_json_returns_empty() {
        let path = std::env::temp_dir().join(format!("mage_corrupt_{}.json", std::process::id()));
        std::fs::write(&path, "not valid json!!!").unwrap();
        let store = load(&path);
        assert!(store.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_empty_file_returns_empty() {
        let path = std::env::temp_dir().join(format!("mage_empty_{}.json", std::process::id()));
        std::fs::write(&path, "").unwrap();
        let store = load(&path);
        assert!(store.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn round_trip_save_load() {
        let path = temp_path();
        let mut store = CredentialStore::new();
        store.insert(
            "anthropic".to_string(),
            Credential::OAuth {
                refresh_token: "rt_abc".to_string(),
                access_token: "at_xyz".to_string(),
                expires_at_ms: 1700000000000,
            },
        );

        save(&path, &store).unwrap();
        let loaded = load(&path);

        assert_eq!(loaded.len(), 1);
        match &loaded["anthropic"] {
            Credential::OAuth {
                refresh_token,
                access_token,
                expires_at_ms,
            } => {
                assert_eq!(refresh_token, "rt_abc");
                assert_eq!(access_token, "at_xyz");
                assert_eq!(*expires_at_ms, 1700000000000);
            }
        }

        cleanup(&path);
    }

    #[test]
    fn save_oauth_merges_into_existing() {
        let path = temp_path();

        save_oauth(&path, "anthropic", "rt1", "at1", 100).unwrap();
        save_oauth(&path, "other_provider", "rt2", "at2", 200).unwrap();

        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains_key("anthropic"));
        assert!(loaded.contains_key("other_provider"));

        // Overwrite anthropic
        save_oauth(&path, "anthropic", "rt3", "at3", 300).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        match &loaded["anthropic"] {
            Credential::OAuth {
                refresh_token,
                expires_at_ms,
                ..
            } => {
                assert_eq!(refresh_token, "rt3");
                assert_eq!(*expires_at_ms, 300);
            }
        }

        cleanup(&path);
    }

    #[test]
    fn credential_serializes_with_type_tag() {
        let cred = Credential::OAuth {
            refresh_token: "r".to_string(),
            access_token: "a".to_string(),
            expires_at_ms: 42,
        };
        let json = serde_json::to_string(&cred).unwrap();
        assert!(json.contains(r#""type":"oauth""#));
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_permissions_600() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path();
        let store = CredentialStore::new();
        save(&path, &store).unwrap();

        let perms = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        cleanup(&path);
    }

    #[test]
    fn default_path_returns_some() {
        // HOME is set in most test environments
        if std::env::var("HOME").is_ok() {
            let p = default_path();
            assert!(p.is_some());
            let p = p.unwrap();
            assert!(p.ends_with(".mage/auth.json"));
        }
    }
}
