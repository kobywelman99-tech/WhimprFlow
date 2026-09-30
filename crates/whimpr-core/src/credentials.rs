//! Local, restricted-permission file storage for cloud cleanup API keys.
//!
//! This used to go through the OS keychain (macOS Keychain Services / the
//! `keyring` crate), but on an unsigned local dev build (`cargo tauri build`
//! without a real Developer ID signature — the case for this app today),
//! Keychain writes were observed to report success and even read back
//! correctly *within the same process call*, yet never actually persist:
//! neither a fresh process restart nor the `security` CLI could find the
//! item afterward. That's consistent with macOS Keychain Services silently
//! failing to durably commit writes from processes without a stable code
//! signature/Team ID, rather than a bug in this app's read/write logic.
//!
//! Rather than requiring a paid Apple Developer ID certificate just to store
//! one API key, keys live in a small JSON file under the app's own support
//! directory, permissioned so only the current user can read it (`0600` on
//! Unix). This is a standard, reasonable tradeoff for a personal,
//! single-user, locally-built tool — not OS-grade secret storage, but
//! reliable, which the keychain path was not.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialStore {
    #[serde(default)]
    pub openai_api_key: Option<String>,
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
}

impl CredentialStore {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist to `path`, restricting it to the current user (`0600`) on
    /// Unix so the plaintext key isn't world/group readable. Best-effort on
    /// other platforms (Windows ACLs already default to the owning user).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn get(&self, account: &str) -> Option<String> {
        match account {
            "openai_api_key" => self.openai_api_key.clone(),
            "anthropic_api_key" => self.anthropic_api_key.clone(),
            _ => None,
        }
        .filter(|k| !k.trim().is_empty())
    }

    /// Set (or clear, when `key` is empty) an account's key and persist.
    pub fn set(&mut self, account: &str, key: &str) {
        let key = key.trim();
        let value = if key.is_empty() { None } else { Some(key.to_string()) };
        match account {
            "openai_api_key" => self.openai_api_key = value,
            "anthropic_api_key" => self.anthropic_api_key = value,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let mut s = CredentialStore::default();
        s.set("anthropic_api_key", "sk-ant-test");
        let json = serde_json::to_string(&s).unwrap();
        let back: CredentialStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get("anthropic_api_key").as_deref(), Some("sk-ant-test"));
    }

    #[test]
    fn empty_key_clears_the_field() {
        let mut s = CredentialStore::default();
        s.set("openai_api_key", "sk-something");
        assert!(s.get("openai_api_key").is_some());
        s.set("openai_api_key", "  ");
        assert!(s.get("openai_api_key").is_none());
    }

    #[test]
    fn unknown_account_is_a_no_op() {
        let mut s = CredentialStore::default();
        s.set("something_else", "value");
        assert!(s.get("something_else").is_none());
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        let mut s = CredentialStore::default();
        s.set("anthropic_api_key", "sk-ant-disk-test");
        let dir = std::env::temp_dir().join(format!("whimpr-cred-test-{}", std::process::id()));
        let path = dir.join("credentials.json");
        s.save(&path).unwrap();
        let loaded = CredentialStore::load(&path);
        assert_eq!(loaded.get("anthropic_api_key").as_deref(), Some("sk-ant-disk-test"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
