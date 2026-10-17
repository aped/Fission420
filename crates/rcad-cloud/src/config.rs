//! Accounts: `<config>/r-cad/cloud.json` next to `prefs.json`, written with mode 0600.
//!
//! The file is versioned JSON (`"version": 1`). It holds each account's bearer token, so it
//! is created with mode 0600 on Unix, replaced atomically, and the token never appears in
//! `Debug` output or in any message this crate produces.
//!
//! The app and `rcad-cli` share the file, so changes go through [`CloudConfig::update`],
//! which holds an exclusive advisory lock (`.cloud.json.lock` next to the file) while it
//! re-reads, changes and writes the list: a sign-in in one never undoes one in the other.
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
    /// The server's [`crate::proto::ServerInfo::max_blob_bytes`] at sign-in: downloads larger
    /// than this are refused. Absent in files written by earlier builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_blob_bytes: Option<u64>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("server_url", &self.server_url)
            .field("server_name", &self.server_name)
            .field("username", &self.username)
            .field("token", &"<redacted>")
            .field("max_blob_bytes", &self.max_blob_bytes)
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

    /// Change the file at `path` under its lock: re-read it, apply `f`, write it back. The
    /// lock is held only for the local read and write, never across a network request.
    pub fn update<R>(path: &Path, f: impl FnOnce(&mut CloudConfig) -> R) -> Result<R, CloudError> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _lock = crate::cache::FileLock::acquire(&path.with_file_name(format!(".{name}.lock")))?;
        let mut config = CloudConfig::load(path)?;
        let r = f(&mut config);
        config.save(path)?;
        Ok(r)
    }

    /// The account in use, if any.
    pub fn active_account(&self) -> Option<&Account> {
        self.active.and_then(|i| self.accounts.get(i))
    }

    /// Index of the account on `server_url` (normalized first, so `cad.example/` matches
    /// `https://cad.example`) for `username`. Without a username the server must have
    /// exactly one account; several are an error that lists their usernames.
    pub fn find_account(
        &self,
        server_url: &str,
        username: Option<&str>,
    ) -> Result<usize, CloudError> {
        let url = crate::client::normalize_server_url(server_url)?;
        let matches: Vec<usize> = (0..self.accounts.len())
            .filter(|&i| {
                let a = &self.accounts[i];
                a.server_url == url && username.is_none_or(|u| a.username == u)
            })
            .collect();
        match (matches.as_slice(), username) {
            ([i], _) => Ok(*i),
            ([], Some(u)) => Err(CloudError::Invalid(format!(
                "no account for {u} on {url} (sign in with `rcad-cli cloud login {url} {u}`)"
            ))),
            ([], None) => Err(CloudError::Invalid(format!(
                "no account on {url} (sign in with `rcad-cli cloud login {url} USERNAME`)"
            ))),
            (several, _) => {
                let names: Vec<&str> = several
                    .iter()
                    .map(|&i| self.accounts[i].username.as_str())
                    .collect();
                Err(CloudError::Invalid(format!(
                    "{url} has several accounts ({}); name one",
                    names.join(", ")
                )))
            }
        }
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

    /// Forget the account of `username` on `server_url`. When it was the active one, the
    /// first remaining account (if any) becomes active.
    pub fn remove_account(&mut self, server_url: &str, username: &str) -> Option<Account> {
        let i = self
            .accounts
            .iter()
            .position(|a| a.server_url == server_url && a.username == username)?;
        let gone = self.accounts.remove(i);
        self.active = match self.active {
            _ if self.accounts.is_empty() => None,
            Some(a) if a == i => Some(0),
            Some(a) if a > i => Some(a - 1),
            other => other.filter(|&a| a < self.accounts.len()),
        };
        Some(gone)
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
            max_blob_bytes: None,
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

    #[test]
    fn remove_account_keeps_the_active_one_current() {
        let mut c = CloudConfig::default();
        c.upsert(account("ada"));
        c.upsert(account("bob"));
        c.upsert(account("cy"));
        c.active = Some(2);
        assert_eq!(
            c.remove_account("https://cad.example", "ada")
                .unwrap()
                .username,
            "ada"
        );
        assert_eq!(c.active_account().unwrap().username, "cy");
        assert!(c.remove_account("https://cad.example", "nobody").is_none());
        assert_eq!(
            c.remove_account("https://cad.example", "cy")
                .unwrap()
                .username,
            "cy"
        );
        assert_eq!(c.active_account().unwrap().username, "bob");
    }

    #[test]
    fn find_account_by_server_and_optional_username() {
        let mut c = CloudConfig::default();
        c.upsert(account("ada"));
        c.upsert(account("bob"));
        let mut other = account("ada");
        other.server_url = "http://lab.local:8640".into();
        c.upsert(other);
        assert_eq!(
            c.find_account("https://cad.example/", Some("bob")).unwrap(),
            1
        );
        assert_eq!(c.find_account("cad.example", Some("ada")).unwrap(), 0);
        assert_eq!(c.find_account("http://lab.local:8640", None).unwrap(), 2);
        let several = c.find_account("https://cad.example", None).unwrap_err();
        assert!(several.to_string().contains("ada, bob"), "{several}");
        assert!(c.find_account("https://cad.example", Some("cy")).is_err());
        assert!(c.find_account("https://nowhere.example", None).is_err());
        assert!(c.find_account("ftp://x", None).is_err());
    }

    #[test]
    fn concurrent_updates_lose_no_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud.json");
        std::thread::scope(|scope| {
            for t in 0..8 {
                let path = path.clone();
                scope.spawn(move || {
                    for i in 0..10 {
                        CloudConfig::update(&path, |c| c.upsert(account(&format!("u{t}-{i}"))))
                            .unwrap();
                    }
                });
            }
        });
        assert_eq!(CloudConfig::load(&path).unwrap().accounts.len(), 80);
    }

    #[test]
    fn files_of_earlier_builds_load_without_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud.json");
        std::fs::write(
            &path,
            r#"{"version":1,"accounts":[{"server_url":"https://x","server_name":"X","username":"ada","token":"t"}],"active":0}"#,
        )
        .unwrap();
        let c = CloudConfig::load(&path).unwrap();
        assert_eq!(c.active_account().unwrap().max_blob_bytes, None);
    }
}
