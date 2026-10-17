//! Optional cloud tier: the Data panel, the cloud status chip on the
//! application bar, and the cloud dialogs (sign in, save version, version
//! history, share link, comments, conflicts). Everything goes through the
//! background [`CloudService`] of `rcad-cloud`: the UI sends [`Request`]s and
//! polls [`Event`]s once per frame, so it never waits on the network.
//!
//! Local-first rules this module keeps:
//! * With no account configured nothing is constructed at startup: no
//!   service thread, no socket, no prompt. The Data panel (application bar
//!   "Data") explains what a server adds and offers "Connect to server…".
//! * A cloud document is an ordinary local file (its working copy). Local
//!   editing and local Save always work; a lock held by someone else or an
//!   unreachable server only shows in the chip and makes Save Version fail.
//! * Messages never contain tokens or passwords.
//!
//! The application wires in through a few hooks: [`RcadApp::cloud_tick`]
//! (each frame), [`RcadApp::cloud_on_reply`] (engine replies: a local save
//! that a version or upload waits for), the panel, app-bar and window
//! drawing functions, and the registry commands in File.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use rcad_cloud::cache::Link;
use rcad_cloud::proto;
use rcad_cloud::service::{CloudService, Event, Request};
use rcad_commands::Command;

use crate::app::{Guarded, RcadApp};
use crate::probe::Probe as _;

mod admin;
pub mod thumb;
pub use admin::{fmt_bytes, member_error_text, org_role_name, project_role_name};

/// The calls into `rcad-cloud` that construct things (account file, cache
/// directory, service thread), kept in one place.
pub mod adapter {
    use super::CloudService;
    use std::path::{Path, PathBuf};

    /// The account file (`<config>/r-cad/cloud.json`, next to `prefs.json`).
    pub fn config_path() -> Option<PathBuf> {
        rcad_cloud::config::CloudConfig::default_path()
    }

    /// The cache root (`$XDG_CACHE_HOME/r-cad/cloud`, else `~/.cache/r-cad/cloud`).
    pub fn cache_root() -> Option<PathBuf> {
        rcad_cloud::cache::default_root()
    }

    /// Whether the account file lists at least one account. Reads one small
    /// file; a missing or unreadable file means no account.
    pub fn has_account(config: &Path) -> bool {
        rcad_cloud::config::CloudConfig::load(config)
            .map(|c| !c.accounts.is_empty())
            .unwrap_or(false)
    }

    /// Start the service thread.
    pub fn start(config: PathBuf, cache_root: PathBuf) -> Option<CloudService> {
        Some(CloudService::start(config, cache_root))
    }
}

/// The signed-in account as the service reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub server_url: String,
    pub server_name: String,
    pub me: proto::Me,
}

/// Open cloud dialog (one at a time).
#[derive(Debug, Clone, PartialEq)]
pub enum CloudDialog {
    SignIn {
        server_url: String,
        username: String,
        password: String,
        /// "Allow unencrypted connection": consent to plain http:// to a host that is not
        /// on a local network (only offered for such a URL).
        allow_insecure: bool,
        error: Option<String>,
        sent: bool,
    },
    NewProject {
        name: String,
    },
    NewFolder {
        name: String,
        parent: Option<String>,
    },
    Upload {
        name: String,
        folder: Option<String>,
    },
    SaveVersion {
        description: String,
        milestone: String,
    },
    Share {
        days: String,
        allow_download: bool,
        version: Option<u64>,
        sent: bool,
    },
    Promote {
        document: String,
        version: u64,
    },
    Milestone {
        document: String,
        version: u64,
        name: String,
    },
    /// Project members (everyone with view rights; editing for project admins).
    Members {
        project: String,
        username: String,
        role: proto::ProjectRole,
        error: Option<String>,
    },
    /// Organization members and storage (organization admins and owners).
    Organization {
        org: String,
        username: String,
        role: proto::OrgRole,
        error: Option<String>,
    },
    RenameDocument {
        document: String,
        name: String,
    },
    MoveDocument {
        document: String,
        folder: Option<String>,
    },
    TrashDocument {
        project: String,
        document: String,
        name: String,
    },
    RenameFolder {
        folder: String,
        name: String,
    },
    DeleteFolder {
        folder: String,
        name: String,
    },
    /// Confirm discarding a queued version.
    DiscardPending {
        index: usize,
        document: String,
        blob: String,
        what: String,
    },
    /// File ▸ Save to Cloud Project… for a document that is not on the server.
    SaveToCloud {
        project: Option<String>,
        folder: Option<String>,
        name: String,
    },
}

impl CloudDialog {
    /// Short machine name (live-test probe).
    pub fn key(&self) -> &'static str {
        match self {
            CloudDialog::SignIn { .. } => "sign_in",
            CloudDialog::NewProject { .. } => "new_project",
            CloudDialog::NewFolder { .. } => "new_folder",
            CloudDialog::Upload { .. } => "upload",
            CloudDialog::SaveVersion { .. } => "save_version",
            CloudDialog::Share { .. } => "share",
            CloudDialog::Promote { .. } => "promote",
            CloudDialog::Milestone { .. } => "milestone",
            CloudDialog::Members { .. } => "members",
            CloudDialog::Organization { .. } => "organization",
            CloudDialog::RenameDocument { .. } => "rename_document",
            CloudDialog::MoveDocument { .. } => "move_document",
            CloudDialog::TrashDocument { .. } => "trash_document",
            CloudDialog::RenameFolder { .. } => "rename_folder",
            CloudDialog::DeleteFolder { .. } => "delete_folder",
            CloudDialog::DiscardPending { .. } => "discard_pending",
            CloudDialog::SaveToCloud { .. } => "save_to_cloud",
        }
    }
}

/// A version save refused because the head moved.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub local_path: PathBuf,
    pub document: String,
    pub base_version: u64,
    pub head_version: u64,
}

/// Data panel section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Browse,
    History,
    Comments,
}

/// State of the open cloud-linked document, as the chip shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncState {
    Synced,
    /// Local edits since the base version.
    Modified,
    Offline {
        pending: usize,
    },
    Conflict,
    Locked {
        holder: String,
    },
    /// A read-only copy of an older version.
    ReadOnly,
    /// The server has a newer version and the working copy is unmodified:
    /// "Update" replaces it.
    UpdateAvailable {
        head: u64,
    },
    /// The server has a newer version and the working copy has local edits:
    /// a Save Version will conflict.
    NewerOnServer {
        head: u64,
    },
}

impl SyncState {
    pub fn text(&self) -> String {
        match self {
            SyncState::Synced => "Synced".into(),
            SyncState::Modified => "Modified".into(),
            SyncState::Offline { pending } => format!("Offline · {pending} pending"),
            SyncState::Conflict => "Conflict".into(),
            SyncState::Locked { holder } => format!("Locked by {holder}"),
            SyncState::ReadOnly => "Read-only".into(),
            SyncState::UpdateAvailable { head } => format!("v{head} available"),
            SyncState::NewerOnServer { head } => format!("Newer v{head} on server"),
        }
    }
    /// Short machine name (live-test probe).
    pub fn key(&self) -> &'static str {
        match self {
            SyncState::Synced => "synced",
            SyncState::Modified => "modified",
            SyncState::Offline { .. } => "offline",
            SyncState::Conflict => "conflict",
            SyncState::Locked { .. } => "locked",
            SyncState::ReadOnly => "read_only",
            SyncState::UpdateAvailable { .. } => "update_available",
            SyncState::NewerOnServer { .. } => "newer_on_server",
        }
    }
}

/// Work waiting for a local save to finish.
#[derive(Debug, Clone, PartialEq)]
enum AfterSave {
    Version {
        path: PathBuf,
        description: String,
        milestone: Option<String>,
    },
    Upload {
        path: PathBuf,
        project: String,
        folder: Option<String>,
        name: String,
    },
}

/// Cloud UI state (view state only; never part of a document).
#[derive(Default)]
pub struct CloudUi {
    service: Option<CloudService>,
    config_path: Option<PathBuf>,
    cache_root: Option<PathBuf>,
    pub(crate) panel_open: bool,
    section: Section,
    account: Option<Account>,
    org: Option<String>,
    projects: Vec<proto::Project>,
    project: Option<String>,
    tree: Option<proto::ProjectTree>,
    selected_doc: Option<String>,
    /// Local working copies and the cloud documents they belong to.
    links: Vec<Link>,
    read_only: HashSet<PathBuf>,
    /// Documents with local edits since their base version.
    edited: HashSet<String>,
    offline: bool,
    pending: usize,
    conflict: Option<Conflict>,
    /// Locks held by other users.
    locked_by: HashMap<String, proto::Lock>,
    /// Locks this client holds, and lock requests in flight.
    my_locks: HashSet<String>,
    lock_sent: HashSet<String>,
    /// Open document path and unsaved state last frame.
    last_path: Option<PathBuf>,
    last_dirty: bool,
    detail: Option<proto::DocumentDetail>,
    comments: Option<(String, Vec<proto::Comment>)>,
    comment_draft: String,
    share_url: Option<String>,
    dialog: Option<CloudDialog>,
    after_save: Option<AfterSave>,
    /// Description and milestone of the last version save (resent on "save anyway").
    last_version: Option<(String, Option<String>)>,
    /// Screenshot scenes: the detached service's request receiver (kept so
    /// sends succeed).
    scene: Option<std::sync::mpsc::Receiver<Request>>,
    /// Folder selected in the tree: the target of New folder and Upload.
    selected_folder: Option<String>,
    /// Browse shows the project's trash instead of its tree.
    show_trash: bool,
    /// Explicit members of a project, as last loaded.
    members: Option<(String, Vec<proto::ProjectMember>)>,
    org_members: Option<(String, Vec<proto::OrgMember>)>,
    usage: Option<(String, proto::OrgUsage)>,
    /// The offline queue, as last listed, and the pending count it was asked for.
    queue: Vec<rcad_cloud::cache::PendingSave>,
    queue_asked: Option<usize>,
    /// Save to Cloud Project… waits for a first local save (Save As).
    save_to_cloud_after_save: bool,
    /// Code of the error shown in the Members or Organization window (probe).
    admin_error_code: Option<String>,
    /// Versions saved or uploaded get a thumbnail rendered from the model
    /// (interactive app only; tests opt in, and without a GPU view there is none).
    capture_thumbnails: bool,
    /// Thumbnails asked for, by (document, version): never asked twice.
    thumb_asked: HashSet<ThumbKey>,
    /// Thumbnail PNGs received and not yet turned into textures.
    thumb_png: Vec<(ThumbKey, Option<Vec<u8>>)>,
    /// Thumbnail textures (None: the version has none, or it did not decode).
    thumbs: HashMap<ThumbKey, Option<egui::TextureHandle>>,
    /// Head versions on the server, as last polled.
    heads: HashMap<String, u64>,
    /// When the next update-available poll is due (None: now).
    poll_due: Option<std::time::Instant>,
    /// Window focus last frame (a regained focus polls at once; None before
    /// the first frame).
    focused: Option<bool>,
}

/// A version of a document: (document id, version number).
type ThumbKey = (String, u64);

/// How often the open cloud document's head (and, with the Data panel open,
/// the project list) is polled.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Panel actions collected while drawing and applied afterwards.
enum Action {
    Send(Request),
    Dialog(CloudDialog),
    SelectOrg(String),
    SelectProject(String),
    SelectDoc(String),
    Open(String),
    Section(Section),
    Upload,
    SignOut,
    Refresh,
    Connect,
    ClosePanel,
    AddComment,
    SelectFolder(Option<String>),
    ToggleTrash,
    Members(String),
    Organization(String),
    LoadThumbnail(String, u64),
    Update(String),
}

impl CloudUi {
    /// The server state was just seen: the next poll waits a full interval.
    fn poll_next_interval(&mut self) {
        self.poll_due = Some(std::time::Instant::now() + POLL_INTERVAL);
    }

