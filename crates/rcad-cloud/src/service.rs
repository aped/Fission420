//! Background service: the UI sends [`Request`]s and polls [`Event`]s once per frame.
//!
//! Contract shared by the UI and the implementation (keep both sides to it):
//! * Every request produces at least one event; failures produce [`Event::Error`] naming
//!   the request (`Request::name`), or [`Event::Offline`] when the server is unreachable.
//! * Network work happens only on the service thread.
//! * A cloud document is edited as an ordinary local file (its working copy); the
//!   [`crate::cache::Link`] records which cloud document and base version it belongs to.
//!
//! Behaviour of the implementation ([`CloudService::start`], [`Worker`]):
//! * The thread does nothing until the first request; the UI sends `Refresh` after start
//!   when an account exists. The account file is re-read for every request, so a sign-in
//!   from `rcad-cli cloud login` is picked up by a running app.
//! * `SignIn` checks `GET /info` (product and protocol version) before sending the password;
//!   an unreachable server is reported as `Error` (there is no account to be offline with).
//!   Over plain `http://` to a host that is not on a local or private network
//!   ([`crate::client::plain_http_is_local`]) it refuses unless `allow_insecure_http` is set,
//!   before any request is made.
//! * The account file and each account's cache directory are shared with `rcad-cli`; every
//!   read-modify-write of them holds an advisory lock, and the offline queue is re-read
//!   before each step of a replay and changed by entry, never rewritten from a stale copy.
//! * Downloads stop at the size the server declared for the version, and versions larger
//!   than the server's advertised `max_blob_bytes` (1 GiB when unknown) are refused.
//! * `OpenDocument` never overwrites a working copy with local modifications (its bytes
//!   differ from the link's `base_blob`): it reports `DocumentReady` for the existing copy and,
//!   when the server head moved past the copy's base version, a `Conflict` as well.
//! * `SaveVersion` while offline (or while earlier saves of the same document are queued)
//!   snapshots the file into the blob cache and appends to the queue; `SyncPending` replays the
//!   queue in order and stops at the first conflict or lock, which stays queued. A queued
//!   save that can never succeed (document gone, no permission, too large, file no longer
//!   linked) is dropped with an `Error`; its bytes stay in the blob cache and the file is
//!   untouched. `SaveVersion { force: true }` forces the document's queued saves too.
//! * A conflict whose server head already holds exactly the bytes being saved (a save whose
//!   answer was lost to a dropped connection) is treated as saved.
//! * When creating a version fails with `HashMismatch` (the server no longer has the blob,
//!   e.g. its garbage collection removed it between the upload check and the save), the blob
//!   is uploaded again from the local cache and the save retried once before it fails.
//! * `LoadPending` and `DiscardPending` work on the local queue only (no network); both answer
//!   with `PendingQueue`. Discarding never touches the file or the blob cache.
//! * Member and organization changes answer with the reloaded list (`ProjectMembers`,
//!   `OrgMembers`); document and folder changes (rename, move, trash, restore) answer with the
//!   reloaded `Tree`. Renaming a document also renames it in the local links.
use crate::cache::{Link, PendingSave, ServerCache};
use crate::client::Client;
use crate::config::{Account, CloudConfig};
use crate::proto::{self, ErrorCode};
use crate::{CloudError, fsutil};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

#[derive(Clone, PartialEq)]
pub enum Request {
    /// Log in and store the account (replaces an account for the same server and user).
    SignIn {
        server_url: String,
        username: String,
        password: String,
        /// The user confirmed signing in over unencrypted `http://` to a host that is not on
        /// a local network (see [`crate::client::needs_insecure_consent`]).
        allow_insecure_http: bool,
    },
    /// Revoke the token on the server (best effort) and forget the account.
    SignOut,
    /// Reload `me` (orgs) for the active account. Sent once after start when an account exists.
    Refresh,
    ListProjects {
        org: String,
    },
    CreateProject {
        org: String,
        name: String,
    },
    LoadTree {
        project: String,
    },
    CreateFolder {
        project: String,
        parent: Option<String>,
        name: String,
    },
    /// Download the head version into the working copy and report `DocumentReady`.
    /// Uses the cached copy (and reports `Offline`) when the server is unreachable.
    OpenDocument {
        document: String,
    },
    /// Download a specific version as a read-only copy.
    OpenVersion {
        document: String,
        version: u64,
    },
    /// Upload a local document file as a new cloud document (version 1) and link the file.
    UploadNew {
        local_path: PathBuf,
        project: String,
        folder: Option<String>,
        name: String,
    },
    /// Save the linked file as a new version. Queued if offline.
    SaveVersion {
        local_path: PathBuf,
        description: String,
        milestone: Option<String>,
        force: bool,
    },
    LoadDetail {
        document: String,
    },
    SetMilestone {
        document: String,
        version: u64,
        milestone: Option<String>,
    },
    Promote {
        document: String,
        version: u64,
    },
    Lock {
        document: String,
    },
    Unlock {
        document: String,
    },
    CreateShare {
        document: String,
        version: Option<u64>,
        expires_in_days: Option<u32>,
        allow_download: bool,
    },
    LoadComments {
        document: String,
    },
    AddComment {
        document: String,
        version: u64,
        body: String,
    },
    ResolveComment {
        document: String,
        comment: String,
        resolved: bool,
    },
    /// Retry versions queued while offline.
    SyncPending,
    /// Report the offline queue ([`Event::PendingQueue`]); local only.
    LoadPending,
    /// Drop the queued save at `index` (its snapshot stays in the blob cache; the file is
    /// untouched). `document` and `blob` must match the entry, so a queue that changed since
    /// the UI listed it is not edited by position alone. Local only.
    DiscardPending {
        index: usize,
        document: String,
        blob: proto::BlobHash,
    },
    LoadProjectMembers {
        project: String,
    },
    /// Add a member or change their role (project admin).
    SetProjectMember {
        project: String,
        username: String,
        role: proto::ProjectRole,
    },
    RemoveProjectMember {
        project: String,
        username: String,
    },
    LoadOrgMembers {
        org: String,
    },
    /// Add a member or change their role (organization admin).
    SetOrgMember {
        org: String,
        username: String,
        role: proto::OrgRole,
    },
    RemoveOrgMember {
        org: String,
        username: String,
    },
    LoadUsage {
        org: String,
    },
    RenameDocument {
        document: String,
        name: String,
    },
    /// Move a document to `folder` (None: the project root).
    MoveDocument {
        document: String,
        folder: Option<String>,
    },
    /// Move a document to the project's trash (restorable).
    TrashDocument {
        project: String,
        document: String,
    },
    RestoreDocument {
        document: String,
    },
    RenameFolder {
        project: String,
        folder: String,
        name: String,
    },
    /// Delete an empty folder.
    DeleteFolder {
        project: String,
        folder: String,
    },
}

