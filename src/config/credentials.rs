//! Persistent, workspace-independent credential storage.
//!
//! File-based — not a true OS-level encrypted vault; see the caveat below
//! — but crucially **not** the current directory, **not** the workspace,
//! and **not** `rexo.toml`. It lives in the same per-user, per-machine
//! directory as the rest of global config ([`super::global::global_dir`]),
//! which the OS already restricts to the owning user account, and this
//! module additionally locks the file down to owner-read/write only on
//! Unix.
//!
//! **Stated plainly rather than oversold:** this is not Windows Credential
//! Manager / macOS Keychain / a Secret Service vault — a key sits in a
//! plain file, not an OS-encrypted store. It *is*, however, structurally
//! outside any git-tracked project directory (impossible to `git add` by
//! accident, unlike a workspace `.env`), file-permission-protected, and —
//! the actual bug this exists to fix — resolved identically no matter
//! which directory `rexo` is started from. True OS-keychain integration
//! is tracked as a roadmap item; see the README.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::global::global_dir;

#[derive(Clone, Default, Serialize, Deserialize)]
struct CredentialFile {
    #[serde(flatten)]
    keys: BTreeMap<String, String>,
}

// Deliberately hand-written rather than `#[derive(Debug)]`: this struct
// holds raw secret values, and a derived Debug would print them verbatim
// the moment anyone (now or in some future change) does `{:?}` on it for
// an error message or log line. Showing only *which* keys are present —
// never their values — makes that mistake structurally impossible here.
impl std::fmt::Debug for CredentialFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialFile")
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn open() -> Result<Self> {
        Ok(Self { path: global_dir()?.join("credentials.toml") })
    }

    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        Self { path }
    }

    fn read(&self) -> Result<CredentialFile> {
        if !self.path.exists() {
            return Ok(CredentialFile::default());
        }
        let raw = std::fs::read_to_string(&self.path).with_context(|| format!("Failed to read {}", self.path.display()))?;
        toml::from_str(&raw).with_context(|| {
            format!(
                "Failed to parse {} — it looks corrupted. Delete it to reset saved credentials \
                 (you'll need to reconnect providers), or restore it from a backup.",
                self.path.display()
            )
        })
    }

    fn write(&self, file: &CredentialFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(file)?;
        std::fs::write(&self.path, raw).with_context(|| format!("Failed to write {}", self.path.display()))?;
        harden_permissions(&self.path);
        Ok(())
    }

    /// `key` is a provider profile's `credential_key` (e.g. `"openrouter"`
    /// or `"work-openai"`) — not a provider *kind*. Two profiles of the
    /// same kind can hold independent keys under different names.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.read()?.keys.get(key).cloned())
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        let mut file = self.read()?;
        file.keys.insert(key.to_string(), value.to_string());
        self.write(&file)
    }

    pub fn delete(&self, key: &str) -> Result<bool> {
        let mut file = self.read()?;
        let existed = file.keys.remove(key).is_some();
        if existed {
            self.write(&file)?;
        }
        Ok(existed)
    }

    /// Names of every credential currently stored — **never** their
    /// values. Used by `/doctor`/`/status`-style displays.
    pub fn configured_keys(&self) -> Result<Vec<String>> {
        Ok(self.read()?.keys.keys().cloned().collect())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn harden_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn harden_permissions(_path: &Path) {
    // Windows: the file already lives under the current user's
    // %LOCALAPPDATA%, which NTFS restricts to that account by default —
    // there's no portable `chmod` equivalent in std, and reaching for a
    // Windows-specific ACL API just for this felt like more risk (an
    // untestable-in-this-sandbox unsafe FFI call) than it's worth right
    // now. Tracked as a roadmap item alongside real OS-keychain support.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> CredentialStore {
        let dir = std::env::temp_dir().join(format!("rexo_test_credentials_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        CredentialStore::at(dir.join("credentials.toml"))
    }

    #[test]
    fn round_trips_a_credential() {
        let store = temp_store("roundtrip");
        assert_eq!(store.get("openrouter").unwrap(), None);
        store.set("openrouter", "sk-test-123").unwrap();
        assert_eq!(store.get("openrouter").unwrap(), Some("sk-test-123".to_string()));
    }

    #[test]
    fn delete_removes_and_reports_whether_it_existed() {
        let store = temp_store("delete");
        assert!(!store.delete("nope").unwrap());
        store.set("nvidia", "nvapi-x").unwrap();
        assert!(store.delete("nvidia").unwrap());
        assert_eq!(store.get("nvidia").unwrap(), None);
    }

    #[test]
    fn configured_keys_lists_names_not_values() {
        let store = temp_store("keys");
        store.set("openai", "sk-abc").unwrap();
        store.set("nvidia", "nvapi-def").unwrap();
        let keys = store.configured_keys().unwrap();
        assert!(keys.contains(&"openai".to_string()));
        assert!(keys.contains(&"nvidia".to_string()));
    }

    #[test]
    fn debug_format_never_includes_the_secret_value() {
        let file = CredentialFile { keys: BTreeMap::from([("openai".to_string(), "sk-super-secret".to_string())]) };
        let debugged = format!("{file:?}");
        assert!(!debugged.contains("sk-super-secret"));
        assert!(debugged.contains("openai"));
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let store = temp_store("missing");
        assert_eq!(store.get("anything").unwrap(), None);
        assert_eq!(store.configured_keys().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn corrupted_file_gives_a_clear_error_not_a_panic() {
        let store = temp_store("corrupt");
        std::fs::write(store.path(), "this is not valid toml {{{").unwrap();
        let err = store.get("anything").unwrap_err();
        assert!(err.to_string().contains("corrupted"));
    }

    #[cfg(unix)]
    #[test]
    fn file_permissions_are_owner_only_after_a_write() {
        use std::os::unix::fs::PermissionsExt;
        let store = temp_store("perms");
        store.set("x", "y").unwrap();
        let mode = std::fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
