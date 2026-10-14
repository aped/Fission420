//! Native file dialogs for path fields. A path field keeps its text box (typed paths, the
//! live walkthroughs and tests still use it) and gains a **Browse…** button that opens the
//! platform dialog (`rfd`: the XDG desktop portal on Linux, so it works on Wayland and in
//! sandboxes) and writes the chosen path into the same field.
//!
//! The dialog blocks, so it runs on a worker thread and the UI keeps rendering; the result is
//! delivered to the field that asked (by its label) on a later frame. One dialog at a time. The
//! backend is swappable so tests can stand in for the platform dialog.
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, OnceLock};

/// What the dialog chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickMode {
    /// An existing file.
    Open,
    /// A file to write (the dialog asks before replacing one).
    Save,
    /// A folder.
    Folder,
}

/// File-type filters: (description, extensions without the dot).
pub type Filters = &'static [(&'static str, &'static [&'static str])];

/// One dialog request.
#[derive(Clone, Debug)]
pub struct PickRequest {
    pub mode: PickMode,
    pub title: String,
    pub filters: Filters,
    /// Folder the dialog starts in.
    pub start_dir: Option<PathBuf>,
    /// Suggested file name (Save).
    pub file_name: Option<String>,
}

/// Shows a dialog and blocks until it closes; `None` when cancelled or unavailable.
/// Called on a worker thread.
pub trait PickBackend: Send + Sync {
    fn pick(&self, req: &PickRequest) -> Option<PathBuf>;
}

/// The platform dialog.
struct Native;

impl PickBackend for Native {
    fn pick(&self, req: &PickRequest) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new().set_title(&req.title);
        for (name, exts) in req.filters {
            d = d.add_filter(*name, exts);
        }
        if let Some(dir) = &req.start_dir {
            d = d.set_directory(dir);
        }
        if let Some(name) = &req.file_name {
            d = d.set_file_name(name);
        }
        match req.mode {
            PickMode::Open => d.pick_file(),
            PickMode::Save => d.save_file(),
            PickMode::Folder => d.pick_folder(),
        }
    }
}

struct Hub {
    backend: Arc<dyn PickBackend>,
    /// Field label of the open dialog and the channel its result arrives on.
    pending: Option<(String, Receiver<Option<PathBuf>>)>,
    /// A chosen path not yet taken by its field.
    ready: Option<(String, PathBuf)>,
}

fn hub() -> &'static Mutex<Hub> {
    static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
    HUB.get_or_init(|| {
        Mutex::new(Hub {
            backend: Arc::new(Native),
            pending: None,
            ready: None,
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, Hub> {
    hub().lock().unwrap_or_else(|e| e.into_inner())
}

/// Replace the dialog backend (tests). Drops any pending result.
pub fn set_backend(backend: Arc<dyn PickBackend>) {
    let mut h = lock();
    h.backend = backend;
    h.pending = None;
    h.ready = None;
}

/// True while a dialog is open.
pub fn busy() -> bool {
    poll();
    lock().pending.is_some()
}

/// Open a dialog for the field `key`, unless one is already open.
pub fn request(ctx: &egui::Context, key: &str, req: PickRequest) {
    let mut h = lock();
    if h.pending.is_some() {
        return;
    }
    let (tx, rx) = channel();
    let backend = h.backend.clone();
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("file-dialog".into())
        .spawn(move || {
            let _ = tx.send(backend.pick(&req));
            ctx.request_repaint();
        });
    if spawned.is_ok() {
        h.pending = Some((key.to_string(), rx));
    }
}

fn poll() {
    let mut h = lock();
    let Some((key, rx)) = h.pending.take() else {
        return;
    };
    match rx.try_recv() {
        Ok(Some(path)) => h.ready = Some((key, path)),
        Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        Err(std::sync::mpsc::TryRecvError::Empty) => h.pending = Some((key, rx)),
    }
}

/// The path chosen for field `key`, once.
pub fn take(key: &str) -> Option<PathBuf> {
    poll();
    let mut h = lock();
    match &h.ready {
        Some((k, _)) if k == key => h.ready.take().map(|(_, p)| p),
        _ => None,
    }
}

/// Start folder and suggested name from what the field holds now: its parent folder when that
/// exists, and its file name for a save.
pub fn start_from(current: &str) -> (Option<PathBuf>, Option<String>) {
    let p = Path::new(current.trim());
    if current.trim().is_empty() {
        return (None, None);
    }
    if p.is_dir() {
        return (Some(p.to_path_buf()), None);
    }
    let dir = p.parent().filter(|d| d.is_dir()).map(Path::to_path_buf);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
    (dir, name)
}

/// Filters for the fields that use them.
pub const DOCUMENT: Filters = &[("R-CAD document", &["json"])];
pub const IMPORT: Filters = &[
    (
        "Supported files",
        &["step", "stp", "stl", "3mf", "dxf", "svg"],
    ),
    ("STEP", &["step", "stp"]),
    ("STL", &["stl"]),
    ("3MF", &["3mf"]),
    ("DXF", &["dxf"]),
    ("SVG", &["svg"]),
];
pub const EXTERNAL: Filters = &[
    ("R-CAD document or STEP assembly", &["json", "step", "stp"]),
    ("R-CAD document", &["json"]),
    ("STEP", &["step", "stp"]),
];
pub const STEP: Filters = &[("STEP", &["step", "stp"])];
pub const STL: Filters = &[("STL", &["stl"])];
pub const THREEMF: Filters = &[("3MF", &["3mf"])];
pub const DXF: Filters = &[("DXF", &["dxf"])];
pub const SVG: Filters = &[("SVG", &["svg"])];
pub const CSV: Filters = &[("CSV", &["csv"])];
pub const DRAWING: Filters = &[("PDF, SVG or DXF", &["pdf", "svg", "dxf"])];
pub const NONE: Filters = &[];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_folder_and_name_from_the_field() {
        let tmp = std::env::temp_dir();
        let f = tmp.join("part.step");
        let (dir, name) = start_from(&f.to_string_lossy());
        assert_eq!(dir.as_deref(), Some(tmp.as_path()));
        assert_eq!(name.as_deref(), Some("part.step"));
        assert_eq!(start_from("  "), (None, None));
        let (dir, name) = start_from(&tmp.to_string_lossy());
        assert_eq!((dir.as_deref(), name), (Some(tmp.as_path()), None));
        // A relative name with no existing folder: no start folder, the name is kept.
        assert_eq!(
            start_from("no/such/dir/x.csv"),
            (None, Some("x.csv".to_string()))
        );
    }
}