    fn link_for(&self, path: &Path) -> Option<&Link> {
        // The cache stores canonical paths; the open document may be named another way.
        let want = rcad_cloud::cache::normalize_local(path);
        self.links
            .iter()
            .find(|l| l.local_path == path || l.local_path == want)
    }
    fn upsert_link(&mut self, link: Link) {
        match self
            .links
            .iter_mut()
            .find(|l| l.local_path == link.local_path)
        {
            Some(l) => *l = link,
            None => self.links.push(link),
        }
    }
    fn doc_name(&self, id: &str) -> String {
        self.tree
            .as_ref()
            .and_then(|t| t.documents.iter().find(|d| d.id == id))
            .map(|d| d.name.clone())
            .or_else(|| {
                self.links
                    .iter()
                    .find(|l| l.document == id)
                    .map(|l| l.document_name.clone())
            })
            .unwrap_or_else(|| id.to_string())
    }
    fn folder_of(&self, doc: &str) -> Option<String> {
        self.tree
            .as_ref()?
            .documents
            .iter()
            .find(|d| d.id == doc)?
            .folder
            .clone()
    }
    /// (id, "Parent / Child") of every folder of the loaded tree.
    fn folder_paths(&self) -> Vec<(String, String)> {
        let Some(t) = &self.tree else { return vec![] };
        let mut out: Vec<(String, String)> = t
            .folders
            .iter()
            .map(|f| {
                let mut name = f.name.clone();
                let mut parent = f.parent.clone();
                // Bounded walk (a malformed tree cannot loop forever).
                for _ in 0..t.folders.len() {
                    let Some(p) = parent.and_then(|p| t.folders.iter().find(|x| x.id == p)) else {
                        break;
                    };
                    name = format!("{} / {name}", p.name);
                    parent = p.parent.clone();
                }
                (f.id.clone(), name)
            })
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }
}

/// "Bracket" from "/x/Bracket.rcad.json".
fn file_stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map_or(String::new(), |n| n.to_string_lossy().into_owned());
    name.strip_suffix(".rcad.json")
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(&name)
        .to_string()
}

/// "2026-10-17 08:05 UTC (3 h ago)".
pub fn fmt_time(t: i64, now: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let ago = now - t;
    let rel = match ago {
        i64::MIN..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", ago / 60),
        3600..=86_399 => format!("{} h ago", ago / 3600),
        _ => format!("{} d ago", ago / 86_400),
    };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC ({rel})",
        secs / 3600,
        (secs % 3600) / 60
    )
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Links of every server from the cache (`<root>/<server>/links.json`).
fn load_links(root: &Path) -> Vec<Link> {
    let Ok(dirs) = std::fs::read_dir(root) else {
        return vec![];
    };
    let mut out = vec![];
    for d in dirs.flatten() {
        let p = d.path().join("links.json");
        if let Ok(t) = std::fs::read_to_string(&p)
            && let Ok(v) = serde_json::from_str::<Vec<Link>>(&t)
        {
            out.extend(v);
        }
    }
    out
}

/// A labelled single-line text field (accessible name and probe key: the label).
fn field(ui: &mut egui::Ui, label: &str, buf: &mut String, password: bool) -> egui::Response {
    ui.horizontal(|ui| {
        let l = crate::form::label(ui, label);
        ui.add(
            egui::TextEdit::singleline(buf)
                .password(password)
                .desired_width(240.0),
        )
        .labelled_by(l.id)
        .probe(label)
    })
    .inner
}

/// Whether signing in to `url` (as typed) would send the password over plain http:// to a
/// host that is not on a local or private network.
fn insecure_url(url: &str) -> bool {
    rcad_cloud::client::normalize_server_url(url)
        .is_ok_and(|u| rcad_cloud::client::needs_insecure_consent(&u))
}

/// Requests whose failure is not shown as a banner (background work).
fn background(request: &str) -> bool {
    matches!(
        request,
        "refresh"
            | "lock"
            | "unlock"
            | "sync_pending"
            | "load_detail"
            | "load_comments"
            | "load_tree"
            | "list_projects"
            | "load_pending"
            | "load_project_members"
            | "load_org_members"
            | "load_usage"
            | "load_thumbnail"
            | "check_head"
    )
}

impl RcadApp {
    // ----- startup and test hooks -----

    /// Interactive startup: start the cloud service only when an account is
    /// configured (reads the account file; nothing else).
    pub fn enable_cloud(&mut self) {
        self.cloud.capture_thumbnails = true;
        if let (Some(c), Some(r)) = (adapter::config_path(), adapter::cache_root()) {
            self.enable_cloud_at(c, r);
        }
    }

    /// [`Self::enable_cloud`] with explicit paths (tests).
    pub fn enable_cloud_at(&mut self, config: PathBuf, cache_root: PathBuf) {
        self.cloud.config_path = Some(config.clone());
        self.cloud.cache_root = Some(cache_root.clone());
        if adapter::has_account(&config)
            && let Some(s) = adapter::start(config, cache_root)
        {
            self.attach_cloud_service(s);
        }
    }

    /// Use `service` (the started service, or a detached one in tests): load
    /// the known links and ask for the account state.
    pub fn attach_cloud_service(&mut self, service: CloudService) {
        if let Some(root) = &self.cloud.cache_root {
            self.cloud.links = load_links(root);
        }
        service.send(Request::Refresh);
        self.cloud.service = Some(service);
    }

    pub fn cloud_service_running(&self) -> bool {
        self.cloud.service.is_some()
    }
    pub fn cloud_panel_open(&self) -> bool {
        self.cloud.panel_open
    }
    pub fn cloud_dialog(&self) -> Option<&CloudDialog> {
        self.cloud.dialog.as_ref()
    }
    pub fn cloud_conflict(&self) -> Option<&Conflict> {
        self.cloud.conflict.as_ref()
    }
    pub fn cloud_share_url(&self) -> Option<&str> {
        self.cloud.share_url.as_deref()
    }
    /// Take a screenshot of the 3D view as the thumbnail of each version saved
    /// or uploaded (on in the interactive app; tests opt in).
    pub fn set_cloud_thumbnail_capture(&mut self, on: bool) {
        self.cloud.capture_thumbnails = on;
    }
    /// Make the next update-available poll due now (tests: no 60 s wait).
    pub fn cloud_expire_poll_timer(&mut self) {
        self.cloud.poll_due = None;
    }
    /// Whether a thumbnail texture of `version` of `document` is loaded.
    pub fn cloud_thumbnail_loaded(&self, document: &str, version: u64) -> bool {
        self.cloud
            .thumbs
            .get(&(document.to_string(), version))
            .is_some_and(Option::is_some)
    }

    /// Link of the open document, if it is a cloud working copy.
    pub fn cloud_link(&self) -> Option<&Link> {
        if !self.summary.open {
            return None;
        }
        self.cloud.link_for(self.summary.path.as_deref()?)
    }

    fn cloud_read_only(&self) -> bool {
        self.summary
            .path
            .as_ref()
            .is_some_and(|p| self.cloud.read_only.contains(p))
    }

    /// State of the open cloud-linked document (None: not linked).
    pub fn cloud_state(&self) -> Option<SyncState> {
        let link = self.cloud_link()?;
        let doc = &link.document;
        Some(if self.cloud_read_only() {
            SyncState::ReadOnly
        } else if self
            .cloud
            .conflict
            .as_ref()
            .is_some_and(|c| c.document == *doc)
        {
            SyncState::Conflict
        } else if let Some(l) = self.cloud.locked_by.get(doc) {
            SyncState::Locked {
                holder: l.holder.clone(),
            }
        } else if self.cloud.offline {
            SyncState::Offline {
                pending: self.cloud.pending,
            }
        } else {
            let modified = self.cloud.edited.contains(doc) || self.has_unsaved_changes();
            match self.cloud.heads.get(doc) {
                Some(&head) if head > link.base_version && modified => {
                    SyncState::NewerOnServer { head }
                }
                Some(&head) if head > link.base_version => SyncState::UpdateAvailable { head },
                _ if modified => SyncState::Modified,
                _ => SyncState::Synced,
            }
        })
    }

    /// The app-bar chip text: `☁ Bracket v3 · Synced`.
    pub fn cloud_chip(&self) -> Option<String> {
        let link = self.cloud_link()?;
        let state = self.cloud_state()?;
        let mut name = link.document_name.clone();
        if name.chars().count() > 24 {
            name = name.chars().take(23).collect::<String>() + "…";
        }
        Some(format!(
            "☁ {name} v{} · {}",
            link.base_version,
            state.text()
        ))
    }

    /// Cloud state for the live-test probe (JSON object).
    pub fn cloud_probe(&self) -> serde_json::Value {
        let c = &self.cloud;
        let link = self.cloud_link();
        serde_json::json!({
            "running": c.service.is_some(),
            "panel": c.panel_open,
            "signed_in": c.account.is_some(),
            "server": c.account.as_ref().map(|a| a.server_name.clone()),
            "user": c.account.as_ref().map(|a| a.me.user.username.clone()),
            "org": c.org,
            "project": c.project,
            "linked": link.is_some(),
            "document": link.map(|l| l.document_name.clone()),
            "version": link.map(|l| l.base_version),
            "state": self.cloud_state().map_or("none", |s| s.key()),
            "pending": c.pending,
            "versions_listed": c.detail.as_ref().map_or(0, |d| d.versions.len()),
            "share_url": c.share_url,
            "has_share_url": c.share_url.is_some(),
            "dialog": c.dialog.is_some(),
            "dialog_kind": c.dialog.as_ref().map(|d| d.key()),
            "conflict": c.conflict.is_some(),
            "selected_document": c.selected_doc,
            "selected_folder": c.selected_folder,
            "documents_listed": c.tree.as_ref().map_or(0, |t| t.documents.len()),
            "folders_listed": c.tree.as_ref().map_or(0, |t| t.folders.len()),
            "trash_listed": c.tree.as_ref().map_or(0, |t| t.trash.len()),
            "show_trash": c.show_trash,
            "members_listed": c.members.as_ref().map_or(0, |(_, m)| m.len()),
            "members": c.members.as_ref().map(|(_, m)| m
                .iter()
                .map(|x| serde_json::json!({
                    "user": x.user.username,
                    "role": project_role_name(x.role),
                }))
                .collect::<Vec<_>>()),
            "member_roles": c.members.as_ref().map(|(_, m)| m
                .iter()
                .map(|x| (x.user.username.clone(), serde_json::json!(project_role_name(x.role))))
                .collect::<serde_json::Map<_, _>>()),
            "org_members_listed": c.org_members.as_ref().map_or(0, |(_, m)| m.len()),
            "usage_loaded": c.usage.is_some(),
            "admin_error_code": c.admin_error_code,
            "usage_bytes": c.usage.as_ref().map(|(_, u)| u.used_bytes),
            "admin_error": match &c.dialog {
                Some(CloudDialog::Members { error, .. } | CloudDialog::Organization { error, .. }) => error.clone(),
                _ => None,
            },
            "queue_listed": c.queue.len(),
            "server_head": link.and_then(|l| c.heads.get(&l.document).copied()),
            "thumbnails_loaded": c.thumbs.values().filter(|t| t.is_some()).count(),
            "save_version_enabled": self
                .command_enabled(crate::shell::CmdId::SaveVersion)
                .is_ok(),
            "detail_head": c.detail.as_ref().map(|d| d.document.head_version),
            "milestones_listed": c.detail.as_ref().map_or(0, |d| d
                .versions
                .iter()
                .filter(|v| v.milestone.is_some())
                .count()),
            "comments_listed": c.comments.as_ref().map_or(0, |(_, l)| l.len()),
            "comments_resolved": c.comments.as_ref().map_or(0, |(_, l)| l
                .iter()
                .filter(|x| x.resolved)
                .count()),
            "document_folders": c.tree.as_ref().map(|t| t
                .documents
                .iter()
                .map(|d| {
                    let folder = d.folder.as_ref().and_then(|f| {
                        t.folders.iter().find(|x| x.id == *f).map(|x| x.name.clone())
                    });
                    // "/" is the project root (a missing document reads as null).
                    (d.name.clone(), serde_json::json!(folder.unwrap_or_else(|| "/".into())))
                })
                .collect::<serde_json::Map<_, _>>()),
        })
    }

