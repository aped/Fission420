//! Accounts: `<config>/r-cad/cloud.json` next to `prefs.json`, written with mode 0600.
//!
//! The file is versioned JSON (`"version": 1`). It holds each account's bearer token, so it
//! is created with mode 0600 on Unix, replaced atomically, and the token never appears in
//! `Debug` output or in any message this crate produces.
use crate::{CloudError, fsutil};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Version of the account file this build writes and reads.
pub const CONFIG_VERSION: u32 = 1;

/// One signed-in account on one server.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Normalized server URL (`https://host[:port][/prefix]`, no trailing slash).
    pub server_url: String,
    /// The server's human name ([`crate::proto::ServerInfo::name`]) at sign-in.
    pub server_name: String,
    pub username: String,
    /// Bearer token (session or personal access token). Secret.
    pub token: String,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("server_url", &self.server_url)
            .field("server_name", &self.server_name)
            .field("username", &self.username)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The account list. Empty (and never written) until the user signs in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CloudConfig {
    pub accounts: Vec<Account>,
    /// Index into `accounts` of the account the app uses.
    pub active: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct OnDisk {
    version: u32,
    #[serde(default)]
    accounts: Vec<Account>,
    #[serde(default)]
    active: Option<usize>,
}

impl CloudConfig {
    /// `$XDG_CONFIG_HOME/r-cad/cloud.json`, else `~/.config/r-cad/cloud.json`; None when
    /// neither variable gives an absolute directory.
    pub fn default_path() -> Option<PathBuf> {
        default_path()
    }

    /// Read `path`. An absent file is an empty configuration, not an error.
    pub fn load(path: &Path) -> Result<CloudConfig, CloudError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CloudConfig::default());
            }
            Err(e) => return Err(e.into()),
        };
        let disk: OnDisk = serde_json::from_str(&text).map_err(|e| {
            CloudError::Protocol(format!("account file {} is not valid: {e}", path.display()))
        })?;
        if disk.version > CONFIG_VERSION {
            return Err(CloudError::Protocol(format!(
                "account file {} has version {}, newer than this build ({CONFIG_VERSION})",
                path.display(),
                disk.version
            )));
        }
        let active = disk.active.filter(|&i| i < disk.accounts.len());
        Ok(CloudConfig {
            accounts: disk.accounts,
            active,
        })
    }

    /// Write `path` atomically (temporary file + rename), mode 0600 on Unix.
    pub fn save(&self, path: &Path) -> Result<(), CloudError> {
        let disk = OnDisk {
            version: CONFIG_VERSION,
            accounts: self.accounts.clone(),
            active: self.active.filter(|&i| i < self.accounts.len()),
        };
        let json = serde_json::to_vec_pretty(&disk)
            .map_err(|e| CloudError::Protocol(format!("cannot encode accounts: {e}")))?;
        fsutil::write_atomic(path, &json, true)?;
        Ok(())
    }

    /// The account in use, if any.
    pub fn active_account(&self) -> Option<&Account> {
        self.active.and_then(|i| self.accounts.get(i))
    }

    /// Add `account`, replacing one for the same server and username, and make it active.
    pub fn upsert(&mut self, account: Account) {
        let i = match self
            .accounts
            .iter()
            .position(|a| a.server_url == account.server_url && a.username == account.username)
        {
            Some(i) => {
                self.accounts[i] = account;
                i
            }
            None => {
                self.accounts.push(account);
                self.accounts.len() - 1
            }
        };
        self.active = Some(i);
    }

    /// Forget the active account; the first remaining account (if any) becomes active.
    pub fn remove_active(&mut self) -> Option<Account> {
        let i = self.active.filter(|&i| i < self.accounts.len())?;
        let gone = self.accounts.remove(i);
        self.active = if self.accounts.is_empty() {
            None
        } else {
            Some(0)
        };
        Some(gone)
    }
}

/// `$XDG_CONFIG_HOME/r-cad/cloud.json`, else `~/.config/r-cad/cloud.json` (the same
/// directory as the app's `prefs.json`).
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("r-cad").join("cloud.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(user: &str) -> Account {
        Account {
            server_url: "https://cad.example".into(),
            server_name: "Example".into(),
            username: user.into(),
            token: "rcs_secret_value".into(),
        }
    }

    #[test]
    fn absent_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let c = CloudConfig::load(&dir.path().join("cloud.json")).unwrap();
        assert_eq!(c, CloudConfig::default());
        assert!(c.active_account().is_none());
    }

    #[test]
    fn round_trip_is_versioned_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("cloud.json");
        let mut c = CloudConfig::default();
        c.upsert(account("ada"));
        c.upsert(account("bob"));
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(CloudConfig::load(&path).unwrap(), c);
        assert_eq!(c.active_account().unwrap().username, "bob");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No temporary files left behind.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn upsert_replaces_same_user_and_remove_active() {
        let mut c = CloudConfig::default();
        c.upsert(account("ada"));
        let mut again = account("ada");
        again.token = "rcs_new".into();
        c.upsert(again);
        assert_eq!(c.accounts.len(), 1);
        assert_eq!(c.active_account().unwrap().token, "rcs_new");
        assert_eq!(c.remove_active().unwrap().username, "ada");
        assert!(c.active_account().is_none());
        assert!(c.remove_active().is_none());
    }

    #[test]
    fn debug_redacts_tokens() {
        let mut c = CloudConfig::default();
        c.upsert(account("ada"));
        let text = format!("{c:?} {:?}", c.accounts[0]);
        assert!(!text.contains("rcs_secret_value"), "{text}");
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn newer_version_and_garbage_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud.json");
        std::fs::write(&path, r#"{"version":99,"accounts":[]}"#).unwrap();
        assert!(CloudConfig::load(&path).is_err());
        std::fs::write(&path, "not json").unwrap();
        assert!(CloudConfig::load(&path).is_err());
        // An out-of-range active index is dropped, not trusted.
        std::fs::write(&path, r#"{"version":1,"accounts":[],"active":3}"#).unwrap();
        assert_eq!(CloudConfig::load(&path).unwrap().active, None);
    }
}
