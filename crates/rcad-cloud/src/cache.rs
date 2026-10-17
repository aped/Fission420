//! Per-server local cache: working copies, links, blob cache, offline queue.
//!
//! Layout under the cache root (`$XDG_CACHE_HOME/r-cad/cloud` or `~/.cache/r-cad/cloud`):
//!
//! ```text
//! <server-key>/                 server-key: see [`server_key`] (scheme, host, port, path
//!                               and username, so accounts and servers never share a dir)
//!   .lock                       advisory lock taken around every read-modify-write
//!   links.json                  Vec<Link>: local working file -> cloud document + base version
//!   pending.json                versions saved while offline, in order
//!   blobs/<aa>/<hash>           content-addressed copies of downloaded/uploaded versions
//!   work/<document-id>/<name>.rcad.json        editable working copy
//!   versions/<document-id>/v<N>-<name>.rcad.json  read-only copies of older versions
//! ```
//!
//! Every file is replaced atomically (temporary file + rename). The desktop app and
//! `rcad-cli` may run at the same time, so every read-modify-write of `links.json` and
//! `pending.json` holds an exclusive advisory lock on `.lock` ([`std::fs::File::lock`]): an
//! update made by one process is never overwritten by a stale copy in the other. Plain reads
//! take no lock (a rename is atomic, so a reader sees either the old or the new file).
//!
//! Names that come from the
//! server (document names and ids) pass through [`sanitize_file_name`] before they touch the
//! filesystem, so a hostile name cannot escape the cache directory.
use crate::proto::{self, BlobHash};
use crate::{CloudError, fsutil};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which cloud document a local file is a working copy of.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub local_path: PathBuf,
    pub server_url: String,
    pub org: String,
    pub project: String,
    pub document: String,
    pub document_name: String,
    /// Version the working copy was downloaded at or last saved as (0 = none yet).
    pub base_version: u64,
    /// Hash of the bytes of `base_version` (to tell whether the working copy changed).
    pub base_blob: Option<String>,
}

/// A version saved while the server was unreachable. The bytes were copied into the blob
/// cache when it was queued, so later edits of the file do not change what is uploaded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingSave {
    pub local_path: PathBuf,
    pub document: String,
    /// Blob cache hash of the snapshot taken at queue time.
    pub blob: BlobHash,
    pub description: String,
    pub milestone: Option<String>,
    /// The link's base version when queued.
    pub base_version: u64,
    /// Queued behind another save of the same document: on replay its base is the version
    /// the previous queued save produced, not `base_version`.
    #[serde(default)]
    pub chained: bool,
    #[serde(default)]
    pub force: bool,
    /// Unix seconds.
    pub queued_at: i64,
}

/// `$XDG_CACHE_HOME/r-cad/cloud`, else `~/.cache/r-cad/cloud`.
pub fn default_root() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("r-cad").join("cloud"))
}

/// Longest file name component produced by [`sanitize_file_name`], in bytes.
pub const MAX_NAME_BYTES: usize = 96;

/// A single safe file name component for `name` (which may come from the server): no path
/// separators, drive prefixes, control characters, characters Windows forbids, leading dots
/// or trailing dots/spaces, no reserved Windows device names, at most [`MAX_NAME_BYTES`]
/// bytes; never empty, `.` or `..`.
pub fn sanitize_file_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len().min(MAX_NAME_BYTES));
    for c in name.chars() {
        let bad = c.is_control()
            || matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'
            )
            || c == '\u{202E}'; // right-to-left override: hides the real extension
        let c = if bad { '_' } else { c };
        if out.len() + c.len_utf8() > MAX_NAME_BYTES {
            break;
        }
        out.push(c);
    }
    let trimmed = out
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    let mut out = trimmed.to_string();
    if out.is_empty() {
        out = "document".into();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    out
}

