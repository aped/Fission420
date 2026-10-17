//! Per-server local cache: working copies, links, blob cache, offline queue.
//!
//! Layout under the cache root (`$XDG_CACHE_HOME/r-cad/cloud` or `~/.cache/r-cad/cloud`):
//!
//! ```text
//! <server-key>/                 server-key = sanitized host[:port] of the server URL
//!   links.json                  Vec<Link>: local working file -> cloud document + base version
//!   pending.json                versions saved while offline, in order
//!   blobs/<aa>/<hash>           content-addressed copies of downloaded/uploaded versions
//!   work/<document-id>/<name>.rcad.json        editable working copy
//!   versions/<document-id>/v<N>-<name>.rcad.json  read-only copies of older versions
//! ```
//!
//! Every file is replaced atomically (temporary file + rename). Names that come from the
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

/// Directory name of a server in the cache: `host[_port][_path]`, sanitized.
pub fn server_key(server_url: &str) -> String {
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

/// The cache directory of one server.
#[derive(Clone, Debug)]
pub struct ServerCache {
    dir: PathBuf,
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
    /// The cache of `server_url` under `root` (directories are created when first written).
    pub fn new(root: &Path, server_url: &str) -> ServerCache {
        ServerCache {
            dir: root.join(server_key(server_url)),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    // -- Links ----------------------------------------------------------------------------------

    pub fn links(&self) -> Result<Vec<Link>, CloudError> {
        read_json(&self.dir.join("links.json"))
    }

    fn save_links(&self, links: &[Link]) -> Result<(), CloudError> {
        write_json(&self.dir.join("links.json"), &links)
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
        let mut links = self.links()?;
        links.retain(|l| normalize_local(&l.local_path) != link.local_path);
        links.push(link.clone());
        self.save_links(&links)?;
        Ok(link)
    }

    /// Forget the link of `path` (the file itself is untouched).
    pub fn remove_link(&self, path: &Path) -> Result<bool, CloudError> {
        let want = normalize_local(path);
        let mut links = self.links()?;
        let before = links.len();
        links.retain(|l| normalize_local(&l.local_path) != want);
        if links.len() != before {
            self.save_links(&links)?;
        }
        Ok(links.len() != before)
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

    pub fn set_pending(&self, queue: &[PendingSave]) -> Result<(), CloudError> {
        write_json(&self.dir.join("pending.json"), &queue)
    }

    /// Append to the queue; returns the queue length.
    pub fn enqueue(&self, save: PendingSave) -> Result<usize, CloudError> {
        let mut q = self.pending()?;
        q.push(save);
        self.set_pending(&q)?;
        Ok(q.len())
    }

    /// Remove and return the first queued save.
    pub fn dequeue(&self) -> Result<Option<PendingSave>, CloudError> {
        let mut q = self.pending()?;
        if q.is_empty() {
            return Ok(None);
        }
        let first = q.remove(0);
        self.set_pending(&q)?;
        Ok(Some(first))
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
        let c = ServerCache::new(root, "https://cad.example:8443/rcad");
        assert_eq!(c.dir(), Path::new("/cache/cad.example_8443_rcad"));
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
        let c = ServerCache::new(dir.path(), "http://127.0.0.1:9");
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
}