    pub(crate) fn cloud_modal_open(&self) -> bool {
        self.cloud.dialog.is_some() || self.cloud.conflict.is_some()
    }

    fn cloud_send(&mut self, r: Request) {
        match &self.cloud.service {
            Some(s) => s.send(r),
            None => self.cloud_log(false, "not connected to a server".into()),
        }
    }

    fn cloud_log(&mut self, error: bool, text: String) {
        self.diag(error, "Cloud", format!("Cloud: {text}"));
    }

    /// Screenshot scene: a detached service fed with a sample server's
    /// events (no network), and `doc` linked as the working copy of v4 of a
    /// cloud document. `history`: show the version history section.
    pub(crate) fn cloud_scene(&mut self, doc: PathBuf, history: bool) {
        let (svc, req, ev) = CloudService::detached();
        self.attach_cloud_service(svc);
        self.cloud.scene = Some(req);
        self.cloud.panel_open = true;
        self.cloud.project = Some("p1".into());
        self.cloud.section = if history {
            Section::History
        } else {
            Section::Browse
        };
        let t0 = 1_792_224_300;
        let user = |name: &str| proto::User {
            id: name.into(),
            username: name.into(),
            display_name: name.into(),
            server_admin: false,
        };
        let project = proto::Project {
            id: "p1".into(),
            org: "o1".into(),
            name: "Workshop fixtures".into(),
            description: String::new(),
            my_role: proto::ProjectRole::Editor,
            created_at: t0,
            revision: 9,
        };
        let docsum = |id: &str, name: &str, folder: Option<&str>, head: u64, by: &str| {
            proto::DocumentSummary {
                id: id.into(),
                project: "p1".into(),
                folder: folder.map(String::from),
                name: name.into(),
                head_version: head,
                updated_at: t0 - 3600 * head as i64,
                updated_by: by.into(),
                lock: (id == "d3").then(|| proto::Lock {
                    holder: "sam".into(),
                    since: t0,
                    expires_at: t0 + 900,
                }),
                has_thumbnail: false,
            }
        };
        let version =
            |n: u64, author: &str, description: &str, milestone: Option<&str>| proto::Version {
                number: n,
                blob: "0".repeat(64),
                size: 40_000,
                author: author.into(),
                created_at: t0 - 86_400 * (4 - n as i64),
                description: description.into(),
                milestone: milestone.map(String::from),
                promoted_from: None,
                generator: String::new(),
                thumbnail: None,
            };
        let events = vec![
            Event::SignedIn {
                server_url: "https://cad.example.org".into(),
                server_name: "Workshop server".into(),
                me: proto::Me {
                    user: user("alex"),
                    orgs: vec![proto::OrgSummary {
                        id: "o1".into(),
                        name: "Workshop".into(),
                        my_role: proto::OrgRole::Member,
                    }],
                },
            },
            Event::Projects {
                org: "o1".into(),
                projects: vec![
                    project.clone(),
                    proto::Project {
                        id: "p2".into(),
                        name: "Printer upgrades".into(),
                        ..project.clone()
                    },
                ],
            },
            Event::Tree(proto::ProjectTree {
                project,
                folders: vec![
                    proto::Folder {
                        id: "f1".into(),
                        project: "p1".into(),
                        parent: None,
                        name: "Brackets".into(),
                    },
                    proto::Folder {
                        id: "f2".into(),
                        project: "p1".into(),
                        parent: None,
                        name: "Jigs".into(),
                    },
                ],
                documents: vec![
                    docsum("d1", "Bracket", Some("f1"), 4, "alex"),
                    docsum("d2", "Corner bracket", Some("f1"), 2, "sam"),
                    docsum("d3", "Drill jig", Some("f2"), 7, "sam"),
                    docsum("d4", "Enclosure", None, 1, "alex"),
                ],
                trash: vec![],
            }),
            Event::Detail(proto::DocumentDetail {
                document: docsum("d1", "Bracket", Some("f1"), 4, "alex"),
                versions: vec![
                    version(4, "alex", "Wider flange for the M6 bolts", None),
                    version(3, "sam", "Fillet on the inner corner", Some("Prototype A")),
                    version(2, "alex", "Mounting holes", None),
                    version(1, "alex", "First upload", None),
                ],
                my_role: proto::ProjectRole::Editor,
            }),
        ];
        for e in events {
            let _ = ev.send(e);
        }
        self.cloud.selected_doc = Some("d1".into());
        self.cloud.upsert_link(Link {
            local_path: doc,
            server_url: "https://cad.example.org".into(),
            org: "o1".into(),
            project: "p1".into(),
            document: "d1".into(),
            document_name: "Bracket".into(),
            base_version: 4,
            base_blob: None,
        });
    }

    // ----- per frame -----

    /// Poll service events and follow the open document's edit state
    /// (auto-lock, unlock when leaving a locked document).
    pub(crate) fn cloud_tick(&mut self, ctx: &egui::Context) {
        let Some(service) = &self.cloud.service else {
            return;
        };
        let mut events = vec![];
        while let Some(e) = service.poll() {
            events.push(e);
        }
        for e in events {
            self.cloud_event(e);
        }
        self.cloud_thumbnail_textures(ctx);
        self.cloud_poll(ctx);
        if self.cloud.save_to_cloud_after_save
            && self.dialog.is_none()
            && self.idle()
            && self.summary.path.is_none()
        {
            // Save As was cancelled.
            self.cloud.save_to_cloud_after_save = false;
        }
        let path = self.summary.path.clone().filter(|_| self.summary.open);
        let dirty = self.has_unsaved_changes();
        if path != self.cloud.last_path {
            // Left a document: release its lock.
            if let Some(prev) = self.cloud.last_path.take()
                && let Some(doc) = self.cloud.link_for(&prev).map(|l| l.document.clone())
                && (self.cloud.my_locks.contains(&doc) || self.cloud.lock_sent.contains(&doc))
            {
                self.cloud.my_locks.remove(&doc);
                self.cloud.lock_sent.remove(&doc);
                self.cloud_send(Request::Unlock { document: doc });
            }
            self.cloud.last_path = path;
            self.cloud.last_dirty = dirty;
            return;
        }
        if dirty
            && !self.cloud.last_dirty
            && !self.cloud_read_only()
            && let Some(doc) = self.cloud_link().map(|l| l.document.clone())
        {
            self.cloud.edited.insert(doc.clone());
            if self.shell.prefs.cloud_auto_lock
                && !self.cloud.my_locks.contains(&doc)
                && !self.cloud.lock_sent.contains(&doc)
                && !self.cloud.locked_by.contains_key(&doc)
            {
                self.cloud.lock_sent.insert(doc.clone());
                self.cloud_send(Request::Lock { document: doc });
            }
        }
        self.cloud.last_dirty = dirty;
    }

    /// Release a held lock at exit (best effort; locks also expire on the server).
    pub(crate) fn cloud_shutdown(&mut self) {
        let docs: Vec<String> = self.cloud.my_locks.drain().collect();
        for document in docs {
            self.cloud_send(Request::Unlock { document });
        }
    }

    fn cloud_event(&mut self, e: Event) {
        let c = &mut self.cloud;
        match e {
            Event::Thumbnail {
                document,
                version,
                png,
            } => {
                c.thumb_png.push(((document, version), png));
            }
            Event::Head {
                document,
                head_version,
            } => {
                c.heads.insert(document, head_version);
            }
            Event::SignedIn {
                server_url,
                server_name,
                me,
            } => {
                if matches!(c.dialog, Some(CloudDialog::SignIn { .. })) {
                    c.dialog = None;
                }
                let first = c.account.is_none();
                if c.org
                    .as_ref()
                    .is_none_or(|o| !me.orgs.iter().any(|x| x.id == *o))
                {
                    c.org = me.orgs.first().map(|o| o.id.clone());
                }
                let user = me.user.username.clone();
                c.account = Some(Account {
                    server_url,
                    server_name: server_name.clone(),
                    me,
                });
                c.poll_next_interval();
                if let Some(org) = c.org.clone() {
                    self.cloud_send(Request::ListProjects { org });
                }
                if first {
                    self.cloud_log(false, format!("signed in to {server_name} as {user}"));
                }
            }
            Event::SignedOut => {
                let links = std::mem::take(&mut c.links);
                let (config_path, cache_root) = (c.config_path.clone(), c.cache_root.clone());
                let service = c.service.take();
                let panel_open = c.panel_open;
                let capture_thumbnails = c.capture_thumbnails;
                *c = CloudUi {
                    service,
                    config_path,
                    cache_root,
                    panel_open,
                    capture_thumbnails,
                    // Links stay: the working copies are still on disk.
                    links,
                    ..Default::default()
                };
                self.cloud_log(false, "signed out".into());
            }
            Event::Projects { org, projects } => {
                if c.org.as_deref() == Some(org.as_str()) {
                    // A project whose revision moved has a stale tree: reload it.
                    let stale = c.tree.as_ref().and_then(|t| {
                        projects
                            .iter()
                            .find(|p| p.id == t.project.id && p.revision != t.project.revision)
                            .map(|p| p.id.clone())
                    });
                    c.projects = projects;
                    if let Some(project) = stale {
                        self.cloud_send(Request::LoadTree { project });
                    }
                }
            }
            Event::Tree(t) => {
                if c.project.as_deref() == Some(t.project.id.as_str()) {
                    // Forget selections the reload removed (trashed, deleted).
                    if c.selected_folder
                        .as_ref()
                        .is_some_and(|f| !t.folders.iter().any(|x| x.id == *f))
                    {
                        c.selected_folder = None;
                    }
                    if c.selected_doc
                        .as_ref()
                        .is_some_and(|d| !t.documents.iter().any(|x| x.id == *d))
                    {
                        c.selected_doc = None;
                    }
                    // Keep the project's role current in the project list.
                    if let Some(p) = c.projects.iter_mut().find(|p| p.id == t.project.id) {
                        *p = t.project.clone();
                    }
                    // Renamed documents keep their working-copy links current.
                    for l in &mut c.links {
                        if let Some(d) = t.documents.iter().find(|d| d.id == l.document) {
                            l.document_name.clone_from(&d.name);
                        }
                    }
                    c.tree = Some(t);
                    c.poll_next_interval();
                }
            }
            Event::DocumentReady {
                path,
                link,
                read_only,
                from_cache,
            } => {
                let version = link.base_version;
                if read_only {
                    c.read_only.insert(path.clone());
                } else {
                    c.read_only.remove(&path);
                    c.edited.remove(&link.document);
                }
                c.upsert_link(link);
                // Just downloaded (or the server is unreachable): poll later.
                c.poll_next_interval();
                if read_only {
                    self.cloud_log(
                        false,
                        format!("read-only copy of v{version}; Save Version is disabled"),
                    );
                }
                if from_cache {
                    self.cloud_log(false, "server unreachable; opened the cached copy".into());
                }
                self.guarded(Guarded::Open(path));
            }
            Event::VersionSaved { link, version } => {
                let doc = link.document.clone();
                let name = link.document_name.clone();
                let project = link.project.clone();
                c.edited.remove(&doc);
                if c.conflict.as_ref().is_some_and(|x| x.document == doc) {
                    c.conflict = None;
                }
                c.read_only.remove(&link.local_path);
                c.upsert_link(link);
                c.poll_next_interval();
                let unlock = c.my_locks.remove(&doc) | c.lock_sent.remove(&doc);
                let reload_detail = c.detail.as_ref().is_some_and(|d| d.document.id == doc);
                let reload_tree = c.project.as_deref() == Some(project.as_str());
                self.cloud_log(false, format!("saved version {} of {name}", version.number));
                if unlock && self.shell.prefs.cloud_auto_lock {
                    self.cloud_send(Request::Unlock {
                        document: doc.clone(),
                    });
                }
                if reload_detail {
                    self.cloud_send(Request::LoadDetail { document: doc });
                }
                if reload_tree {
                    self.cloud_send(Request::LoadTree { project });
                }
            }
            Event::Conflict {
                local_path,
                document,
                base_version,
                head_version,
            } => {
                c.conflict = Some(Conflict {
                    local_path,
                    document,
                    base_version,
                    head_version,
                });
            }
            Event::Locked { document, lock } => {
                c.lock_sent.remove(&document);
                c.my_locks.remove(&document);
                let holder = lock.holder.clone();
                let name = c.doc_name(&document);
                c.locked_by.insert(document, lock);
                self.cloud_log(
                    false,
                    format!(
                        "{name} is locked by {holder}; you can keep editing locally, \
                         but Save Version waits until the lock is released"
                    ),
                );
            }
            Event::LockAcquired { document, .. } => {
                c.lock_sent.remove(&document);
                c.locked_by.remove(&document);
                c.my_locks.insert(document);
            }
            Event::LockReleased { document } => {
                c.lock_sent.remove(&document);
                c.my_locks.remove(&document);
                c.locked_by.remove(&document);
            }
            Event::Detail(d) => {
                // The lock state shown in the tree and chip follows the server.
                let id = d.document.id.clone();
                match &d.document.lock {
                    Some(l)
                        if c.account
                            .as_ref()
                            .is_none_or(|a| a.me.user.username != l.holder) =>
                    {
                        c.locked_by.insert(id, l.clone());
                    }
                    _ => {
                        c.locked_by.remove(&id);
                    }
                }
                c.detail = Some(d);
            }
            Event::ShareCreated(r) => {
                c.share_url = Some(r.url);
                if let Some(CloudDialog::Share { sent, .. }) = &mut c.dialog {
                    *sent = false;
                }
            }
            Event::Comments { document, comments } => {
                c.comments = Some((document, comments));
            }
            Event::Offline { pending } => {
                let was = c.offline;
                c.offline = true;
                c.pending = pending;
                // Back off: the next poll waits a full interval.
                c.poll_next_interval();
                if let Some(CloudDialog::SignIn { sent, error, .. }) = &mut c.dialog {
                    *sent = false;
                    *error = Some("The server could not be reached.".into());
                }
                if !was {
                    self.cloud_log(
                        false,
                        format!(
                            "server unreachable; working offline ({pending} version(s) waiting)"
                        ),
                    );
                }
            }
            Event::Synced { pending } => {
                let was = c.offline;
                c.offline = false;
                c.pending = pending;
                if pending == 0 {
                    c.queue.clear();
                    c.queue_asked = None;
                }
                if was {
                    self.cloud_log(false, format!("back online ({pending} version(s) waiting)"));
                }
            }
            Event::PendingQueue { saves } => {
                c.pending = saves.len();
                c.queue_asked = Some(saves.len());
                c.queue = saves;
            }
            Event::ProjectMembers { project, members } => {
                if let Some(CloudDialog::Members { error, .. }) = &mut c.dialog {
                    *error = None;
                    c.admin_error_code = None;
                }
                c.members = Some((project, members));
            }
            Event::OrgMembers { org, members } => {
                if let Some(CloudDialog::Organization { error, .. }) = &mut c.dialog {
                    *error = None;
                    c.admin_error_code = None;
                }
                c.org_members = Some((org, members));
            }
            Event::Usage { org, usage } => {
                c.usage = Some((org, usage));
            }
            Event::Error {
                request,
                code,
                message,
            } => {
                if request == "sign_in"
                    && let Some(CloudDialog::SignIn { sent, error, .. }) = &mut c.dialog
                {
                    *sent = false;
                    *error = Some(message.clone());
                }
                if request == "create_share"
                    && let Some(CloudDialog::Share { sent, .. }) = &mut c.dialog
                {
                    *sent = false;
                }
                if request == "lock" {
                    c.lock_sent.clear();
                }
                let inline = self.cloud_admin_error(request, code, &message);
                let hint =
                    if code == Some(proto::ErrorCode::Unauthenticated) && request != "sign_in" {
                        " (sign in again from the Data panel)"
                    } else {
                        ""
                    };
                self.cloud_log(!background(request) && !inline, format!("{message}{hint}"));
            }
        }
    }