/// Like a derived `Debug`, except that the sign-in password is never printed.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::SignIn {
                server_url,
                username,
                password: _,
                allow_insecure_http,
            } => f
                .debug_struct("SignIn")
                .field("server_url", server_url)
                .field("username", username)
                .field("password", &"<redacted>")
                .field("allow_insecure_http", allow_insecure_http)
                .finish(),
            Request::SignOut => f.write_str("SignOut"),
            Request::Refresh => f.write_str("Refresh"),
            Request::ListProjects { org } => {
                f.debug_struct("ListProjects").field("org", org).finish()
            }
            Request::CreateProject { org, name } => f
                .debug_struct("CreateProject")
                .field("org", org)
                .field("name", name)
                .finish(),
            Request::LoadTree { project } => f
                .debug_struct("LoadTree")
                .field("project", project)
                .finish(),
            Request::CreateFolder {
                project,
                parent,
                name,
            } => f
                .debug_struct("CreateFolder")
                .field("project", project)
                .field("parent", parent)
                .field("name", name)
                .finish(),
            Request::OpenDocument { document } => f
                .debug_struct("OpenDocument")
                .field("document", document)
                .finish(),
            Request::OpenVersion { document, version } => f
                .debug_struct("OpenVersion")
                .field("document", document)
                .field("version", version)
                .finish(),
            Request::UploadNew {
                local_path,
                project,
                folder,
                name,
            } => f
                .debug_struct("UploadNew")
                .field("local_path", local_path)
                .field("project", project)
                .field("folder", folder)
                .field("name", name)
                .finish(),
            Request::SaveVersion {
                local_path,
                description,
                milestone,
                force,
            } => f
                .debug_struct("SaveVersion")
                .field("local_path", local_path)
                .field("description", description)
                .field("milestone", milestone)
                .field("force", force)
                .finish(),
            Request::LoadDetail { document } => f
                .debug_struct("LoadDetail")
                .field("document", document)
                .finish(),
            Request::SetMilestone {
                document,
                version,
                milestone,
            } => f
                .debug_struct("SetMilestone")
                .field("document", document)
                .field("version", version)
                .field("milestone", milestone)
                .finish(),
            Request::Promote { document, version } => f
                .debug_struct("Promote")
                .field("document", document)
                .field("version", version)
                .finish(),
            Request::Lock { document } => {
                f.debug_struct("Lock").field("document", document).finish()
            }
            Request::Unlock { document } => f
                .debug_struct("Unlock")
                .field("document", document)
                .finish(),
            Request::CreateShare {
                document,
                version,
                expires_in_days,
                allow_download,
            } => f
                .debug_struct("CreateShare")
                .field("document", document)
                .field("version", version)
                .field("expires_in_days", expires_in_days)
                .field("allow_download", allow_download)
                .finish(),
            Request::LoadComments { document } => f
                .debug_struct("LoadComments")
                .field("document", document)
                .finish(),
            Request::AddComment {
                document,
                version,
                body,
            } => f
                .debug_struct("AddComment")
                .field("document", document)
                .field("version", version)
                .field("body", body)
                .finish(),
            Request::ResolveComment {
                document,
                comment,
                resolved,
            } => f
                .debug_struct("ResolveComment")
                .field("document", document)
                .field("comment", comment)
                .field("resolved", resolved)
                .finish(),
            Request::SyncPending => f.write_str("SyncPending"),
            Request::LoadPending => f.write_str("LoadPending"),
            Request::DiscardPending {
                index,
                document,
                blob,
            } => f
                .debug_struct("DiscardPending")
                .field("index", index)
                .field("document", document)
                .field("blob", blob)
                .finish(),
            Request::LoadProjectMembers { project } => f
                .debug_struct("LoadProjectMembers")
                .field("project", project)
                .finish(),
            Request::SetProjectMember {
                project,
                username,
                role,
            } => f
                .debug_struct("SetProjectMember")
                .field("project", project)
                .field("username", username)
                .field("role", role)
                .finish(),
            Request::RemoveProjectMember { project, username } => f
                .debug_struct("RemoveProjectMember")
                .field("project", project)
                .field("username", username)
                .finish(),
            Request::LoadOrgMembers { org } => {
                f.debug_struct("LoadOrgMembers").field("org", org).finish()
            }
            Request::SetOrgMember {
                org,
                username,
                role,
            } => f
                .debug_struct("SetOrgMember")
                .field("org", org)
                .field("username", username)
                .field("role", role)
                .finish(),
            Request::RemoveOrgMember { org, username } => f
                .debug_struct("RemoveOrgMember")
                .field("org", org)
                .field("username", username)
                .finish(),
            Request::LoadUsage { org } => f.debug_struct("LoadUsage").field("org", org).finish(),
            Request::RenameDocument { document, name } => f
                .debug_struct("RenameDocument")
                .field("document", document)
                .field("name", name)
                .finish(),
            Request::MoveDocument { document, folder } => f
                .debug_struct("MoveDocument")
                .field("document", document)
                .field("folder", folder)
                .finish(),
            Request::TrashDocument { project, document } => f
                .debug_struct("TrashDocument")
                .field("project", project)
                .field("document", document)
                .finish(),
            Request::RestoreDocument { document } => f
                .debug_struct("RestoreDocument")
                .field("document", document)
                .finish(),
            Request::RenameFolder {
                project,
                folder,
                name,
            } => f
                .debug_struct("RenameFolder")
                .field("project", project)
                .field("folder", folder)
                .field("name", name)
                .finish(),
            Request::DeleteFolder { project, folder } => f
                .debug_struct("DeleteFolder")
                .field("project", project)
                .field("folder", folder)
                .finish(),
        }
    }
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
            Request::LoadPending => "load_pending",
            Request::DiscardPending { .. } => "discard_pending",
            Request::LoadProjectMembers { .. } => "load_project_members",
            Request::SetProjectMember { .. } => "set_project_member",
            Request::RemoveProjectMember { .. } => "remove_project_member",
            Request::LoadOrgMembers { .. } => "load_org_members",
            Request::SetOrgMember { .. } => "set_org_member",
            Request::RemoveOrgMember { .. } => "remove_org_member",
            Request::LoadUsage { .. } => "load_usage",
            Request::RenameDocument { .. } => "rename_document",
            Request::MoveDocument { .. } => "move_document",
            Request::TrashDocument { .. } => "trash_document",
            Request::RestoreDocument { .. } => "restore_document",
            Request::RenameFolder { .. } => "rename_folder",
            Request::DeleteFolder { .. } => "delete_folder",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Signed in (or account restored at start): who, where, which orgs.
    SignedIn {
        server_url: String,
        server_name: String,
        me: proto::Me,
    },
    SignedOut,
    Projects {
        org: String,
        projects: Vec<proto::Project>,
    },
    Tree(proto::ProjectTree),
    /// A working copy (or read-only version copy) is on disk; the app opens `path`.
    DocumentReady {
        path: PathBuf,
        link: Link,
        read_only: bool,
        from_cache: bool,
    },
    /// A version was saved; `link.base_version` is now `version.number`.
    VersionSaved {
        link: Link,
        version: proto::Version,
    },
    /// The head moved since the working copy's base version.
    Conflict {
        local_path: PathBuf,
        document: String,
        base_version: u64,
        head_version: u64,
    },
    /// Another user holds the edit lock.
    Locked {
        document: String,
        lock: proto::Lock,
    },
    LockAcquired {
        document: String,
        lock: proto::Lock,
    },
    LockReleased {
        document: String,
    },
    Detail(proto::DocumentDetail),
    ShareCreated(proto::CreateShareResponse),
    Comments {
        document: String,
        comments: Vec<proto::Comment>,
    },
    /// The server is unreachable; `pending` versions wait in the offline queue.
    Offline {
        pending: usize,
    },
    /// Queue state after a sync attempt.
    Synced {
        pending: usize,
    },
    /// The offline queue, oldest first (answer to `LoadPending` and `DiscardPending`).
    PendingQueue {
        saves: Vec<PendingSave>,
    },
    /// Explicit members of a project (org admins are project admins without being listed).
    ProjectMembers {
        project: String,
        members: Vec<proto::ProjectMember>,
    },
    OrgMembers {
        org: String,
        members: Vec<proto::OrgMember>,
    },
    Usage {
        org: String,
        usage: proto::OrgUsage,
    },
    Error {
        request: &'static str,
        code: Option<proto::ErrorCode>,
        message: String,
    },
}

