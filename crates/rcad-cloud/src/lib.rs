//! Optional client for an R-CAD sync server.
//!
//! Nothing here runs unless the user adds an account: with no account configured the app
//! never constructs a [`service::CloudService`], opens no socket and reads no cloud files
//! other than the (absent) account list. Local documents never depend on this crate.
//!
//! * [`config`]: accounts (server URL, username, token) in `<config>/r-cad/cloud.json`.
//! * [`client`]: blocking HTTP client for the protocol in `rcad_cloud_proto`.
//! * [`cache`]: per-server cache under `<cache>/r-cad/cloud/`: working copies of cloud
//!   documents, the link from a local file to its cloud document and base version, the
//!   content-addressed blob cache and the queue of versions saved while offline.
//! * [`service`]: a background thread that runs [`service::Request`]s and reports
//!   [`service::Event`]s, so the UI never blocks on the network.
pub mod cache;
pub mod client;
pub mod config;
mod fsutil;
pub mod service;

pub use rcad_cloud_proto as proto;

/// Lowercase hex BLAKE3 hash of `bytes` (a [`proto::BlobHash`]).
pub fn blob_hash(bytes: &[u8]) -> proto::BlobHash {
    blake3::hash(bytes).to_hex().to_string()
}

/// `R-CAD <version>`: the HTTP User-Agent and the `generator` of saved versions.
pub fn generator() -> String {
    format!("R-CAD {}", env!("CARGO_PKG_VERSION"))
}

/// Errors of the cloud client. `Api` carries the server's structured error.
#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("{0}")]
    Api(proto::ApiError),
    /// The server could not be reached (DNS, connect, TLS, timeout): treated as offline.
    #[error("server unreachable: {0}")]
    Offline(String),
    #[error("unexpected response: {0}")]
    Protocol(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not signed in")]
    NotSignedIn,
    /// The request cannot be carried out with the local state (e.g. the file is not linked
    /// to a cloud document); no request reached the server.
    #[error("{0}")]
    Invalid(String),
}

impl CloudError {
    /// The server's error code, if this is a structured API error.
    pub fn code(&self) -> Option<proto::ErrorCode> {
        match self {
            CloudError::Api(e) => Some(e.code),
            _ => None,
        }
    }
}