    /// A finished engine command: continue a version save or an upload that
    /// waited for the local save.
    pub(crate) fn cloud_on_reply(&mut self, command: &Command, ok: bool) {
        let Command::Save { path } = command else {
            return;
        };
        if self.cloud.save_to_cloud_after_save {
            self.cloud.save_to_cloud_after_save = false;
            if ok {
                self.cloud_save_to_cloud_dialog();
            }
        }
        let waiting = match &self.cloud.after_save {
            Some(AfterSave::Version { path: p, .. } | AfterSave::Upload { path: p, .. }) => {
                p == path
            }
            None => false,
        };
        if !waiting {
            return;
        }
        let Some(next) = self.cloud.after_save.take() else {
            return;
        };
        if !ok {
            self.cloud_log(
                true,
                "the local save failed, so nothing was sent to the server".into(),
            );
            return;
        }
        match next {
            AfterSave::Version {
                path,
                description,
                milestone,
            } => self.cloud_send_with_thumbnail(Request::SaveVersion {
                local_path: path,
                description,
                milestone,
                force: false,
                thumbnail: None,
            }),
            AfterSave::Upload {
                path,
                project,
                folder,
                name,
            } => self.cloud_send_with_thumbnail(Request::UploadNew {
                local_path: path,
                project,
                folder,
                name,
                thumbnail: None,
            }),
        }
    }

    // ----- commands (registry) -----

    fn need_cloud(&self) -> Result<(), String> {
        if self.cloud.account.is_some() {
            Ok(())
        } else {
            Err("Connect to an R-CAD server first (Data panel)".into())
        }
    }

    pub(crate) fn cloud_linked_blocker(&self) -> Result<(), String> {
        self.need_cloud()?;
        self.need_open()?;
        if self.cloud_link().is_none() {
            return Err(
                "This document is not on the server; use Upload current document… in the Data panel"
                    .into(),
            );
        }
        Ok(())
    }

    pub(crate) fn cloud_save_version_blocker(&self) -> Result<(), String> {
        self.cloud_linked_blocker()?;
        if self.cloud_read_only() {
            return Err("This is a read-only copy of an older version".into());
        }
        Ok(())
    }

    pub(crate) fn cloud_sync_blocker(&self) -> Result<(), String> {
        if self.cloud.service.is_none() {
            return Err("Connect to an R-CAD server first (Data panel)".into());
        }
        if !self.cloud.offline && self.cloud.pending == 0 {
            return Err("No versions are waiting to be sent".into());
        }
        Ok(())
    }

    pub(crate) fn toggle_data_panel(&mut self) {
        self.cloud.panel_open = !self.cloud.panel_open;
        if self.cloud.panel_open {
            // Opening the panel checks for newer versions at once.
            self.cloud.poll_due = None;
        }
    }

    pub(crate) fn cloud_save_version_dialog(&mut self) {
        if let Err(why) = self.cloud_save_version_blocker() {
            self.notify(why);
            return;
        }
        self.cloud.dialog = Some(CloudDialog::SaveVersion {
            description: String::new(),
            milestone: String::new(),
        });
    }

    /// Document the history / comments / share actions are about: the open
    /// linked document.
    fn cloud_target_doc(&self) -> Option<String> {
        self.cloud_link().map(|l| l.document.clone())
    }

    pub(crate) fn cloud_version_history(&mut self) {
        let Some(doc) = self.cloud_target_doc() else {
            return;
        };
        self.cloud.panel_open = true;
        self.cloud.section = Section::History;
        self.cloud_send(Request::LoadDetail { document: doc });
    }

    pub(crate) fn cloud_comments(&mut self) {
        let Some(doc) = self.cloud_target_doc() else {
            return;
        };
        self.cloud.panel_open = true;
        self.cloud.section = Section::Comments;
        self.cloud_send(Request::LoadComments { document: doc });
    }

    pub(crate) fn cloud_share_dialog(&mut self) {
        let Some(doc) = self.cloud_target_doc() else {
            return;
        };
        self.cloud.share_url = None;
        self.cloud.dialog = Some(CloudDialog::Share {
            days: "7".into(),
            allow_download: true,
            version: None,
            sent: false,
        });
        if self
            .cloud
            .detail
            .as_ref()
            .is_none_or(|d| d.document.id != doc)
        {
            self.cloud_send(Request::LoadDetail { document: doc });
        }
    }

    pub(crate) fn cloud_sync_now(&mut self) {
        self.cloud_send(Request::SyncPending);
    }

    fn cloud_connect_dialog(&mut self) {
        let server_url = self
            .cloud
            .account
            .as_ref()
            .map(|a| a.server_url.clone())
            .unwrap_or_default();
        self.cloud.dialog = Some(CloudDialog::SignIn {
            server_url,
            username: String::new(),
            password: String::new(),
            allow_insecure: false,
            error: None,
            sent: false,
        });
    }

    /// Connect: start the service if needed, then send SignIn.
    fn cloud_sign_in(&mut self) {
        let Some(CloudDialog::SignIn {
            server_url,
            username,
            password,
            allow_insecure,
            error,
            sent,
        }) = &mut self.cloud.dialog
        else {
            return;
        };
        let url = server_url.trim().to_string();
        let user = username.trim().to_string();
        if url.is_empty() || user.is_empty() || password.is_empty() {
            *error = Some("Enter the server URL, your username and your password.".into());
            return;
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            *error =
                Some("The server URL starts with https:// (or http:// on a local network).".into());
            return;
        }
        let allow_insecure_http = *allow_insecure;
        if insecure_url(&url) && !allow_insecure_http {
            *error = Some(
                "This connection would not be encrypted. Use https://, or tick \
                 \u{201c}Allow unencrypted connection\u{201d}."
                    .into(),
            );
            return;
        }
        let password = std::mem::take(password);
        *error = None;
        *sent = true;
        if self.cloud.service.is_none() {
            let started = match (
                self.cloud.config_path.clone(),
                self.cloud.cache_root.clone(),
            ) {
                (Some(c), Some(r)) => adapter::start(c, r),
                _ => adapter::config_path()
                    .zip(adapter::cache_root())
                    .and_then(|(c, r)| adapter::start(c, r)),
            };
            match started {
                Some(s) => self.cloud.service = Some(s),
                None => {
                    if let Some(CloudDialog::SignIn { error, sent, .. }) = &mut self.cloud.dialog {
                        *error = Some("This build has no sync client.".into());
                        *sent = false;
                    }
                    return;
                }
            }
        }
        self.cloud_send(Request::SignIn {
            server_url: url,
            username: user,
            password,
            allow_insecure_http,
        });
    }

    fn cloud_upload_dialog(&mut self) {
        if !self.summary.open {
            self.notify("Open or create a document to upload first".into());
            return;
        }
        let Some(path) = self.summary.path.clone() else {
            self.save();
            self.notify(
                "Save the document locally first, then choose Upload current document… again"
                    .into(),
            );
            return;
        };
        self.cloud.dialog = Some(CloudDialog::Upload {
            name: file_stem(&path),
            folder: self.cloud.selected_folder.clone(),
        });
    }

    /// File ▸ Save to Cloud Project…: put the open, unlinked document in a
    /// project. A document never saved locally goes through Save As first.
    pub(crate) fn cloud_save_to_cloud(&mut self) {
        if let Err(why) = self.cloud_save_to_cloud_blocker() {
            self.notify(why);
            return;
        }
        if self.summary.path.is_none() {
            self.cloud.save_to_cloud_after_save = true;
            self.save();
            return;
        }
        self.cloud_save_to_cloud_dialog();
    }