/// Handle owned by the UI.
pub struct CloudService {
    pub(crate) tx: Sender<Request>,
    pub(crate) rx: Receiver<Event>,
}

impl CloudService {
    /// Start the service thread. `config_path` is the account file
    /// ([`crate::config::default_path`]) and `cache_root` the cache directory
    /// ([`crate::cache::default_root`]). Nothing is read or sent until the first request.
    pub fn start(config_path: PathBuf, cache_root: PathBuf) -> CloudService {
        let (tx, req_rx) = std::sync::mpsc::channel::<Request>();
        let (ev_tx, rx) = std::sync::mpsc::channel();
        let thread_tx = ev_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("rcad-cloud".into())
            .spawn(move || {
                let mut worker = Worker::new(config_path, cache_root);
                for req in req_rx {
                    let name = req.name();
                    let mut emit = |e: Event| {
                        let _ = thread_tx.send(e);
                    };
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        worker.handle(req, &mut emit)
                    }));
                    if outcome.is_err() {
                        emit(Event::Error {
                            request: name,
                            code: None,
                            message: "internal error in the cloud service".into(),
                        });
                    }
                }
            });
        if let Err(e) = spawned {
            let _ = ev_tx.send(Event::Error {
                request: "start",
                code: None,
                message: format!("cannot start the cloud service: {e}"),
            });
        }
        CloudService { tx, rx }
    }

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

// -------------------------------------------------------------------------------------------------
// Implementation

/// The signed-in account with its client and cache.
struct Session {
    account: Account,
    client: Client,
    cache: ServerCache,
}

/// Synchronous request handler behind [`CloudService`]; also used directly by `rcad-cli`.
pub struct Worker {
    config_path: PathBuf,
    cache_root: PathBuf,
}