/// Directory name of an account's cache: `<host>[_<port>][_<path>]-<user>-<hash>`.
///
/// The readable part is sanitized and shortened; `hash` is 16 hex digits of BLAKE3 over the
/// normalized URL (scheme included, scheme and host lowercased) and the lowercased username.
/// So `http://h` and `https://h` get different directories, as do two accounts on one
/// server, and no two URLs collide through sanitizing.
pub fn server_key(server_url: &str, username: &str) -> String {
    let (scheme, rest) = server_url.split_once("://").unwrap_or(("", server_url));
    let rest = rest.trim_end_matches('/');
    let (host, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let canonical = format!(
        "{}://{}{path}\n{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase(),
        username.to_lowercase()
    );
    let hash = blake3::hash(canonical.as_bytes()).to_hex();
    let readable = shorten(
        &sanitize_file_name(&rest.to_ascii_lowercase().replace([':', '/'], "_")),
        48,
    );
    let user = shorten(&sanitize_file_name(&username.to_lowercase()), 32);
    format!("{readable}-{user}-{}", &hash[..16])
}

/// The first `max` bytes of `s` (on a character boundary).
fn shorten(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Directory name earlier builds used for a server (host, port and path only; no scheme,
/// no username). Read once, to adopt an existing cache.
pub fn legacy_server_key(server_url: &str) -> String {
    let rest = server_url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(server_url);
    let rest = rest.trim_end_matches('/').to_ascii_lowercase();
    sanitize_file_name(&rest.replace([':', '/'], "_"))
}

/// Strip a trailing `.rcad.json` / `.json` so it is not doubled.
fn stem_of(name: &str) -> &str {
    name.strip_suffix(".rcad.json")
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(name)
}

/// Create `dir` and missing parents; new directories get mode 0700 on Unix.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// The cache directory of one account on one server.
#[derive(Clone, Debug)]
pub struct ServerCache {
    dir: PathBuf,
    server_url: String,
}

/// An exclusive advisory lock on a cache or account file, released on drop.
pub(crate) struct FileLock {
    _file: std::fs::File,
}

impl FileLock {
    /// Block until the exclusive lock on `path` (created if absent) is held.
    pub(crate) fn acquire(path: &Path) -> Result<FileLock, CloudError> {
        fsutil::ensure_parent(path)?;
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(path)?;
        file.lock()?;
        Ok(FileLock { _file: file })
    }
}

/// Normalize a local path for link lookups: canonical when it exists, else absolute.
pub fn normalize_local(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|d| d.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Result<T, CloudError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            CloudError::Protocol(format!("cache file {} is not valid: {e}", path.display()))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CloudError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| CloudError::Protocol(format!("cannot encode {}: {e}", path.display())))?;
    fsutil::write_atomic(path, &bytes, false)?;
    Ok(())
}

impl ServerCache {
    /// The cache of `username` on `server_url` under `root`. Nothing is read or created
    /// (directories are created when first written).
    pub fn new(root: &Path, server_url: &str, username: &str) -> ServerCache {
        ServerCache {
            dir: root.join(server_key(server_url, username)),
            server_url: server_url.to_string(),
        }
    }

    /// [`ServerCache::new`], adopting the directory an earlier build kept for this server
    /// (keyed by host only) when this account has no directory yet and every link in the
    /// old one belongs to `server_url`. The first account opened on that server takes it
    /// over; a directory that also holds links to another scheme's server stays where it
    /// is (its documents download again).
    ///
    /// The directory (and missing parents) is created private to the user (mode 0700 on
    /// Unix), so the cached documents do not depend on the umask.
    pub fn open(root: &Path, server_url: &str, username: &str) -> ServerCache {
        let cache = ServerCache::new(root, server_url, username);
        let legacy = root.join(legacy_server_key(server_url));
        if !cache.dir.exists() && legacy.is_dir() && legacy != cache.dir {
            let links: Result<Vec<Link>, _> = read_json(&legacy.join("links.json"));
            if let Ok(links) = links
                && links.iter().all(|l| l.server_url == server_url)
            {
                // Atomic: if another process adopted it first, this rename fails harmlessly.
                let _ = std::fs::rename(&legacy, &cache.dir);
            }
        }
        // Best effort: a failure here shows up as an error on the first write.
        let _ = create_private_dir(&cache.dir);
        cache
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The server URL this cache belongs to.
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// Run `f` holding the cache's exclusive lock (see the module documentation).
    fn locked<R>(&self, f: impl FnOnce() -> Result<R, CloudError>) -> Result<R, CloudError> {
        let _lock = FileLock::acquire(&self.dir.join(".lock"))?;
        f()
    }

    // -- Links ----------------------------------------------------------------------------------

    fn all_links(&self) -> Result<Vec<Link>, CloudError> {
        read_json(&self.dir.join("links.json"))
    }

    /// Links to documents on this cache's server.
    pub fn links(&self) -> Result<Vec<Link>, CloudError> {
        let mut links = self.all_links()?;
        links.retain(|l| l.server_url == self.server_url);
        Ok(links)
    }

    /// Change the link list under the lock: re-read, apply `f`, write back.
    fn update_links<R>(&self, f: impl FnOnce(&mut Vec<Link>) -> R) -> Result<R, CloudError> {
        self.locked(|| {
            let path = self.dir.join("links.json");
            let mut links: Vec<Link> = read_json(&path)?;
            let before = links.clone();
            let r = f(&mut links);
            if links != before {
                write_json(&path, &links)?;
            }
            Ok(r)
        })
    }

    /// The link whose working copy is `path`.
    pub fn link_for_path(&self, path: &Path) -> Result<Option<Link>, CloudError> {
        let want = normalize_local(path);
        Ok(self
            .links()?
            .into_iter()
            .find(|l| normalize_local(&l.local_path) == want))
    }

    /// The (first) link to `document`.
    pub fn link_for_document(&self, document: &str) -> Result<Option<Link>, CloudError> {
        Ok(self.links()?.into_iter().find(|l| l.document == document))
    }

    /// Insert `link`, replacing the link of the same local file. `local_path` is stored
    /// normalized. Returns the stored link.
    pub fn upsert_link(&self, mut link: Link) -> Result<Link, CloudError> {
        link.local_path = normalize_local(&link.local_path);
        let stored = link.clone();
        self.update_links(move |links| {
            links.retain(|l| {
                l.server_url != link.server_url || normalize_local(&l.local_path) != link.local_path
            });
            links.push(link);
        })?;
        Ok(stored)
    }

    /// Forget the link of `path` (the file itself is untouched).
    pub fn remove_link(&self, path: &Path) -> Result<bool, CloudError> {
        let want = normalize_local(path);
        self.update_links(|links| {
            let before = links.len();
            links.retain(|l| {
                l.server_url != self.server_url || normalize_local(&l.local_path) != want
            });
            links.len() != before
        })
    }

    // -- Working and version copies --------------------------------------------------------------

    /// `work/<document>/<name>.rcad.json`, both components sanitized.
    pub fn working_copy_path(&self, document: &str, name: &str) -> PathBuf {
        self.dir
            .join("work")
            .join(sanitize_file_name(document))
            .join(format!("{}.rcad.json", sanitize_file_name(stem_of(name))))
    }

    /// `versions/<document>/v<N>-<name>.rcad.json`, sanitized.
    pub fn version_copy_path(&self, document: &str, version: u64, name: &str) -> PathBuf {
        let file = sanitize_file_name(&format!("v{version}-{}", stem_of(name)));
        self.dir
            .join("versions")
            .join(sanitize_file_name(document))
            .join(format!("{file}.rcad.json"))
    }

    // -- Blobs ----------------------------------------------------------------------------------

    /// `blobs/<aa>/<hash>`; only well-formed hashes are accepted.
    pub fn blob_path(&self, hash: &str) -> Result<PathBuf, CloudError> {
        if !proto::is_blob_hash(hash) {
            return Err(CloudError::Protocol(format!("not a blob hash: {hash:.80}")));
        }
        Ok(self.dir.join("blobs").join(&hash[..2]).join(hash))
    }

    pub fn has_blob(&self, hash: &str) -> bool {
        self.blob_path(hash).map(|p| p.is_file()).unwrap_or(false)
    }

    /// Store `bytes`, returning their hash (no-op when already present).
    pub fn store_blob(&self, bytes: &[u8]) -> Result<BlobHash, CloudError> {
        let hash = crate::blob_hash(bytes);
        let path = self.blob_path(&hash)?;
        if !path.is_file() {
            fsutil::write_atomic(&path, bytes, false)?;
        }
        Ok(hash)
    }

    /// Read a cached blob, verifying its hash. A corrupt entry is deleted and reported.
    pub fn read_blob(&self, hash: &str) -> Result<Vec<u8>, CloudError> {
        let path = self.blob_path(hash)?;
        let bytes = std::fs::read(&path)?;
        if crate::blob_hash(&bytes) != hash {
            let _ = std::fs::remove_file(&path);
            return Err(CloudError::Protocol(format!(
                "cached blob {hash} was corrupt"
            )));
        }
        Ok(bytes)
    }

    // -- Offline queue --------------------------------------------------------------------------

    pub fn pending(&self) -> Result<Vec<PendingSave>, CloudError> {
        read_json(&self.dir.join("pending.json"))
    }

    /// Change the queue under the lock: re-read, apply `f`, write back if it changed.
    pub fn update_pending<R>(
        &self,
        f: impl FnOnce(&mut Vec<PendingSave>) -> R,
    ) -> Result<R, CloudError> {
        self.locked(|| {
            let path = self.dir.join("pending.json");
            let mut queue: Vec<PendingSave> = read_json(&path)?;
            let before = queue.clone();
            let r = f(&mut queue);
            if queue != before {
                write_json(&path, &queue)?;
            }
            Ok(r)
        })
    }

    /// Append to the queue; returns the queue length.
    pub fn enqueue(&self, save: PendingSave) -> Result<usize, CloudError> {
        self.update_pending(|q| {
            q.push(save);
            q.len()
        })
    }

    /// Remove the queued save equal to `save` (by value, not position, so entries added or
    /// removed meanwhile by another process are kept). Returns whether it was still queued.
    pub fn remove_pending(&self, save: &PendingSave) -> Result<bool, CloudError> {
        self.update_pending(|q| match q.iter().position(|p| p == save) {
            Some(i) => {
                q.remove(i);
                true
            }
            None => false,
        })
    }

    /// Remove and return the first queued save.
    pub fn dequeue(&self) -> Result<Option<PendingSave>, CloudError> {
        self.update_pending(|q| (!q.is_empty()).then(|| q.remove(0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_names_are_sanitized() {
        let long = "x".repeat(1000);
        let cases = [
            "../../etc/passwd",
            "..",
            ".",
            "",
            "   ",
            "C:\\x",
            "C:",
            "/abs/path",
            "a\0b",
            "line\nbreak",
            "CON",
            "nul.txt",
            "com1",
            "evil\u{202E}nosj.exe",
            long.as_str(),
            "日本語の部品",
            "émoji 🔩 bolt",
        ];
        for name in cases {
            let s = sanitize_file_name(name);
            assert!(!s.is_empty(), "{name:?}");
            assert!(s != "." && s != "..", "{name:?}");
            assert!(
                !s.contains(['/', '\\', ':', '\0', '\n']),
                "{name:?} -> {s:?}"
            );
            assert!(!s.starts_with('.'), "{name:?} -> {s:?}");
            assert!(s.len() <= MAX_NAME_BYTES, "{name:?}");
            assert_eq!(Path::new(&s).components().count(), 1, "{name:?} -> {s:?}");
            assert!(
                matches!(
                    Path::new(&s).components().next(),
                    Some(std::path::Component::Normal(_))
                ),
                "{name:?} -> {s:?}"
            );
        }
        assert_eq!(sanitize_file_name("Bracket v2"), "Bracket v2");
        assert_eq!(sanitize_file_name("日本語の部品"), "日本語の部品");
        assert_eq!(sanitize_file_name("CON"), "_CON");
        assert_eq!(sanitize_file_name(""), "document");
    }

    #[test]
    fn cache_paths_stay_inside_the_server_dir() {
        let root = Path::new("/cache");
        let c = ServerCache::new(root, "https://cad.example:8443/rcad", "ada");
        assert_eq!(c.dir().parent(), Some(root));
        let name = c.dir().file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("cad.example_8443_rcad-ada-"), "{name}");
        let w = c.working_copy_path("../../..", "../../etc/passwd");
        assert!(w.starts_with(c.dir().join("work")), "{w:?}");
        assert!(
            !w.components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
        );
        let v = c.version_copy_path("d/../x", 3, "C:\\Windows\\evil.rcad.json");
        assert!(v.starts_with(c.dir().join("versions")), "{v:?}");
        assert!(
            !v.components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
        );
        assert!(v.to_string_lossy().ends_with(".rcad.json"));
        assert_eq!(
            c.working_copy_path("d1", "Bracket.rcad.json")
                .file_name()
                .unwrap(),
            "Bracket.rcad.json"
        );
        assert!(c.blob_path("../x").is_err());
    }

    #[test]
    fn links_blobs_and_queue() {
        let dir = tempfile::tempdir().unwrap();
        let c = ServerCache::new(dir.path(), "http://127.0.0.1:9", "ada");
        let file = dir.path().join("part.rcad.json");
        std::fs::write(&file, b"{}").unwrap();
        let link = Link {
            local_path: file.clone(),
            server_url: "http://127.0.0.1:9".into(),
            org: "o".into(),
            project: "p".into(),
            document: "d".into(),
            document_name: "Part".into(),
            base_version: 1,
            base_blob: None,
        };
        c.upsert_link(link.clone()).unwrap();
        c.upsert_link(Link {
            base_version: 2,
            ..link.clone()
        })
        .unwrap();
        assert_eq!(c.links().unwrap().len(), 1);
        assert_eq!(c.link_for_path(&file).unwrap().unwrap().base_version, 2);
        assert_eq!(c.link_for_document("d").unwrap().unwrap().base_version, 2);
        assert!(c.remove_link(&file).unwrap());
        assert!(c.link_for_path(&file).unwrap().is_none());

        let h = c.store_blob(b"hello").unwrap();
        assert!(c.has_blob(&h));
        assert_eq!(c.read_blob(&h).unwrap(), b"hello");
        std::fs::write(c.blob_path(&h).unwrap(), b"tampered").unwrap();
        assert!(c.read_blob(&h).is_err());
        assert!(!c.has_blob(&h), "corrupt blob is dropped");

        let save = |d: &str| PendingSave {
            local_path: file.clone(),
            document: d.into(),
            blob: h.clone(),
            description: String::new(),
            milestone: None,
            base_version: 1,
            chained: false,
            force: false,
            queued_at: 0,
        };
        assert_eq!(c.enqueue(save("a")).unwrap(), 1);
        assert_eq!(c.enqueue(save("b")).unwrap(), 2);
        assert_eq!(c.dequeue().unwrap().unwrap().document, "a");
        assert_eq!(c.pending().unwrap().len(), 1);
    }

    #[test]
    fn cache_dirs_separate_schemes_accounts_and_lookalike_urls() {
        let root = Path::new("/cache");
        let dir = |url: &str, user: &str| ServerCache::new(root, url, user).dir().to_path_buf();
        let a = dir("https://h.example", "ada");
        assert_ne!(
            a,
            dir("http://h.example", "ada"),
            "scheme is part of the key"
        );
        assert_ne!(
            a,
            dir("https://h.example", "bob"),
            "username is part of the key"
        );
        assert_eq!(
            a,
            dir("https://H.EXAMPLE", "Ada"),
            "host and username are case-blind"
        );
        // These sanitize to the same readable text; the hash keeps them apart.
        assert_ne!(
            dir("https://h.example:8443", "ada"),
            dir("https://h.example/8443", "ada")
        );
        assert_ne!(
            dir("https://h.example/a_b", "ada"),
            dir("https://h.example/a/b", "ada")
        );
        // A hostile username stays one short component.
        let evil = dir("https://h.example", "../../x\u{202E}");
        assert_eq!(evil.parent(), Some(root));
        assert!(evil.file_name().unwrap().len() <= 48 + 32 + 18);
    }

    fn link_to(server: &str, file: &Path, doc: &str) -> Link {
        Link {
            local_path: file.to_path_buf(),
            server_url: server.into(),
            org: "o".into(),
            project: "p".into(),
            document: doc.into(),
            document_name: "Part".into(),
            base_version: 1,
            base_blob: None,
        }
    }

    #[test]
    fn links_of_another_server_are_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let c = ServerCache::new(dir.path(), "https://h.example", "ada");
        let file = dir.path().join("part.rcad.json");
        std::fs::write(&file, b"{}").unwrap();
        // A link written for http://h.example into this directory (as an old shared cache
        // could hold) is invisible to the https account.
        write_json(
            &c.dir().join("links.json"),
            &vec![link_to("http://h.example", &file, "d")],
        )
        .unwrap();
        assert!(c.link_for_path(&file).unwrap().is_none());
        assert!(c.link_for_document("d").unwrap().is_none());
        assert!(c.links().unwrap().is_empty());
        // Upserting keeps the foreign entry in the file (it is not this cache's to drop).
        c.upsert_link(link_to("https://h.example", &file, "e"))
            .unwrap();
        assert_eq!(c.all_links().unwrap().len(), 2);
        assert_eq!(c.link_for_path(&file).unwrap().unwrap().document, "e");
    }

    #[test]
    fn an_old_host_keyed_cache_is_adopted_only_when_it_matches() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("part.rcad.json");
        std::fs::write(&file, b"{}").unwrap();
        let legacy = root.path().join(legacy_server_key("https://h.example"));
        write_json(
            &legacy.join("links.json"),
            &vec![link_to("https://h.example", &file, "d")],
        )
        .unwrap();
        // The http server of the same host does not take the https server's cache.
        let http = ServerCache::open(root.path(), "http://h.example", "ada");
        assert!(http.links().unwrap().is_empty());
        assert!(legacy.is_dir());
        let c = ServerCache::open(root.path(), "https://h.example", "ada");
        assert!(!legacy.exists());
        assert_eq!(c.link_for_path(&file).unwrap().unwrap().document, "d");
        // A second account on that server starts empty.
        let bob = ServerCache::open(root.path(), "https://h.example", "bob");
        assert!(bob.links().unwrap().is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(bob.dir()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn concurrent_writers_lose_no_update() {
        // Several handles (as the app and the CLI would hold) enqueue and upsert at once.
        let dir = tempfile::tempdir().unwrap();
        let url = "https://h.example";
        let threads = 8;
        let per = 25;
        std::thread::scope(|scope| {
            for t in 0..threads {
                let root = dir.path().to_path_buf();
                scope.spawn(move || {
                    let c = ServerCache::new(&root, url, "ada");
                    for i in 0..per {
                        let file = root.join(format!("f{t}-{i}.rcad.json"));
                        c.upsert_link(link_to(url, &file, &format!("d{t}-{i}")))
                            .unwrap();
                        c.enqueue(PendingSave {
                            local_path: file,
                            document: format!("d{t}-{i}"),
                            blob: "0".repeat(64),
                            description: String::new(),
                            milestone: None,
                            base_version: 0,
                            chained: false,
                            force: false,
                            queued_at: i as i64,
                        })
                        .unwrap();
                    }
                });
            }
        });
        let c = ServerCache::new(dir.path(), url, "ada");
        assert_eq!(c.links().unwrap().len(), threads * per);
        let q = c.pending().unwrap();
        assert_eq!(q.len(), threads * per);
        // Removal by value keeps everything else.
        assert!(c.remove_pending(&q[7]).unwrap());
        assert!(!c.remove_pending(&q[7]).unwrap());
        assert_eq!(c.pending().unwrap().len(), threads * per - 1);
    }
}