    fn cloud_save_to_cloud_dialog(&mut self) {
        let Some(path) = self.summary.path.clone() else {
            return;
        };
        let editable = |p: &proto::Project| p.my_role >= proto::ProjectRole::Editor;
        let project = self
            .cloud
            .project
            .clone()
            .filter(|id| {
                self.cloud
                    .projects
                    .iter()
                    .any(|p| p.id == *id && editable(p))
            })
            .or_else(|| {
                self.cloud
                    .projects
                    .iter()
                    .find(|p| editable(p))
                    .map(|p| p.id.clone())
            });
        if project.is_some() && project != self.cloud.project {
            self.cloud_apply(vec![Action::SelectProject(
                project.clone().unwrap_or_default(),
            )]);
        }
        if self.cloud.projects.is_empty()
            && let Some(org) = self.cloud.org.clone()
        {
            self.cloud_send(Request::ListProjects { org });
        }
        let folder = self
            .cloud
            .selected_folder
            .clone()
            .filter(|_| project.is_some() && project == self.cloud.project);
        self.cloud.dialog = Some(CloudDialog::SaveToCloud {
            project,
            folder,
            name: file_stem(&path),
        });
    }

    pub(crate) fn cloud_save_to_cloud_blocker(&self) -> Result<(), String> {
        self.need_cloud()?;
        self.need_open()?;
        if self.cloud_link().is_some() {
            return Err(
                "This document is already on the server; use Save Version to Cloud…".into(),
            );
        }
        if self.sketch.is_some() {
            return Err("Finish or cancel the sketch first".into());
        }
        Ok(())
    }

    fn cloud_upload(&mut self, name: String, folder: Option<String>) {
        let Some(project) = self.cloud.project.clone() else {
            return;
        };
        self.cloud_upload_to(project, name, folder);
    }

    fn cloud_upload_to(&mut self, project: String, name: String, folder: Option<String>) {
        let Some(path) = self.summary.path.clone() else {
            return;
        };
        if self.sketch.is_some() {
            self.notify("Finish or cancel the sketch first".into());
            return;
        }
        if self.has_unsaved_changes() {
            self.cloud.after_save = Some(AfterSave::Upload {
                path: path.clone(),
                project,
                folder,
                name,
            });
            self.request(Command::Save { path });
        } else {
            self.cloud_send_with_thumbnail(Request::UploadNew {
                local_path: path,
                project,
                folder,
                name,
                thumbnail: None,
            });
        }
    }

    fn cloud_commit_version(&mut self, description: String, milestone: Option<String>) {
        let Some(path) = self.cloud_link().map(|l| l.local_path.clone()) else {
            return;
        };
        if self.sketch.is_some() {
            self.notify("Finish or cancel the sketch first".into());
            return;
        }
        self.cloud.last_version = Some((description.clone(), milestone.clone()));
        if self.has_unsaved_changes() {
            self.cloud.after_save = Some(AfterSave::Version {
                path: path.clone(),
                description,
                milestone,
            });
            self.request(Command::Save { path });
        } else {
            self.cloud_send_with_thumbnail(Request::SaveVersion {
                local_path: path,
                description,
                milestone,
                force: false,
                thumbnail: None,
            });
        }
    }

    /// Send a version save or upload with a thumbnail of the model when the
    /// app has a GPU view: a clean offscreen render (fitted isometric view, no
    /// sketches, dimensions, selection, hover, section or other overlays; see
    /// `rcad_viewport::Viewport::render_thumbnail`). The window's view is not
    /// changed, so nothing flickers. Without a view or a body the request goes
    /// out without a thumbnail.
    fn cloud_send_with_thumbnail(&mut self, mut request: Request) {
        if self.cloud.capture_thumbnails {
            let side = 2 * thumb::THUMBNAIL_PX;
            let png = self
                .viewport
                .as_ref()
                .and_then(|vp| vp.render_thumbnail(side))
                .and_then(|rgba| thumb::thumbnail_png(&rgba, side));
            if let Request::SaveVersion { thumbnail, .. } | Request::UploadNew { thumbnail, .. } =
                &mut request
            {
                *thumbnail = png;
            }
        }
        self.cloud_send(request);
    }

    /// Turn received thumbnail PNGs into textures.
    fn cloud_thumbnail_textures(&mut self, ctx: &egui::Context) {
        for ((doc, version), png) in std::mem::take(&mut self.cloud.thumb_png) {
            let tex = png.as_deref().and_then(thumb::decode_png).map(|img| {
                ctx.load_texture(
                    format!("cloud-thumb-{doc}-{version}"),
                    img,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.cloud.thumbs.insert((doc, version), tex);
        }
    }

    /// Update-available polling: while signed in, every [`POLL_INTERVAL`]
    /// (and at once when the window regains focus or the Data panel opens),
    /// ask for the open linked document's head, and with the Data panel open
    /// re-list the projects (a changed revision reloads the tree).
    fn cloud_poll(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        if focused && self.cloud.focused == Some(false) {
            self.cloud.poll_due = None;
        }
        self.cloud.focused = Some(focused);
        if self.cloud.account.is_none() {
            return;
        }
        let now = std::time::Instant::now();
        if self.cloud.poll_due.is_some_and(|t| now < t) {
            if let Some(t) = self.cloud.poll_due {
                ctx.request_repaint_after(t - now);
            }
            return;
        }
        let doc = self
            .cloud_link()
            .filter(|_| !self.cloud_read_only())
            .map(|l| l.document.clone());
        let org = self
            .cloud
            .org
            .clone()
            .filter(|_| self.cloud.panel_open && self.cloud.tree.is_some());
        if doc.is_none() && org.is_none() {
            return;
        }
        self.cloud.poll_due = Some(now + POLL_INTERVAL);
        ctx.request_repaint_after(POLL_INTERVAL);
        if let Some(document) = doc {
            self.cloud_send(Request::CheckHead { document });
        }
        if let Some(org) = org {
            self.cloud_send(Request::ListProjects { org });
        }
    }

    /// A thumbnail texture to draw, asking for it once when not loaded yet.
    fn cloud_thumb(
        &self,
        document: &str,
        version: u64,
        actions: &mut Vec<Action>,
    ) -> Option<egui::TextureHandle> {
        let key = (document.to_string(), version);
        if let Some(t) = self.cloud.thumbs.get(&key) {
            return t.clone();
        }
        if !self.cloud.thumb_asked.contains(&key) {
            actions.push(Action::LoadThumbnail(key.0, version));
        }
        None
    }

    // ----- drawing -----

    /// Application-bar toggle for the Data panel (left end of the bar).
    pub(crate) fn cloud_app_bar_toggle(&mut self, ui: &mut egui::Ui) {
        let def = crate::shell::registry::def(crate::shell::CmdId::DataPanel);
        if ui
            .add(egui::Button::selectable(self.cloud.panel_open, "Data"))
            .named(def.label)
            .on_hover_text(def.tooltip)
            .clicked()
        {
            self.toggle_data_panel();
        }
    }

    /// The status chip of a cloud-linked document (nothing otherwise).
    pub(crate) fn cloud_chip_ui(&mut self, ui: &mut egui::Ui) {
        let (Some(text), Some(state)) = (self.cloud_chip(), self.cloud_state()) else {
            return;
        };
        let p = crate::theme::of(ui);
        let color = match state {
            SyncState::Synced => p.ok,
            SyncState::Modified | SyncState::Offline { .. } => p.warn,
            SyncState::Conflict => p.bad,
            SyncState::Locked { .. } => p.caution,
            SyncState::ReadOnly | SyncState::UpdateAvailable { .. } => p.info,
            SyncState::NewerOnServer { .. } => p.warn,
        };
        let hover = match state {
            SyncState::UpdateAvailable { head } => format!(
                "Someone saved v{head} on the server. Your copy has no local edits: \
                 Update replaces it with v{head}."
            ),
            SyncState::NewerOnServer { head } => format!(
                "Someone saved v{head} on the server while you edited. Save Version will \
                 report a conflict: save anyway, or save as a new document."
            ),
            _ => "Cloud document: version on the server and sync state. Click for the \
                  version history."
                .into(),
        };
        let r = ui
            .add(egui::Button::new(RichText::new(&text).color(color)).frame(false))
            .named("Cloud status")
            .on_hover_text(hover);
        if matches!(state, SyncState::UpdateAvailable { .. })
            && let Some(doc) = self.cloud_link().map(|l| l.document.clone())
            && ui
                .small_button("Update")
                .probe("Update")
                .on_hover_text("Download the newer version into this working copy and reopen it")
                .clicked()
        {
            self.cloud_apply(vec![Action::Update(doc)]);
            return;
        }
        if r.clicked() {
            if matches!(state, SyncState::Offline { .. }) {
                self.cloud_sync_now();
            }
            self.cloud_version_history();
        }
    }

    /// The Data panel (left column).
    pub(crate) fn cloud_panel(&mut self, ui: &mut egui::Ui) {
        let mut actions: Vec<Action> = vec![];
        ui.horizontal(|ui| {
            ui.strong("Data");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .named("Close data panel")
                    .on_hover_text("Close the Data panel")
                    .clicked()
                {
                    actions.push(Action::ClosePanel);
                }
            });
        });
        ui.separator();
        // List the offline queue when its size changed since it was listed.
        let pending = self.cloud.pending;
        if self.cloud.account.is_some()
            && self.cloud.service.is_some()
            && pending > 0
            && self.cloud.queue_asked != Some(pending)
        {
            self.cloud.queue_asked = Some(pending);
            self.cloud_send(Request::LoadPending);
        }
        if let Some(account) = self.cloud.account.clone() {
            self.cloud_signed_in_ui(ui, &account, &mut actions);
        } else {
            ui.label(
                "R-CAD works fully offline. Connect to an R-CAD server to share projects, \
                 keep versions on a server and collaborate.",
            );
            ui.add_space(4.0);
            if self.cloud.service.is_some()
                && !matches!(self.cloud.dialog, Some(CloudDialog::SignIn { .. }))
            {
                ui.weak("Contacting the server…");
            }
            if ui
                .button("Connect to server…")
                .probe("Connect to server…")
                .clicked()
            {
                actions.push(Action::Connect);
            }
        }
        self.cloud_apply(actions);
    }

    fn cloud_signed_in_ui(
        &mut self,
        ui: &mut egui::Ui,
        account: &Account,
        actions: &mut Vec<Action>,
    ) {
        let c = &self.cloud;
        ui.label(RichText::new(&account.server_name).strong())
            .on_hover_text(&account.server_url);
        ui.horizontal(|ui| {
            ui.weak(&account.me.user.username);
            if ui.small_button("Refresh").probe("Refresh").clicked() {
                actions.push(Action::Refresh);
            }
            if ui.small_button("Sign out").probe("Sign out").clicked() {
                actions.push(Action::SignOut);
            }
        });
        if c.offline || c.pending > 0 {
            ui.horizontal(|ui| {
                let p = crate::theme::of(ui);
                ui.colored_label(
                    p.warn,
                    if c.offline {
                        format!("Offline · {} pending", c.pending)
                    } else {
                        format!("{} pending", c.pending)
                    },
                );
                if ui.small_button("Sync now").probe("Sync now").clicked() {
                    actions.push(Action::Send(Request::SyncPending));
                }
            });
            self.cloud_queue_ui(ui, actions);
        }
        let orgs = &account.me.orgs;
        let current = c
            .org
            .as_ref()
            .and_then(|o| orgs.iter().find(|x| x.id == *o))
            .map_or("Choose…".to_string(), |o| o.name.clone());
        ui.horizontal(|ui| {
            let l = ui.label("Organization");
            egui::ComboBox::from_id_salt("cloud_org")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for o in orgs {
                        if ui
                            .add(egui::Button::selectable(
                                c.org.as_ref() == Some(&o.id),
                                &o.name,
                            ))
                            .clicked()
                        {
                            actions.push(Action::SelectOrg(o.id.clone()));
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            if let Some(org) = c.org.clone()
                && self.cloud_org_role(&org) >= Some(proto::OrgRole::Admin)
                && ui
                    .small_button("Organization…")
                    .probe("Organization…")
                    .on_hover_text("Members, roles and storage of this organization")
                    .clicked()
            {
                actions.push(Action::Organization(org));
            }
        });
        ui.horizontal(|ui| {
            for (s, label) in [
                (Section::Browse, "Browse"),
                (Section::History, "History"),
                (Section::Comments, "Comments"),
            ] {
                if ui
                    .add(egui::Button::selectable(c.section == s, label))
                    .named(&format!("{label} section"))
                    .clicked()
                {
                    actions.push(Action::Section(s));
                }
            }
        });
        ui.separator();
        match c.section {
            Section::Browse => self.cloud_browse_ui(ui, actions),
            Section::History => self.cloud_history_ui(ui, actions),
            Section::Comments => self.cloud_comments_ui(ui, actions),
        }
    }

    fn cloud_browse_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let c = &self.cloud;
        ui.horizontal(|ui| {
            ui.strong("Projects");
            if ui
                .add_enabled(c.org.is_some(), egui::Button::new("New project…").small())
                .probe("New project…")
                .clicked()
            {
                actions.push(Action::Dialog(CloudDialog::NewProject {
                    name: String::new(),
                }));
            }
        });
        if c.projects.is_empty() {
            ui.weak("No projects yet.");
        }
        for p in &c.projects {
            let sel = c.project.as_ref() == Some(&p.id);
            if ui
                .add(egui::Button::selectable(sel, &p.name))
                .named(&format!("Project {}", p.name))
                .on_hover_text(&p.description)
                .clicked()
            {
                actions.push(Action::SelectProject(p.id.clone()));
            }
        }
        let Some(tree) = c.tree.clone() else {
            if c.project.is_some() {
                ui.weak("Loading…");
            }
            return;
        };
        ui.separator();
        let editor = tree.project.my_role >= proto::ProjectRole::Editor;
        let target = c
            .selected_folder
            .clone()
            .filter(|f| tree.folders.iter().any(|x| x.id == *f));
        ui.horizontal_wrapped(|ui| {
            ui.strong(&tree.project.name);
            if ui
                .small_button("Members…")
                .probe("Members…")
                .on_hover_text("People with access to this project and their roles")
                .clicked()
            {
                actions.push(Action::Members(tree.project.id.clone()));
            }
            if ui
                .add_enabled(editor, egui::Button::new("New folder…").small())
                .probe("New folder…")
                .on_hover_text("A new folder inside the selected folder (or the project root)")
                .on_disabled_hover_text("Editors create folders")
                .clicked()
            {
                actions.push(Action::Dialog(CloudDialog::NewFolder {
                    name: String::new(),
                    parent: target.clone(),
                }));
            }
            if ui
                .add(egui::Button::selectable(
                    c.show_trash,
                    format!("Trash ({})", tree.trash.len()),
                ))
                .named("Trash")
                .on_hover_text("Documents moved to the trash; editors restore them")
                .clicked()
            {
                actions.push(Action::ToggleTrash);
            }
        });
        if c.show_trash {
            self.cloud_trash_ui(ui, &tree, actions);
            return;
        }
        let can_upload = self.summary.open && self.sketch.is_none() && editor;
        if ui
            .add_enabled(can_upload, egui::Button::new("Upload current document…"))
            .probe("Upload current document…")
            .on_hover_text(
                "Upload the open document to this project (into the selected folder) \
                 as a new cloud document",
            )
            .on_disabled_hover_text("Open or create a document first (editors upload)")
            .clicked()
        {
            actions.push(Action::Upload);
        }
        if let Some(f) = &target {
            let path = self
                .cloud
                .folder_paths()
                .into_iter()
                .find(|(id, _)| id == f)
                .map_or_else(String::new, |(_, p)| p);
            let name = tree
                .folders
                .iter()
                .find(|x| x.id == *f)
                .map_or_else(String::new, |x| x.name.clone());
            ui.horizontal_wrapped(|ui| {
                ui.weak(format!("Folder: {path}"));
                if ui
                    .add_enabled(editor, egui::Button::new("Rename folder…").small())
                    .probe("Rename folder…")
                    .clicked()
                {
                    actions.push(Action::Dialog(CloudDialog::RenameFolder {
                        folder: f.clone(),
                        name: name.clone(),
                    }));
                }
                if ui
                    .add_enabled(editor, egui::Button::new("Delete folder…").small())
                    .probe("Delete folder…")
                    .on_hover_text("Delete this folder (it must be empty)")
                    .clicked()
                {
                    actions.push(Action::Dialog(CloudDialog::DeleteFolder {
                        folder: f.clone(),
                        name,
                    }));
                }
            });
        }
        let now = now_secs();
        self.cloud_folder_ui(ui, &tree, None, now, actions, 0);
        if let Some(doc) = self.cloud.selected_doc.clone()
            && let Some(d) = tree.documents.iter().find(|d| d.id == doc)
        {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("Open")
                    .probe("Open document")
                    .on_hover_text("Download the latest version and open it")
                    .clicked()
                {
                    actions.push(Action::Open(doc.clone()));
                }
                if ui
                    .add_enabled(editor, egui::Button::new("Rename…"))
                    .probe("Rename document…")
                    .on_disabled_hover_text("Editors rename documents")
                    .clicked()
                {
                    actions.push(Action::Dialog(CloudDialog::RenameDocument {
                        document: doc.clone(),
                        name: d.name.clone(),
                    }));
                }
                if ui
                    .add_enabled(editor, egui::Button::new("Move…"))
                    .probe("Move document…")
                    .on_hover_text("Move to another folder of this project")
                    .on_disabled_hover_text("Editors move documents")
                    .clicked()
                {
                    actions.push(Action::Dialog(CloudDialog::MoveDocument {
                        document: doc.clone(),
                        folder: d.folder.clone(),
                    }));
                }
                if ui
                    .add_enabled(editor, egui::Button::new("Move to trash…"))
                    .probe("Move document to trash…")
                    .on_disabled_hover_text("Editors move documents to the trash")
                    .clicked()
                {
                    actions.push(Action::Dialog(CloudDialog::TrashDocument {
                        project: tree.project.id.clone(),
                        document: doc.clone(),
                        name: d.name.clone(),
                    }));
                }
            });
        }
    }