/// `R-CAD on <hostname>` (standard library only: `HOSTNAME`, `/etc/hostname`, `COMPUTERNAME`).
pub fn device_name() -> String {
    let from_env = |k: &str| std::env::var(k).ok().map(|s| s.trim().to_string());
    let host = from_env("HOSTNAME")
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .or_else(|| from_env("COMPUTERNAME"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "computer".into());
    let host: String = host.chars().filter(|c| !c.is_control()).take(64).collect();
    format!("R-CAD on {host}")
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Errors a queued save can never recover from by retrying.
fn permanent(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::NotFound
            | ErrorCode::Forbidden
            | ErrorCode::TooLarge
            | ErrorCode::BadRequest
            | ErrorCode::HashMismatch
            | ErrorCode::Gone
    )
}

fn unknown_lock() -> proto::Lock {
    proto::Lock {
        holder: "another user".into(),
        since: 0,
        expires_at: 0,
    }
}

/// What a version upload attempt ended with, when it did not fail outright.
enum Pushed {
    Saved(Box<(Link, proto::Version)>),
    Conflict { head_version: u64 },
    Locked(proto::Lock),
}

impl Worker {
    pub fn new(config_path: PathBuf, cache_root: PathBuf) -> Worker {
        Worker {
            config_path,
            cache_root,
        }
    }

    /// The account file in use.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// The active account, if any (re-read from disk).
    pub fn active_account(&self) -> Result<Option<Account>, CloudError> {
        Ok(CloudConfig::load(&self.config_path)?
            .active_account()
            .cloned())
    }

    /// The active account's cache, if signed in.
    pub fn cache(&self) -> Result<Option<ServerCache>, CloudError> {
        Ok(self
            .active_account()?
            .map(|a| ServerCache::open(&self.cache_root, &a.server_url, &a.username)))
    }

    fn session(&self) -> Result<Session, CloudError> {
        let account = self.active_account()?.ok_or(CloudError::NotSignedIn)?;
        let client = Client::new(&account.server_url)?.with_token(account.token.clone());
        let cache = ServerCache::open(&self.cache_root, &account.server_url, &account.username);
        Ok(Session {
            account,
            client,
            cache,
        })
    }

    /// Number of queued saves of the active account (0 when signed out or unreadable).
    pub fn pending_count(&self) -> usize {
        self.cache()
            .ok()
            .flatten()
            .and_then(|c| c.pending().ok())
            .map_or(0, |q| q.len())
    }

    /// Run one request, reporting its events through `emit`.
    pub fn handle(&mut self, req: Request, emit: &mut dyn FnMut(Event)) {
        let name = req.name();
        let sign_in = matches!(req, Request::SignIn { .. });
        if let Err(e) = self.dispatch(req, emit) {
            match e {
                CloudError::Offline(m) if sign_in => emit(Event::Error {
                    request: name,
                    code: None,
                    message: format!("server unreachable: {m}"),
                }),
                CloudError::Offline(_) => emit(Event::Offline {
                    pending: self.pending_count(),
                }),
                CloudError::Api(api) => emit(Event::Error {
                    request: name,
                    code: Some(api.code),
                    message: api.message,
                }),
                other => emit(Event::Error {
                    request: name,
                    code: None,
                    message: other.to_string(),
                }),
            }
        }
    }

    fn dispatch(&mut self, req: Request, emit: &mut dyn FnMut(Event)) -> Result<(), CloudError> {
        match req {
            Request::SignIn {
                server_url,
                username,
                password,
                allow_insecure_http,
            } => self.sign_in(&server_url, &username, &password, allow_insecure_http, emit),
            Request::SignOut => self.sign_out(emit),
            Request::Refresh => {
                let s = self.session()?;
                let me = s.client.me()?;
                emit(Event::SignedIn {
                    server_url: s.account.server_url,
                    server_name: s.account.server_name,
                    me,
                });
                Ok(())
            }
            Request::ListProjects { org } => {
                let s = self.session()?;
                let projects = s.client.projects(&org)?;
                emit(Event::Projects { org, projects });
                Ok(())
            }
            Request::CreateProject { org, name } => {
                let s = self.session()?;
                s.client.create_project(
                    &org,
                    &proto::CreateProjectRequest {
                        name,
                        description: String::new(),
                    },
                )?;
                let projects = s.client.projects(&org)?;
                emit(Event::Projects { org, projects });
                Ok(())
            }
            Request::LoadTree { project } => {
                let s = self.session()?;
                emit(Event::Tree(s.client.tree(&project)?));
                Ok(())
            }
            Request::CreateFolder {
                project,
                parent,
                name,
            } => {
                let s = self.session()?;
                s.client
                    .create_folder(&project, &proto::CreateFolderRequest { name, parent })?;
                emit(Event::Tree(s.client.tree(&project)?));
                Ok(())
            }
            Request::OpenDocument { document } => {
                let s = self.session()?;
                self.open_document(&s, &document, emit)
            }
            Request::OpenVersion { document, version } => {
                let s = self.session()?;
                self.open_version(&s, &document, version, emit)
            }
            Request::UploadNew {
                local_path,
                project,
                folder,
                name,
            } => {
                let s = self.session()?;
                self.upload_new(&s, &local_path, &project, folder, &name, emit)
            }
            Request::SaveVersion {
                local_path,
                description,
                milestone,
                force,
            } => {
                let s = self.session()?;
                self.save_version(&s, &local_path, description, milestone, force, emit)
            }
            Request::LoadDetail { document } => {
                let s = self.session()?;
                emit(Event::Detail(s.client.document(&document)?));
                Ok(())
            }
            Request::SetMilestone {
                document,
                version,
                milestone,
            } => {
                let s = self.session()?;
                s.client.update_version(
                    &document,
                    version,
                    &proto::UpdateVersionRequest {
                        description: None,
                        milestone: Some(milestone),
                    },
                )?;
                emit(Event::Detail(s.client.document(&document)?));
                Ok(())
            }
            Request::Promote { document, version } => {
                let s = self.session()?;
                let head = s.client.document(&document)?.document.head_version;
                s.client.promote(
                    &document,
                    version,
                    &proto::PromoteRequest {
                        base_version: head,
                        force: false,
                    },
                )?;
                emit(Event::Detail(s.client.document(&document)?));
                Ok(())
            }
            Request::Lock { document } => {
                let s = self.session()?;
                match s.client.lock(&document) {
                    Ok(lock) => emit(Event::LockAcquired { document, lock }),
                    Err(CloudError::Api(e)) if e.code == ErrorCode::Locked => emit(Event::Locked {
                        document,
                        lock: e.lock.unwrap_or_else(unknown_lock),
                    }),
                    Err(e) => return Err(e),
                }
                Ok(())
            }
            Request::Unlock { document } => {
                let s = self.session()?;
                s.client.unlock(&document)?;
                emit(Event::LockReleased { document });
                Ok(())
            }
            Request::CreateShare {
                document,
                version,
                expires_in_days,
                allow_download,
            } => {
                let s = self.session()?;
                let r = s.client.create_share(
                    &document,
                    &proto::CreateShareRequest {
                        version,
                        expires_in_days,
                        allow_download,
                    },
                )?;
                emit(Event::ShareCreated(r));
                Ok(())
            }
            Request::LoadComments { document } => {
                let s = self.session()?;
                let comments = s.client.comments(&document)?;
                emit(Event::Comments { document, comments });
                Ok(())
            }
            Request::AddComment {
                document,
                version,
                body,
            } => {
                let s = self.session()?;
                s.client.create_comment(
                    &document,
                    &proto::CreateCommentRequest {
                        body,
                        version,
                        parent: None,
                        anchor: None,
                    },
                )?;
                let comments = s.client.comments(&document)?;
                emit(Event::Comments { document, comments });
                Ok(())
            }
            Request::ResolveComment {
                document,
                comment,
                resolved,
            } => {
                let s = self.session()?;
                s.client.update_comment(
                    &comment,
                    &proto::UpdateCommentRequest {
                        body: None,
                        resolved: Some(resolved),
                    },
                )?;
                let comments = s.client.comments(&document)?;
                emit(Event::Comments { document, comments });
                Ok(())
            }
            Request::SyncPending => {
                let s = self.session()?;
                self.sync_pending(&s, None, emit)
            }
            Request::LoadPending => {
                let cache = self.cache()?.ok_or(CloudError::NotSignedIn)?;
                emit(Event::PendingQueue {
                    saves: cache.pending()?,
                });
                Ok(())
            }
            Request::DiscardPending {
                index,
                document,
                blob,
            } => {
                let cache = self.cache()?.ok_or(CloudError::NotSignedIn)?;
                let (removed, queue) = cache.update_pending(|queue| {
                    let ok = matches!(queue.get(index),
                        Some(p) if p.document == document && p.blob == blob);
                    if ok {
                        queue.remove(index);
                    }
                    (ok, queue.clone())
                })?;
                emit(Event::PendingQueue { saves: queue });
                if !removed {
                    return Err(CloudError::Invalid(
                        "the queue changed since it was listed; nothing was discarded".into(),
                    ));
                }
                Ok(())
            }
            Request::LoadProjectMembers { project } => {
                let s = self.session()?;
                let members = s.client.project_members(&project)?;
                emit(Event::ProjectMembers { project, members });
                Ok(())
            }
            Request::SetProjectMember {
                project,
                username,
                role,
            } => {
                let s = self.session()?;
                s.client.set_project_member(
                    &project,
                    &proto::SetProjectMemberRequest { username, role },
                )?;
                let members = s.client.project_members(&project)?;
                emit(Event::ProjectMembers { project, members });
                Ok(())
            }
            Request::RemoveProjectMember { project, username } => {
                let s = self.session()?;
                s.client.remove_project_member(&project, &username)?;
                let members = s.client.project_members(&project)?;
                emit(Event::ProjectMembers { project, members });
                Ok(())
            }
            Request::LoadOrgMembers { org } => {
                let s = self.session()?;
                let members = s.client.org_members(&org)?;
                emit(Event::OrgMembers { org, members });
                Ok(())
            }
            Request::SetOrgMember {
                org,
                username,
                role,
            } => {
                let s = self.session()?;
                s.client
                    .set_org_member(&org, &proto::SetOrgMemberRequest { username, role })?;
                let members = s.client.org_members(&org)?;
                emit(Event::OrgMembers { org, members });
                Ok(())
            }
            Request::RemoveOrgMember { org, username } => {
                let s = self.session()?;
                s.client.remove_org_member(&org, &username)?;
                let members = s.client.org_members(&org)?;
                emit(Event::OrgMembers { org, members });
                Ok(())
            }
            Request::LoadUsage { org } => {
                let s = self.session()?;
                let usage = s.client.org_usage(&org)?;
                emit(Event::Usage { org, usage });
                Ok(())
            }
            Request::RenameDocument { document, name } => {
                let s = self.session()?;
                let summary = s.client.update_document(
                    &document,
                    &proto::UpdateDocumentRequest {
                        name: Some(name),
                        folder: None,
                    },
                )?;
                self.rename_links(&s, &summary)?;
                emit(Event::Tree(s.client.tree(&summary.project)?));
                Ok(())
            }
            Request::MoveDocument { document, folder } => {
                let s = self.session()?;
                let summary = s.client.update_document(
                    &document,
                    &proto::UpdateDocumentRequest {
                        name: None,
                        folder: Some(folder),
                    },
                )?;
                emit(Event::Tree(s.client.tree(&summary.project)?));
                Ok(())
            }
            Request::TrashDocument { project, document } => {
                let s = self.session()?;
                s.client.delete_document(&document)?;
                emit(Event::Tree(s.client.tree(&project)?));
                Ok(())
            }
            Request::RestoreDocument { document } => {
                let s = self.session()?;
                let summary = s.client.restore_document(&document)?;
                emit(Event::Tree(s.client.tree(&summary.project)?));
                Ok(())
            }
            Request::RenameFolder {
                project,
                folder,
                name,
            } => {
                let s = self.session()?;
                s.client.update_folder(
                    &folder,
                    &proto::UpdateFolderRequest {
                        name: Some(name),
                        parent: None,
                    },
                )?;
                emit(Event::Tree(s.client.tree(&project)?));
                Ok(())
            }
            Request::DeleteFolder { project, folder } => {
                let s = self.session()?;
                s.client.delete_folder(&folder)?;
                emit(Event::Tree(s.client.tree(&project)?));
                Ok(())
            }
        }
    }

    /// Keep the cached document name of links to a renamed document current.
    fn rename_links(
        &self,
        s: &Session,
        summary: &proto::DocumentSummary,
    ) -> Result<(), CloudError> {
        for link in s.cache.links()? {
            if link.document == summary.id && link.document_name != summary.name {
                s.cache.upsert_link(Link {
                    document_name: summary.name.clone(),
                    ..link
                })?;
            }
        }
        Ok(())
    }

    fn sign_in(
        &mut self,
        server_url: &str,
        username: &str,
        password: &str,
        allow_insecure_http: bool,
        emit: &mut dyn FnMut(Event),
    ) -> Result<(), CloudError> {
        let client = Client::new(server_url)?;
        if !allow_insecure_http && crate::client::needs_insecure_consent(client.server_url()) {
            return Err(CloudError::Invalid(format!(
                "refusing to sign in to {} without encryption: {} To connect anyway, allow \
                 the unencrypted connection explicitly.",
                client.server_url(),
                crate::client::INSECURE_HTTP_WARNING
            )));
        }
        let info = client.info()?;
        if info.product != proto::PRODUCT {
            return Err(CloudError::Invalid(format!(
                "{} is not an R-CAD server (it reports product {:?})",
                client.server_url(),
                info.product
            )));
        }
        if info.protocol != proto::PROTOCOL_VERSION {
            return Err(CloudError::Invalid(format!(
                "the server speaks protocol version {}, this build speaks version {}; update {}",
                info.protocol,
                proto::PROTOCOL_VERSION,
                if info.protocol > proto::PROTOCOL_VERSION {
                    "R-CAD"
                } else {
                    "the server"
                }
            )));
        }
        if !info.local_login {
            return Err(CloudError::Invalid(
                "this server does not offer username and password sign-in".into(),
            ));
        }
        let login = client.login(&proto::LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            device_name: device_name(),
        })?;
        let authed = Client::new(client.server_url())?.with_token(login.token.clone());
        let me = authed.me()?;
        let account = Account {
            server_url: client.server_url().to_string(),
            server_name: info.name.clone(),
            username: login.user.username.clone(),
            token: login.token,
            max_blob_bytes: Some(info.max_blob_bytes).filter(|&n| n > 0),
        };
        CloudConfig::update(&self.config_path, |c| c.upsert(account))?;
        emit(Event::SignedIn {
            server_url: client.server_url().to_string(),
            server_name: info.name,
            me,
        });
        Ok(())
    }

    fn sign_out(&mut self, emit: &mut dyn FnMut(Event)) -> Result<(), CloudError> {
        if let Some(account) = self.active_account()? {
            // Best effort: an unreachable server still forgets the account locally.
            if let Ok(c) = Client::new(&account.server_url) {
                let _ = c.with_token(account.token.clone()).logout();
            }
            // By identity: the active index may have changed while the logout ran.
            CloudConfig::update(&self.config_path, |c| {
                c.remove_account(&account.server_url, &account.username)
            })?;
        }
        emit(Event::SignedOut);
        Ok(())
    }

    /// Bytes of `version` of `document`: from the blob cache, else downloaded into it. The
    /// download is refused when the declared size exceeds the account's limit, and cut off
    /// when the server sends more than it declared.
    fn ensure_blob(
        &self,
        s: &Session,
        document: &str,
        version: &proto::Version,
    ) -> Result<Vec<u8>, CloudError> {
        let blob = version.blob.as_str();
        if !proto::is_blob_hash(blob) {
            return Err(CloudError::Protocol(format!(
                "server sent an invalid blob hash {blob:.80}"
            )));
        }
        if s.cache.has_blob(blob)
            && let Ok(bytes) = s.cache.read_blob(blob)
        {
            return Ok(bytes);
        }
        let limit = s
            .account
            .max_blob_bytes
            .unwrap_or(crate::client::DEFAULT_MAX_DOWNLOAD_BYTES);
        if version.size > limit {
            return Err(CloudError::Invalid(format!(
                "version {} is {} bytes, more than the {limit}-byte limit for downloads from \
                 this server",
                version.number, version.size
            )));
        }
        let dest = s.cache.blob_path(blob)?;
        s.client
            .download_content(document, version.number, Some(blob), version.size, &dest)?;
        s.cache.read_blob(blob)
    }

    fn org_of(
        &self,
        s: &Session,
        project: &str,
        known: Option<&Link>,
    ) -> Result<String, CloudError> {
        match known {
            Some(l) if l.project == project && !l.org.is_empty() => Ok(l.org.clone()),
            _ => Ok(s.client.project(project)?.org),
        }
    }

    fn open_document(
        &self,
        s: &Session,
        document: &str,
        emit: &mut dyn FnMut(Event),
    ) -> Result<(), CloudError> {
        let existing = s.cache.link_for_document(document)?;
        let detail = match s.client.document(document) {
            Ok(d) => d,
            Err(CloudError::Offline(m)) => {
                let pending = s.cache.pending().map_or(0, |q| q.len());
                match existing.filter(|l| l.local_path.is_file()) {
                    Some(link) => emit(Event::DocumentReady {
                        path: link.local_path.clone(),
                        link,
                        read_only: false,
                        from_cache: true,
                    }),
                    None => emit(Event::Error {
                        request: "open_document",
                        code: None,
                        message: format!("server unreachable ({m}) and no copy is cached"),
                    }),
                }
                emit(Event::Offline { pending });
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let head = detail.document.head_version;
        if head == 0 {
            return Err(CloudError::Invalid(format!(
                "{} has no saved version yet",
                detail.document.name
            )));
        }
        let version = detail
            .versions
            .iter()
            .find(|v| v.number == head)
            .ok_or_else(|| {
                CloudError::Protocol(format!("head version {head} missing from detail"))
            })?;

        // Keep a working copy with local modifications.
        let live = existing.as_ref().filter(|l| l.local_path.is_file());
        if let Some(link) = live {
            let current = crate::blob_hash(&std::fs::read(&link.local_path)?);
            if link.base_blob.as_deref() != Some(current.as_str()) {
                emit(Event::DocumentReady {
                    path: link.local_path.clone(),
                    link: link.clone(),
                    read_only: false,
                    from_cache: false,
                });
                if head > link.base_version {
                    emit(Event::Conflict {
                        local_path: link.local_path.clone(),
                        document: document.to_string(),
                        base_version: link.base_version,
                        head_version: head,
                    });
                }
                return Ok(());
            }
        }

        let bytes = self.ensure_blob(s, document, version)?;
        let target = match live {
            Some(l) => l.local_path.clone(),
            None => s.cache.working_copy_path(document, &detail.document.name),
        };
        fsutil::write_atomic(&target, &bytes, false)?;
        let org = self.org_of(s, &detail.document.project, existing.as_ref())?;
        if let Some(old) = existing.as_ref().filter(|_| live.is_none()) {
            s.cache.remove_link(&old.local_path)?;
        }
        let link = s.cache.upsert_link(Link {
            local_path: target,
            server_url: s.account.server_url.clone(),
            org,
            project: detail.document.project.clone(),
            document: document.to_string(),
            document_name: detail.document.name.clone(),
            base_version: head,
            base_blob: Some(version.blob.clone()),
        })?;
        emit(Event::DocumentReady {
            path: link.local_path.clone(),
            link,
            read_only: false,
            from_cache: false,
        });
        Ok(())
    }

    /// A cached read-only copy of `version` (any name), for offline use.
    fn cached_version_copy(&self, s: &Session, document: &str, version: u64) -> Option<PathBuf> {
        let dir = s.cache.version_copy_path(document, version, "x");
        let dir = dir.parent()?;
        let prefix = format!("v{version}-");
        std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
            })
    }

    fn open_version(
        &self,
        s: &Session,
        document: &str,
        number: u64,
        emit: &mut dyn FnMut(Event),
    ) -> Result<(), CloudError> {
        let existing = s.cache.link_for_document(document)?;
        let detail = match s.client.document(document) {
            Ok(d) => d,
            Err(CloudError::Offline(m)) => {
                let pending = s.cache.pending().map_or(0, |q| q.len());
                match (self.cached_version_copy(s, document, number), existing) {
                    (Some(path), Some(link)) => emit(Event::DocumentReady {
                        link: Link {
                            local_path: path.clone(),
                            base_version: number,
                            ..link
                        },
                        path,
                        read_only: true,
                        from_cache: true,
                    }),
                    _ => emit(Event::Error {
                        request: "open_version",
                        code: None,
                        message: format!(
                            "server unreachable ({m}) and version {number} is not cached"
                        ),
                    }),
                }
                emit(Event::Offline { pending });
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let version = detail
            .versions
            .iter()
            .find(|v| v.number == number)
            .ok_or_else(|| {
                CloudError::Api(proto::ApiError {
                    code: ErrorCode::NotFound,
                    message: format!("{} has no version {number}", detail.document.name),
                    head_version: None,
                    lock: None,
                })
            })?;
        let bytes = self.ensure_blob(s, document, version)?;
        let path = s
            .cache
            .version_copy_path(document, number, &detail.document.name);
        fsutil::write_atomic(&path, &bytes, false)?;
        let org = self.org_of(s, &detail.document.project, existing.as_ref())?;
        let link = Link {
            local_path: path.clone(),
            server_url: s.account.server_url.clone(),
            org,
            project: detail.document.project.clone(),
            document: document.to_string(),
            document_name: detail.document.name.clone(),
            base_version: number,
            base_blob: Some(version.blob.clone()),
        };
        emit(Event::DocumentReady {
            path,
            link,
            read_only: true,
            from_cache: false,
        });
        Ok(())
    }

    fn upload_new(
        &self,
        s: &Session,
        local_path: &Path,
        project: &str,
        folder: Option<String>,
        name: &str,
        emit: &mut dyn FnMut(Event),
    ) -> Result<(), CloudError> {
        let bytes = std::fs::read(local_path).map_err(|e| {
            CloudError::Invalid(format!("cannot read {}: {e}", local_path.display()))
        })?;
        let org = s.client.project(project)?.org;
        let summary = s.client.create_document(
            project,
            &proto::CreateDocumentRequest {
                name: name.to_string(),
                folder,
            },
        )?;
        let hash = s.cache.store_blob(&bytes)?;
        // Link first: if the first version cannot be saved now, a later SaveVersion (or the
        // offline queue) retries it against base version 0.
        let link = s.cache.upsert_link(Link {
            local_path: local_path.to_path_buf(),
            server_url: s.account.server_url.clone(),
            org,
            project: project.to_string(),
            document: summary.id.clone(),
            document_name: summary.name.clone(),
            base_version: 0,
            base_blob: None,
        })?;
        let save = PendingSave {
            local_path: link.local_path.clone(),
            document: link.document.clone(),
            blob: hash,
            description: String::new(),
            milestone: None,
            base_version: 0,
            chained: false,
            force: false,
            queued_at: now(),
        };
        self.push_or_queue(s, &link, save, emit)
    }

    fn save_version(
        &self,
        s: &Session,
        local_path: &Path,
        description: String,
        milestone: Option<String>,
        force: bool,
        emit: &mut dyn FnMut(Event),
    ) -> Result<(), CloudError> {
        let link = s.cache.link_for_path(local_path)?.ok_or_else(|| {
            CloudError::Invalid(format!(
                "{} is not linked to a cloud document on {}",
                local_path.display(),
                s.account.server_url
            ))
        })?;
        let bytes = std::fs::read(&link.local_path)?;
        let hash = s.cache.store_blob(&bytes)?;
        let queued = s
            .cache
            .pending()?
            .iter()
            .any(|p| p.document == link.document);
        let save = PendingSave {
            local_path: link.local_path.clone(),
            document: link.document.clone(),
            blob: hash,
            description,
            milestone,
            base_version: link.base_version,
            chained: queued,
            force,
            queued_at: now(),
        };
        if queued {
            // Keep the order: this save goes behind the document's queued ones.
            s.cache.enqueue(save)?;
            let force_doc = force.then_some(link.document.as_str());
            return self.sync_pending(s, force_doc, emit);
        }
        self.push_or_queue(s, &link, save, emit)
    }

    /// Try `save` now; queue it when the server is unreachable.
    fn push_or_queue(
        &self,
        s: &Session,
        link: &Link,
        save: PendingSave,
        emit: &mut dyn FnMut(Event),
    ) -> Result<(), CloudError> {
        let base = save.base_version;
        match self.push_version(s, link, &save, base, save.force) {
            Ok(Pushed::Saved(saved)) => {
                let (link, version) = *saved;
                emit(Event::VersionSaved { link, version })
            }
            Ok(Pushed::Conflict { head_version }) => emit(Event::Conflict {
                local_path: link.local_path.clone(),
                document: link.document.clone(),
                base_version: base,
                head_version,
            }),
            Ok(Pushed::Locked(lock)) => emit(Event::Locked {
                document: link.document.clone(),
                lock,
            }),
            Err(CloudError::Offline(_)) => {
                let pending = s.cache.enqueue(save)?;
                emit(Event::Offline { pending });
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Upload the blob if the organization lacks it and create the version.
    fn push_version(
        &self,
        s: &Session,
        link: &Link,
        save: &PendingSave,
        base: u64,
        force: bool,
    ) -> Result<Pushed, CloudError> {
        let blob = &save.blob;
        let missing = s
            .client
            .missing_blobs(&link.org, std::slice::from_ref(blob))?;
        if missing.iter().any(|m| m == blob) {
            let bytes = s.cache.read_blob(blob)?;
            s.client.upload_blob(&link.org, blob, &bytes)?;
        }
        let req = proto::CreateVersionRequest {
            blob: blob.clone(),
            base_version: base,
            description: save.description.clone(),
            milestone: save.milestone.clone(),
            force,
            generator: crate::generator(),
        };
        let mut created = s.client.create_version(&link.document, &req);
        if matches!(&created, Err(CloudError::Api(e)) if e.code == ErrorCode::HashMismatch)
            && let Ok(bytes) = s.cache.read_blob(blob)
        {
            // The server lost the blob between the upload check and the save (for example
            // to garbage collection): upload it again and retry once.
            s.client.upload_blob(&link.org, blob, &bytes)?;
            created = s.client.create_version(&link.document, &req);
        }
        let version = match created {
            Ok(v) => v,
            Err(CloudError::Api(e)) if e.code == ErrorCode::VersionConflict => {
                // A save whose answer was lost: the head already holds these bytes.
                let detail = s.client.document(&link.document)?;
                let head = detail.document.head_version;
                match detail.versions.into_iter().find(|v| v.number == head) {
                    Some(v) if v.blob == *blob => v,
                    _ => {
                        return Ok(Pushed::Conflict {
                            head_version: e.head_version.unwrap_or(head),
                        });
                    }
                }
            }
            Err(CloudError::Api(e)) if e.code == ErrorCode::Locked => {
                return Ok(Pushed::Locked(e.lock.unwrap_or_else(unknown_lock)));
            }
            Err(e) => return Err(e),
        };
        let updated = s.cache.upsert_link(Link {
            base_version: version.number,
            base_blob: Some(blob.clone()),
            ..link.clone()
        })?;
        Ok(Pushed::Saved(Box::new((updated, version))))
    }

    /// Replay the offline queue in order (see the module documentation). The queue is
    /// re-read before each entry and entries are removed by value, so saves another process
    /// queues meanwhile are kept (and sent too).
    fn sync_pending(
        &self,
        s: &Session,
        force_doc: Option<&str>,
        emit: &mut dyn FnMut(Event),
    ) -> Result<(), CloudError> {
        // Bound the loop even if another process keeps adding entries.
        for _ in 0..100_000 {
            let Some(item) = s.cache.pending()?.into_iter().next() else {
                break;
            };
            let link = s
                .cache
                .link_for_path(&item.local_path)?
                .filter(|l| l.document == item.document);
            let problem = match &link {
                None => Some(format!(
                    "{} is no longer linked to that document",
                    item.local_path.display()
                )),
                Some(_) if !s.cache.has_blob(&item.blob) => {
                    Some("its snapshot is missing from the cache".to_string())
                }
                Some(_) => None,
            };
            let Some(link) = link.filter(|_| problem.is_none()) else {
                s.cache.remove_pending(&item)?;
                emit(Event::Error {
                    request: "sync_pending",
                    code: None,
                    message: format!(
                        "dropped a queued save of {}: {}",
                        item.local_path.display(),
                        problem.unwrap_or_default()
                    ),
                });
                continue;
            };
            let base = if item.chained {
                link.base_version
            } else {
                item.base_version
            };
            let force = item.force || force_doc == Some(item.document.as_str());
            match self.push_version(s, &link, &item, base, force) {
                Ok(Pushed::Saved(saved)) => {
                    let (link, version) = *saved;
                    s.cache.remove_pending(&item)?;
                    emit(Event::VersionSaved { link, version });
                }
                Ok(Pushed::Conflict { head_version }) => {
                    emit(Event::Conflict {
                        local_path: link.local_path.clone(),
                        document: link.document.clone(),
                        base_version: base,
                        head_version,
                    });
                    break;
                }
                Ok(Pushed::Locked(lock)) => {
                    emit(Event::Locked {
                        document: link.document.clone(),
                        lock,
                    });
                    break;
                }
                Err(CloudError::Offline(_)) => {
                    emit(Event::Offline {
                        pending: s.cache.pending()?.len(),
                    });
                    return Ok(());
                }
                Err(CloudError::Api(e)) if permanent(e.code) => {
                    s.cache.remove_pending(&item)?;
                    emit(Event::Error {
                        request: "sync_pending",
                        code: Some(e.code),
                        message: format!(
                            "dropped a queued save of {}: {}",
                            item.local_path.display(),
                            e.message
                        ),
                    });
                }
                Err(e) => {
                    emit(Event::Error {
                        request: "sync_pending",
                        code: e.code(),
                        message: e.to_string(),
                    });
                    break;
                }
            }
        }
        emit(Event::Synced {
            pending: s.cache.pending()?.len(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_password() {
        let r = Request::SignIn {
            server_url: "https://x".into(),
            username: "ada".into(),
            password: "hunter2".into(),
            allow_insecure_http: false,
        };
        let text = format!("{r:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("ada") && text.contains("<redacted>"));
        assert_eq!(format!("{:?}", Request::SignOut), "SignOut");
        let t = format!(
            "{:?}",
            Request::Lock {
                document: "d1".into()
            }
        );
        assert_eq!(t, r#"Lock { document: "d1" }"#);
    }
}
