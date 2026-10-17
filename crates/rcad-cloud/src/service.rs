//! Background service: the UI sends [`Request`]s and polls [`Event`]s once per frame.
//!
//! Contract shared by the UI and the implementation (keep both sides to it):
//! * Every request produces at least one event; failures produce [`Event::Error`] naming
//!   the request (`Request::name`), or [`Event::Offline`] when the server is unreachable.
//! * Network work happens only on the service thread.
//! * A cloud document is edited as an ordinary local file (its working copy); the
//!   [`crate::cache::Link`] records which cloud document and base version it belongs to.
use crate::cache::Link;
use crate::proto;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Log in and store the account (replaces an account for the same server and user).
    SignIn { server_url: String, username: String, password: String },
    /// Revoke the token on the server (best effort) and forget the account.
    SignOut,
    /// Reload `me` (orgs) for the active account. Sent once after start when an account exists.
    Refresh,
    ListProjects { org: String },
    CreateProject { org: String, name: String },
    LoadTree { project: String },
    CreateFolder { project: String, parent: Option<String>, name: String },
    /// Download the head version into the working copy and report `DocumentReady`.
    /// Uses the cached copy (and reports `Offline`) when the server is unreachable.
    OpenDocument { document: String },
    /// Download a specific version as a read-only copy.
    OpenVersion { document: String, version: u64 },
    /// Upload a local document file as a new cloud document (version 1) and link the file.
    UploadNew { local_path: PathBuf, project: String, folder: Option<String>, name: String },
    /// Save the linked file as a new version. Queued if offline.
    SaveVersion { local_path: PathBuf, description: String, milestone: Option<String>, force: bool },
    LoadDetail { document: String },
    SetMilestone { document: String, version: u64, milestone: Option<String> },
    Promote { document: String, version: u64 },
    Lock { document: String },
    Unlock { document: String },
    CreateShare { document: String, version: Option<u64>, expires_in_days: Option<u32>, allow_download: bool },
    LoadComments { document: String },
    AddComment { document: String, version: u64, body: String },
    ResolveComment { document: String, comment: String, resolved: bool },
    /// Retry versions queued while offline.
    SyncPending,
}

impl Request {
    /// Stable short name used in [`Event::Error`].
    pub fn name(&self) -> &'static str {
        match self {
            Request::SignIn { .. } => "sign_in",
            Request::SignOut => "sign_out",
            Request::Refresh => "refresh",
            Request::ListProjects { .. } => "list_projects",
            Request::CreateProject { .. } => "create_project",
            Request::LoadTree { .. } => "load_tree",
            Request::CreateFolder { .. } => "create_folder",
            Request::OpenDocument { .. } => "open_document",
            Request::OpenVersion { .. } => "open_version",
            Request::UploadNew { .. } => "upload_new",
            Request::SaveVersion { .. } => "save_version",
            Request::LoadDetail { .. } => "load_detail",
            Request::SetMilestone { .. } => "set_milestone",
            Request::Promote { .. } => "promote",
            Request::Lock { .. } => "lock",
            Request::Unlock { .. } => "unlock",
            Request::CreateShare { .. } => "create_share",
            Request::LoadComments { .. } => "load_comments",
            Request::AddComment { .. } => "add_comment",
            Request::ResolveComment { .. } => "resolve_comment",
            Request::SyncPending => "sync_pending",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Signed in (or account restored at start): who, where, which orgs.
    SignedIn { server_url: String, server_name: String, me: proto::Me },
    SignedOut,
    Projects { org: String, projects: Vec<proto::Project> },
    Tree(proto::ProjectTree),
    /// A working copy (or read-only version copy) is on disk; the app opens `path`.
    DocumentReady { path: PathBuf, link: Link, read_only: bool, from_cache: bool },
    /// A version was saved; `link.base_version` is now `version.number`.
    VersionSaved { link: Link, version: proto::Version },
    /// The head moved since the working copy's base version.
    Conflict { local_path: PathBuf, document: String, base_version: u64, head_version: u64 },
    /// Another user holds the edit lock.
    Locked { document: String, lock: proto::Lock },
    LockAcquired { document: String, lock: proto::Lock },
    LockReleased { document: String },
    Detail(proto::DocumentDetail),
    ShareCreated(proto::CreateShareResponse),
    Comments { document: String, comments: Vec<proto::Comment> },
    /// The server is unreachable; `pending` versions wait in the offline queue.
    Offline { pending: usize },
    /// Queue state after a sync attempt.
    Synced { pending: usize },
    Error { request: &'static str, code: Option<proto::ErrorCode>, message: String },
}

/// Handle owned by the UI.
pub struct CloudService {
    pub(crate) tx: Sender<Request>,
    pub(crate) rx: Receiver<Event>,
}

impl CloudService {
    /// Queue a request for the service thread.
    pub fn send(&self, request: Request) {
        // The thread only stops when the handle is dropped; a send error means it panicked,
        // which the next poll reports.
        let _ = self.tx.send(request);
    }

    /// Next event, if any (non-blocking; call once per frame until None).
    pub fn poll(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }

    /// A service with no thread, for UI tests: the returned receiver sees every request the
    /// UI sends and the sender injects events.
    pub fn detached() -> (CloudService, Receiver<Request>, Sender<Event>) {
        let (tx, req_rx) = std::sync::mpsc::channel();
        let (ev_tx, rx) = std::sync::mpsc::channel();
        (CloudService { tx, rx }, req_rx, ev_tx)
    }
}