    /// Folders and documents under `parent` (None: project root).
    fn cloud_folder_ui(
        &self,
        ui: &mut egui::Ui,
        tree: &proto::ProjectTree,
        parent: Option<&str>,
        now: i64,
        actions: &mut Vec<Action>,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        for f in tree
            .folders
            .iter()
            .filter(|f| f.parent.as_deref() == parent)
        {
            let id = ui.make_persistent_id(("cloud_folder", &f.id));
            let sel = self.cloud.selected_folder.as_deref() == Some(f.id.as_str());
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                .show_header(ui, |ui| {
                    if ui
                        .add(egui::Button::selectable(sel, f.name.as_str()))
                        .named(&format!("Folder {}", f.name))
                        .on_hover_text(
                            "Select: new folders and uploads go into the selected folder",
                        )
                        .clicked()
                    {
                        actions.push(Action::SelectFolder((!sel).then(|| f.id.clone())));
                    }
                })
                .body(|ui| {
                    self.cloud_folder_ui(ui, tree, Some(&f.id), now, actions, depth + 1);
                });
        }
        for d in tree
            .documents
            .iter()
            .filter(|d| d.folder.as_deref() == parent)
        {
            let sel = self.cloud.selected_doc.as_ref() == Some(&d.id);
            let lock = d.lock.as_ref().map_or("", |_| " 🔒");
            let text = format!("{}  v{} · {}{lock}", d.name, d.head_version, d.updated_by);
            let mut tip = format!(
                "Updated {} by {}",
                fmt_time(d.updated_at, now),
                d.updated_by
            );
            if let Some(l) = &d.lock {
                tip += &format!("\nLocked by {}", l.holder);
            }
            tip += "\nDouble-click to open";
            let thumb = if d.has_thumbnail && d.head_version > 0 {
                self.cloud_thumb(&d.id, d.head_version, actions)
            } else {
                None
            };
            let r = ui
                .horizontal(|ui| {
                    if let Some(t) = &thumb {
                        let _ = ui
                            .add(egui::Image::new(t).fit_to_exact_size(egui::vec2(24.0, 24.0)))
                            .named(&format!("Thumbnail of {}", d.name));
                    }
                    ui.add(egui::Button::selectable(sel, text))
                        .named(&format!("Document {}", d.name))
                        .on_hover_text(tip)
                })
                .inner;
            if r.double_clicked() {
                actions.push(Action::Open(d.id.clone()));
            } else if r.clicked() {
                actions.push(Action::SelectDoc(d.id.clone()));
            }
        }
    }

