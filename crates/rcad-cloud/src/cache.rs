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
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