    fn cloud_history_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(doc) = self.cloud_target_doc() else {
            ui.weak("Open a cloud document to see its versions.");
            return;
        };
        let Some(detail) = self.cloud.detail.clone().filter(|d| d.document.id == doc) else {
            ui.weak("Loading…");
            if ui.small_button("Reload").clicked() {
                actions.push(Action::Send(Request::LoadDetail { document: doc }));
            }
            return;
        };
        ui.horizontal(|ui| {
            ui.strong(format!("Version History: {}", detail.document.name));
            if ui.small_button("Reload").probe("Reload history").clicked() {
                actions.push(Action::Send(Request::LoadDetail {
                    document: doc.clone(),
                }));
            }
        });
        let head = detail.document.head_version;
        let editor = detail.my_role >= proto::ProjectRole::Editor;
        let now = now_secs();
        for v in &detail.versions {
            let n = v.number;
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    let star = if v.milestone.is_some() { "★ " } else { "" };
                    ui.strong(format!("{star}v{n}"));
                    if n == head {
                        ui.weak("(latest)");
                    }
                    ui.label(&v.author);
                });
                ui.weak(fmt_time(v.created_at, now));
                if v.thumbnail.is_some()
                    && let Some(t) = self.cloud_thumb(&doc, n, actions)
                {
                    let _ = ui
                        .add(egui::Image::new(&t).fit_to_exact_size(egui::vec2(96.0, 96.0)))
                        .named(&format!("Thumbnail of v{n}"));
                }
                if let Some(m) = &v.milestone {
                    ui.label(format!("Milestone: {m}"));
                }
                if !v.description.is_empty() {
                    ui.label(&v.description);
                }
                if let Some(from) = v.promoted_from {
                    ui.weak(format!("Promoted from v{from}"));
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .small_button(format!("Open v{n}"))
                        .probe(&format!("Open v{n}"))
                        .on_hover_text("Open this version as a read-only copy")
                        .clicked()
                    {
                        actions.push(Action::Send(Request::OpenVersion {
                            document: doc.clone(),
                            version: n,
                        }));
                    }
                    if ui
                        .add_enabled(
                            editor && n != head,
                            egui::Button::new(format!("Promote v{n}")).small(),
                        )
                        .probe(&format!("Promote v{n}"))
                        .on_hover_text("Make this version the latest (saved as a new version)")
                        .clicked()
                    {
                        actions.push(Action::Dialog(CloudDialog::Promote {
                            document: doc.clone(),
                            version: n,
                        }));
                    }
                    if v.milestone.is_some() {
                        if ui
                            .add_enabled(
                                editor,
                                egui::Button::new(format!("Clear milestone v{n}")).small(),
                            )
                            .probe(&format!("Clear milestone v{n}"))
                            .clicked()
                        {
                            actions.push(Action::Send(Request::SetMilestone {
                                document: doc.clone(),
                                version: n,
                                milestone: None,
                            }));
                        }
                    } else if ui
                        .add_enabled(
                            editor,
                            egui::Button::new(format!("Set milestone v{n}…")).small(),
                        )
                        .probe(&format!("Set milestone v{n}…"))
                        .clicked()
                    {
                        actions.push(Action::Dialog(CloudDialog::Milestone {
                            document: doc.clone(),
                            version: n,
                            name: String::new(),
                        }));
                    }
                });
            });
        }
    }

    fn cloud_comments_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(doc) = self.cloud_target_doc() else {
            ui.weak("Open a cloud document to see its comments.");
            return;
        };
        let comments = match &self.cloud.comments {
            Some((d, list)) if *d == doc => list.clone(),
            _ => {
                ui.weak("Loading…");
                if ui.small_button("Reload").clicked() {
                    actions.push(Action::Send(Request::LoadComments { document: doc }));
                }
                return;
            }
        };
        if comments.is_empty() {
            ui.weak("No comments yet.");
        }
        let now = now_secs();
        for (i, cm) in comments.iter().enumerate() {
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&cm.author);
                    ui.weak(format!(
                        "v{} · {}",
                        cm.version,
                        fmt_time(cm.created_at, now)
                    ));
                });
                ui.label(&cm.body);
                let mut resolved = cm.resolved;
                if ui
                    .checkbox(&mut resolved, "Resolved")
                    .named(&format!("Resolve comment {}", i + 1))
                    .changed()
                {
                    actions.push(Action::Send(Request::ResolveComment {
                        document: doc.clone(),
                        comment: cm.id.clone(),
                        resolved,
                    }));
                }
            });
        }
        ui.add(
            egui::TextEdit::multiline(&mut self.cloud.comment_draft)
                .desired_rows(2)
                .hint_text("Write a comment"),
        )
        .named("New comment");
        if ui
            .add_enabled(
                !self.cloud.comment_draft.trim().is_empty(),
                egui::Button::new("Add comment"),
            )
            .probe("Add comment")
            .clicked()
        {
            actions.push(Action::AddComment);
        }
    }

    fn cloud_apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Send(r) => self.cloud_send(r),
                Action::Dialog(d) => self.cloud.dialog = Some(d),
                Action::SelectOrg(org) => {
                    self.cloud.org = Some(org.clone());
                    self.cloud.projects.clear();
                    self.cloud.project = None;
                    self.cloud.tree = None;
                    self.cloud_send(Request::ListProjects { org });
                }
                Action::SelectProject(p) => {
                    self.cloud.project = Some(p.clone());
                    self.cloud.tree = None;
                    self.cloud.selected_doc = None;
                    self.cloud.selected_folder = None;
                    self.cloud.show_trash = false;
                    self.cloud_send(Request::LoadTree { project: p });
                }
                Action::SelectDoc(d) => self.cloud.selected_doc = Some(d),
                Action::Open(d) => {
                    self.cloud.selected_doc = Some(d.clone());
                    self.cloud_send(Request::OpenDocument { document: d });
                }
                Action::Section(s) => {
                    self.cloud.section = s;
                    match s {
                        Section::History => {
                            if let Some(doc) = self.cloud_target_doc() {
                                self.cloud_send(Request::LoadDetail { document: doc });
                            }
                        }
                        Section::Comments => {
                            if let Some(doc) = self.cloud_target_doc() {
                                self.cloud_send(Request::LoadComments { document: doc });
                            }
                        }
                        Section::Browse => {}
                    }
                }
                Action::Upload => self.cloud_upload_dialog(),
                Action::SignOut => self.cloud_send(Request::SignOut),
                Action::Refresh => {
                    self.cloud_send(Request::Refresh);
                    if let Some(project) = self.cloud.project.clone() {
                        self.cloud_send(Request::LoadTree { project });
                    }
                }
                Action::Connect => self.cloud_connect_dialog(),
                Action::SelectFolder(f) => self.cloud.selected_folder = f,
                Action::ToggleTrash => self.cloud.show_trash = !self.cloud.show_trash,
                Action::Members(p) => self.cloud_open_members(p),
                Action::LoadThumbnail(document, version) => {
                    if self.cloud.thumb_asked.insert((document.clone(), version)) {
                        self.cloud_send(Request::LoadThumbnail { document, version });
                    }
                }
                Action::Update(document) => {
                    self.cloud_send(Request::OpenDocument { document });
                }
                Action::Organization(o) => self.cloud_open_organization(o),
                Action::ClosePanel => self.cloud.panel_open = false,
                Action::AddComment => {
                    let body = std::mem::take(&mut self.cloud.comment_draft);
                    if let Some(link) = self.cloud_link() {
                        let (document, version) = (link.document.clone(), link.base_version);
                        self.cloud_send(Request::AddComment {
                            document,
                            version,
                            body: body.trim().to_string(),
                        });
                    }
                }
            }
        }
    }

    /// Cloud dialogs and the conflict window.
    pub(crate) fn cloud_windows(&mut self, ctx: &egui::Context) {
        self.cloud_conflict_ui(ctx);
        match &self.cloud.dialog {
            Some(CloudDialog::Members { .. }) => return self.cloud_members_window(ctx),
            Some(CloudDialog::Organization { .. }) => return self.cloud_org_window(ctx),
            _ => {}
        }
        let Some(mut d) = self.cloud.dialog.clone() else {
            return;
        };
        let before_project = match &d {
            CloudDialog::SaveToCloud { project, .. } => project.clone(),
            _ => None,
        };
        let projects: Vec<(String, String)> = self
            .cloud
            .projects
            .iter()
            .filter(|p| p.my_role >= proto::ProjectRole::Editor)
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let tree_project = self.cloud.tree.as_ref().map(|t| t.project.id.clone());
        let saved_at = self.summary.path.clone();
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let mut ok = false;
        let mut cancel = esc;
        let folders = self.cloud.folder_paths();
        let versions: Vec<u64> = self
            .cloud
            .detail
            .as_ref()
            .filter(|det| Some(&det.document.id) == self.cloud_target_doc().as_ref())
            .map(|det| det.versions.iter().map(|v| v.number).collect())
            .unwrap_or_default();
        let head = self
            .cloud
            .detail
            .as_ref()
            .map_or(0, |det| det.document.head_version);
        let share_url = self.cloud.share_url.clone();
        let title = match &d {
            CloudDialog::SignIn { .. } => "Connect to server",
            CloudDialog::NewProject { .. } => "New project",
            CloudDialog::NewFolder { .. } => "New folder",
            CloudDialog::Upload { .. } => "Upload to server",
            CloudDialog::SaveVersion { .. } => "Save Version to Cloud",
            CloudDialog::Share { .. } => "Share link",
            CloudDialog::Promote { .. } => "Promote version",
            CloudDialog::Milestone { .. } => "Set milestone",
            CloudDialog::Members { .. } | CloudDialog::Organization { .. } => "",
            CloudDialog::RenameDocument { .. } => "Rename a document",
            CloudDialog::MoveDocument { .. } => "Move a document",
            CloudDialog::TrashDocument { .. } => "Move a document to the trash",
            CloudDialog::RenameFolder { .. } => "Rename a folder",
            CloudDialog::DeleteFolder { .. } => "Delete a folder",
            CloudDialog::DiscardPending { .. } => "Discard queued version",
            CloudDialog::SaveToCloud { .. } => "Save to Cloud Project",
        };
        let folder_combo = |ui: &mut egui::Ui, label: &str, value: &mut Option<String>| {
            let current = value
                .as_ref()
                .and_then(|v| folders.iter().find(|(id, _)| id == v))
                .map_or("(project root)".to_string(), |(_, n)| n.clone());
            let r = crate::form::combo(ui, label, current, |ui| {
                let _ = ui
                    .selectable_value(value, None, "(project root)")
                    .probe(&format!("{label}: (project root)"));
                for (id, name) in &folders {
                    let _ = ui
                        .selectable_value(value, Some(id.clone()), name)
                        .probe(&format!("{label}: {name}"));
                }
            });
            let _ = r.response.probe(&format!("Folder list: {label}"));
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .show(ctx, |ui| {
                crate::form::form(ui, "cloud_dialog", |ui| match &mut d {
                    CloudDialog::SignIn {
                        server_url,
                        username,
                        password,
                        allow_insecure,
                        error,
                        sent,
                    } => {
                        let url = field(ui, "Server URL", server_url, false);
                        if server_url.is_empty() && !url.has_focus() && username.is_empty() {
                            url.request_focus();
                        }
                        field(ui, "Username", username, false);
                        field(ui, "Password", password, true);
                        if insecure_url(server_url.trim()) {
                            let bad = crate::theme::of(ui).bad;
                            ui.scope(|ui| {
                                ui.set_max_width(360.0);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(rcad_cloud::client::INSECURE_HTTP_WARNING)
                                            .color(bad),
                                    )
                                    .wrap(),
                                );
                            });
                            let _ = ui
                                .checkbox(allow_insecure, "Allow unencrypted connection")
                                .probe("Allow unencrypted connection");
                        } else {
                            *allow_insecure = false;
                        }
                        if let Some(e) = error {
                            ui.colored_label(crate::theme::of(ui).bad, e.as_str());
                        }
                        if *sent {
                            ui.weak("Connecting…");
                        }
                        ui.small("Your password is sent once; R-CAD keeps a revocable token.");
                        ui.horizontal(|ui| {
                            ok |= ui
                                .add_enabled(!*sent, egui::Button::new("Connect"))
                                .probe("Connect")
                                .clicked()
                                || (enter && !*sent);
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::NewProject { name } => {
                        field(ui, "Project name", name, false);
                        ui.horizontal(|ui| {
                            ok |= ui
                                .button("Create project")
                                .probe("Create project")
                                .clicked()
                                || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::NewFolder { name, parent } => {
                        field(ui, "Folder name", name, false);
                        folder_combo(ui, "Inside", parent);
                        ui.horizontal(|ui| {
                            ok |= ui.button("Create folder").probe("Create folder").clicked()
                                || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::Upload { name, folder } => {
                        field(ui, "Document name", name, false);
                        folder_combo(ui, "Folder", folder);
                        ui.small("The document is saved locally first if it has unsaved changes.");
                        ui.horizontal(|ui| {
                            ok |= ui.button("Upload").probe("Upload").clicked() || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::SaveVersion {
                        description,
                        milestone,
                    } => {
                        field(ui, "Version description", description, false);
                        field(ui, "Milestone name (optional)", milestone, false);
                        ui.small("Unsaved changes are saved locally first.");
                        ui.horizontal(|ui| {
                            ok |=
                                ui.button("Save Version").probe("Save Version").clicked() || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::Share {
                        days,
                        allow_download,
                        version,
                        sent,
                    } => {
                        field(ui, "Expires after (days)", days, false);
                        crate::form::check(ui, allow_download, "Allow download");
                        let current = version.map_or("Latest".to_string(), |v| format!("v{v}"));
                        crate::form::combo(ui, "Version", current, |ui| {
                            ui.selectable_value(version, None, "Latest");
                            for v in &versions {
                                ui.selectable_value(version, Some(*v), format!("v{v}"));
                            }
                        });
                        if let Some(url) = &share_url {
                            ui.separator();
                            let mut shown = url.clone();
                            ui.add(egui::TextEdit::singleline(&mut shown).desired_width(320.0))
                                .named("Share URL");
                            if ui.button("Copy").probe("Copy").clicked() {
                                ui.ctx().copy_text(url.clone());
                            }
                        }
                        ui.horizontal(|ui| {
                            ok |= ui
                                .add_enabled(!*sent, egui::Button::new("Create link"))
                                .probe("Create link")
                                .clicked();
                            cancel |= ui
                                .button(if share_url.is_some() {
                                    "Close"
                                } else {
                                    "Cancel"
                                })
                                .clicked();
                        });
                    }
                    CloudDialog::Promote { version, .. } => {
                        ui.label(format!(
                            "Version {version} becomes the latest version (saved as version {} \
                             with the same content). Continue?",
                            head + 1
                        ));
                        ui.horizontal(|ui| {
                            ok |= ui.button("Promote").probe("Confirm promote").clicked() || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::Milestone { name, .. } => {
                        field(ui, "Milestone name", name, false);
                        ui.horizontal(|ui| {
                            ok |= ui.button("Set milestone").probe("Set milestone").clicked()
                                || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::Members { .. } | CloudDialog::Organization { .. } => {}
                    CloudDialog::RenameDocument { name, .. } => {
                        field(ui, "New name", name, false);
                        ui.small("Names are unique within a project.");
                        ui.horizontal(|ui| {
                            ok |= ui.button("Rename").probe("Rename document").clicked() || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::MoveDocument { folder, .. } => {
                        folder_combo(ui, "Move to", folder);
                        ui.horizontal(|ui| {
                            ok |= ui.button("Move").probe("Move document").clicked() || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::TrashDocument { name, .. } => {
                        ui.label(format!(
                            "Move {name} to the project's trash? Editors can restore it \
                             from Trash; its versions are kept."
                        ));
                        ui.horizontal(|ui| {
                            ok |= ui
                                .button("Move to trash")
                                .probe("Confirm move to trash")
                                .clicked()
                                || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::RenameFolder { name, .. } => {
                        field(ui, "New folder name", name, false);
                        ui.horizontal(|ui| {
                            ok |= ui.button("Rename folder").probe("Rename folder").clicked()
                                || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::DeleteFolder { name, .. } => {
                        ui.label(format!(
                            "Delete the folder {name}? Only an empty folder can be deleted \
                             (documents in the trash still count)."
                        ));
                        ui.horizontal(|ui| {
                            ok |= ui.button("Delete folder").probe("Delete folder").clicked()
                                || enter;
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::DiscardPending { what, .. } => {
                        ui.label(format!(
                            "Discard the queued version \"{what}\"? It will not be sent to the \
                             server. Your local file is not changed."
                        ));
                        ui.horizontal(|ui| {
                            ok |= ui.button("Discard").named("Confirm discard").clicked();
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                    CloudDialog::SaveToCloud {
                        project,
                        folder,
                        name,
                    } => {
                        field(ui, "Document name", name, false);
                        let current = project
                            .as_ref()
                            .and_then(|p| projects.iter().find(|(id, _)| id == p))
                            .map_or("Choose…".to_string(), |(_, n)| n.clone());
                        crate::form::combo(ui, "Project", current, |ui| {
                            for (id, n) in &projects {
                                ui.selectable_value(project, Some(id.clone()), n);
                            }
                        });
                        if projects.is_empty() {
                            ui.weak("No project you can edit; create one in the Data panel.");
                        }
                        if project.is_some() && *project == tree_project {
                            folder_combo(ui, "Folder", folder);
                        } else if project.is_some() {
                            ui.weak("Loading folders…");
                        }
                        if let Some(p) = &saved_at {
                            ui.small(format!(
                                "The cloud document links to {}; unsaved changes are saved \
                                 there first.",
                                p.display()
                            ));
                        }
                        ui.horizontal(|ui| {
                            ok |= ui
                                .add_enabled(
                                    project.is_some() && !name.trim().is_empty(),
                                    egui::Button::new("Save to Cloud"),
                                )
                                .probe("Save to Cloud")
                                .clicked();
                            cancel |= ui.button("Cancel").clicked();
                        });
                    }
                });
            });
        // Choosing another project in Save to Cloud loads its folders.
        if let CloudDialog::SaveToCloud {
            project: Some(p),
            folder,
            ..
        } = &mut d
            && before_project.as_ref() != Some(&*p)
        {
            *folder = None;
            let p = p.clone();
            self.cloud_apply(vec![Action::SelectProject(p)]);
        }
        // Keep edits made this frame unless the dialog was replaced meanwhile.
        if self.cloud.dialog.is_some() {
            self.cloud.dialog = Some(d.clone());
        }
        if cancel {
            self.cloud.dialog = None;
            return;
        }
        if !ok {
            return;
        }
        match d {
            CloudDialog::SignIn { .. } => self.cloud_sign_in(),
            CloudDialog::NewProject { name } => {
                if let (Some(org), false) = (self.cloud.org.clone(), name.trim().is_empty()) {
                    self.cloud.dialog = None;
                    self.cloud_send(Request::CreateProject {
                        org,
                        name: name.trim().to_string(),
                    });
                }
            }
            CloudDialog::NewFolder { name, parent } => {
                if let (Some(project), false) = (self.cloud.project.clone(), name.trim().is_empty())
                {
                    self.cloud.dialog = None;
                    self.cloud_send(Request::CreateFolder {
                        project,
                        parent,
                        name: name.trim().to_string(),
                    });
                }
            }
            CloudDialog::Upload { name, folder } => {
                if !name.trim().is_empty() {
                    self.cloud.dialog = None;
                    self.cloud_upload(name.trim().to_string(), folder);
                }
            }
            CloudDialog::SaveVersion {
                description,
                milestone,
            } => {
                self.cloud.dialog = None;
                let m = milestone.trim();
                self.cloud_commit_version(
                    description.trim().to_string(),
                    (!m.is_empty()).then(|| m.to_string()),
                );
            }
            CloudDialog::Share {
                days,
                allow_download,
                version,
                ..
            } => {
                let days = days.trim();
                let expires = if days.is_empty() {
                    Ok(None)
                } else {
                    days.parse::<u32>()
                        .ok()
                        .filter(|d| *d > 0)
                        .map(Some)
                        .ok_or(())
                };
                match (expires, self.cloud_target_doc()) {
                    (Ok(expires_in_days), Some(document)) => {
                        if let Some(CloudDialog::Share { sent, .. }) = &mut self.cloud.dialog {
                            *sent = true;
                        }
                        self.cloud.share_url = None;
                        self.cloud_send(Request::CreateShare {
                            document,
                            version,
                            expires_in_days,
                            allow_download,
                        });
                    }
                    (Err(()), _) => self.notify(
                        "Share link: the expiry is a whole number of days (empty: never)".into(),
                    ),
                    _ => {}
                }
            }
            CloudDialog::Promote { document, version } => {
                self.cloud.dialog = None;
                self.cloud_send(Request::Promote { document, version });
            }
            CloudDialog::Milestone {
                document,
                version,
                name,
            } => {
                if !name.trim().is_empty() {
                    self.cloud.dialog = None;
                    self.cloud_send(Request::SetMilestone {
                        document,
                        version,
                        milestone: Some(name.trim().to_string()),
                    });
                }
            }
            CloudDialog::Members { .. } | CloudDialog::Organization { .. } => {}
            CloudDialog::RenameDocument { document, name } => {
                if !name.trim().is_empty() {
                    self.cloud.dialog = None;
                    self.cloud_send(Request::RenameDocument {
                        document,
                        name: name.trim().to_string(),
                    });
                }
            }
            CloudDialog::MoveDocument { document, folder } => {
                self.cloud.dialog = None;
                self.cloud_send(Request::MoveDocument { document, folder });
            }
            CloudDialog::TrashDocument {
                project, document, ..
            } => {
                self.cloud.dialog = None;
                self.cloud_send(Request::TrashDocument { project, document });
            }
            CloudDialog::RenameFolder { folder, name } => {
                if let (Some(project), false) = (self.cloud.project.clone(), name.trim().is_empty())
                {
                    self.cloud.dialog = None;
                    self.cloud_send(Request::RenameFolder {
                        project,
                        folder,
                        name: name.trim().to_string(),
                    });
                }
            }
            CloudDialog::DeleteFolder { folder, .. } => {
                if let Some(project) = self.cloud.project.clone() {
                    self.cloud.dialog = None;
                    self.cloud_send(Request::DeleteFolder { project, folder });
                }
            }
            CloudDialog::DiscardPending {
                index,
                document,
                blob,
                ..
            } => {
                self.cloud.dialog = None;
                self.cloud_send(Request::DiscardPending {
                    index,
                    document,
                    blob,
                });
            }
            CloudDialog::SaveToCloud {
                project: Some(project),
                folder,
                name,
            } => {
                if !name.trim().is_empty() {
                    self.cloud.dialog = None;
                    self.cloud_upload_to(project, name.trim().to_string(), folder);
                }
            }
            CloudDialog::SaveToCloud { project: None, .. } => {}
        }
    }

    fn cloud_conflict_ui(&mut self, ctx: &egui::Context) {
        let Some(cf) = self.cloud.conflict.clone() else {
            return;
        };
        #[derive(PartialEq)]
        enum Choice {
            Force,
            Copy,
            Cancel,
        }
        let mut choice = None;
        egui::Window::new("Version conflict")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "Version {} was saved by someone else after version {} that you started from.",
                    cf.head_version, cf.base_version
                ));
                ui.label("Your local file is unchanged.");
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("Save as version {} anyway", cf.head_version + 1))
                        .clicked()
                    {
                        choice = Some(Choice::Force);
                    }
                    if ui.button("Save as new document…").clicked() {
                        choice = Some(Choice::Copy);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(Choice::Cancel);
                    }
                });
            });
        if self.cloud.dialog.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(Choice::Cancel);
        }
        let Some(choice) = choice else { return };
        self.cloud.conflict = None;
        match choice {
            Choice::Cancel => {}
            Choice::Force => {
                let (description, milestone) = self.cloud.last_version.clone().unwrap_or_default();
                self.cloud_send(Request::SaveVersion {
                    local_path: cf.local_path,
                    description,
                    milestone,
                    force: true,
                    thumbnail: None,
                });
            }
            Choice::Copy => {
                let link = self.cloud.link_for(&cf.local_path).cloned();
                let Some(link) = link else {
                    self.cloud_log(true, "the conflicting document is no longer linked".into());
                    return;
                };
                let folder = self.cloud.folder_of(&cf.document);
                self.cloud_send(Request::UploadNew {
                    local_path: cf.local_path,
                    project: link.project,
                    folder,
                    name: format!("{} (copy)", link.document_name),
                    thumbnail: None,
                });
            }
        }
    }

    /// Preferences ▸ Cloud. Returns whether a preference changed.
    pub(crate) fn cloud_prefs_ui(&mut self, ui: &mut egui::Ui) -> bool {
        ui.label(RichText::new("Cloud").strong());
        ui.checkbox(
            &mut self.shell.prefs.cloud_auto_lock,
            "Lock cloud documents while I edit them",
        )
        .named("Pref cloud auto-lock")
        .on_hover_text(
            "Editing a cloud document asks the server for its edit lock; saving a version \
             releases it. Others can still open it; local editing is never blocked.",
        )
        .changed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_format_is_utc_civil_date() {
        // 2026-10-17 08:05:00 UTC
        let t = 1_792_224_300;
        assert_eq!(fmt_time(t, t + 30), "2026-10-17 08:05 UTC (just now)");
        assert_eq!(fmt_time(t, t + 7200), "2026-10-17 08:05 UTC (2 h ago)");
        assert_eq!(fmt_time(0, 0), "1970-01-01 00:00 UTC (just now)");
        assert_eq!(
            fmt_time(951_782_400, 951_782_400 + 3 * 86_400),
            "2000-02-29 00:00 UTC (3 d ago)"
        );
    }

    #[test]
    fn stems_and_accounts() {
        assert_eq!(file_stem(Path::new("/x/Bracket.rcad.json")), "Bracket");
        assert_eq!(file_stem(Path::new("/x/a.json")), "a");
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cloud.json");
        assert!(!adapter::has_account(&p));
        std::fs::write(&p, r#"{"version": 1, "accounts": []}"#).unwrap();
        assert!(!adapter::has_account(&p));
        std::fs::write(
            &p,
            r#"{"version": 1, "accounts": [{"server_url": "https://x", "server_name": "x",
                "username": "u", "token": "t"}]}"#,
        )
        .unwrap();
        assert!(adapter::has_account(&p));
        std::fs::write(&p, "not json").unwrap();
        assert!(!adapter::has_account(&p));
    }
}
