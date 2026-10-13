//! Autosave and recovery (SESSION_4 §7.7). While a document has unsaved
//! changes, a copy is written every [`Autosave::interval`] to the recovery
//! directory with `save_copy` (atomic; the document's own path, dirty state and
//! undo history are untouched). A successful Save, or deliberately leaving the
//! document (New/Open/Close after the unsaved-changes prompt, Quit), removes
//! the copy; a crash leaves it, and the next start offers to open it as an
//! unsaved document (`open_copy`) or discard it.
//!
//! Directory: `RCAD_RECOVERY_DIR`, else `$XDG_STATE_HOME/r-cad/recovery`,
//! else `~/.local/state/r-cad/recovery`. Interval: `RCAD_AUTOSAVE_SECS`
//! (default 60; `0` = after every committed change, for tests).
//!
//! Before the rename to R-CAD (2026-10-13) these were `F420_RECOVERY_DIR`,
//! `F420_AUTOSAVE_SECS` and `.../fission420/recovery`, with copies named
//! `*.autosave.f420.json`. The old variables are still read when the new ones are
//! unset, and copies left in the old directory are still offered at start-up.
use crate::app::RcadApp;
use crate::probe::Probe as _;
use eframe::egui::{self, RichText};
use rcad_commands::Command;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SUFFIX: &str = ".autosave.rcad.json";
/// Recovery copies written before the rename.
const LEGACY_SUFFIX: &str = ".autosave.f420.json";

/// `(recovery dir, pre-rename recovery dir to also search)` from an environment lookup.
/// The legacy dir is only derived from XDG/HOME; an explicit directory variable wins
/// outright.
pub fn recovery_dirs(
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> (Option<PathBuf>, Option<PathBuf>) {
    if let Some(d) = var("RCAD_RECOVERY_DIR").or_else(|| var("F420_RECOVERY_DIR")) {
        return (Some(PathBuf::from(d)), None);
    }
    let base = var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".local/state")));
    match base {
        Some(b) => (
            Some(b.join("r-cad/recovery")),
            Some(b.join("fission420/recovery")),
        ),
        None => (None, None),
    }
}

/// Autosave interval: `RCAD_AUTOSAVE_SECS`, else `F420_AUTOSAVE_SECS`, else 60 s.
pub fn autosave_interval(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Duration {
    var("RCAD_AUTOSAVE_SECS")
        .or_else(|| var("F420_AUTOSAVE_SECS"))
        .and_then(|v| v.to_str()?.parse::<u64>().ok())
        .map_or(Duration::from_secs(60), Duration::from_secs)
}

#[derive(Debug)]
pub struct Autosave {
    /// Off unless the shell enables it ([`RcadApp::enable_autosave`]); tests
    /// and scripted scenes never write recovery files.
    pub enabled: bool,
    pub dir: Option<PathBuf>,
    /// Pre-rename recovery directory, searched (never written) at start-up.
    pub legacy_dir: Option<PathBuf>,
    pub interval: Duration,
    /// (doc id, revision) last written, and where.
    last: Option<(u64, u64, PathBuf)>,
    last_time: Option<Instant>,
    /// Recovery copies found at start-up (offered until opened/discarded).
    pub offered: Vec<PathBuf>,
    /// A recovery copy opened with `open_copy`: removed once the reopened
    /// document's own first copy has been written.
    pub(crate) replaced: Option<PathBuf>,
    /// Open Copy requested, not yet answered (becomes `replaced` on success).
    pub(crate) opening: Option<PathBuf>,
    /// Copies removed because they are no longer needed while a `SaveCopy`
    /// to them may still be queued: removed again when it lands, and at exit
    /// (review run 4 #19: Quit right after an autosave recreated a stale copy).
    pub(crate) dropped: Vec<PathBuf>,
}

impl Default for Autosave {
    fn default() -> Self {
        let (dir, legacy_dir) = recovery_dirs(|k| std::env::var_os(k));
        let interval = autosave_interval(|k| std::env::var_os(k));
        Self {
            enabled: false,
            dir,
            legacy_dir,
            interval,
            last: None,
            last_time: None,
            offered: vec![],
            replaced: None,
            opening: None,
            dropped: Vec::new(),
        }
    }
}

/// Recovery copies in `dir`, newest first.
pub fn find_copies(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let s = p.to_string_lossy();
            s.ends_with(SUFFIX) || s.ends_with(LEGACY_SUFFIX)
        })
        .map(|p| {
            let t = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (t, p)
        })
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.0));
    v.into_iter().map(|(_, p)| p).collect()
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

impl RcadApp {
    /// Turn autosave on (the desktop shell does) and offer the recovery
    /// copies left by an earlier session.
    pub fn enable_autosave(&mut self) {
        self.autosave.enabled = true;
        let a = &self.autosave;
        self.autosave.offered = [a.dir.as_deref(), a.legacy_dir.as_deref()]
            .into_iter()
            .flatten()
            .flat_map(find_copies)
            .collect();
    }

    /// Where the open document's recovery copy goes: the file stem plus a
    /// hash of its full path and this process id, so two documents with the
    /// same name, two app instances, or a reopened document never overwrite
    /// a copy left by a crashed session (review run 4 #12).
    fn autosave_path(&self) -> Option<PathBuf> {
        let dir = self.autosave.dir.as_ref()?;
        let (stem, full) = match &self.summary.path {
            Some(p) => (
                p.file_name()
                    .map(|n| {
                        n.to_string_lossy()
                            .trim_end_matches(".rcad.json")
                            .to_string()
                    })
                    .unwrap_or_else(|| "document".into()),
                p.canonicalize()
                    .unwrap_or_else(|_| p.clone())
                    .display()
                    .to_string(),
            ),
            None => ("untitled".to_string(), String::new()),
        };
        // FNV-1a 64 of the path (identity only, not security).
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in full.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let name = format!(
            "{}-{:08x}-{}{SUFFIX}",
            sanitize(&stem),
            h as u32,
            std::process::id()
        );
        let p = dir.join(name);
        // Never write over a copy that is still on offer.
        (!self.autosave.offered.contains(&p)).then_some(p)
    }

    /// Once per frame: write a recovery copy when due.
    pub(crate) fn autosave_tick(&mut self) {
        let s = &self.summary;
        if !self.autosave.enabled || !s.open || !s.dirty || self.busy() || self.sketch.is_some() {
            return;
        }
        let key = (s.doc_id, s.revision);
        if self
            .autosave
            .last
            .as_ref()
            .is_some_and(|(d, r, _)| (*d, *r) == key)
        {
            return;
        }
        if self
            .autosave
            .last_time
            .is_some_and(|t| t.elapsed() < self.autosave.interval)
        {
            return;
        }
        let Some(path) = self.autosave_path() else {
            return;
        };
        if let Some(dir) = path.parent()
            && std::fs::create_dir_all(dir).is_err()
        {
            return;
        }
        self.autosave.last = Some((key.0, key.1, path.clone()));
        self.autosave.last_time = Some(Instant::now());
        self.autosave.dropped.retain(|d| *d != path);
        self.request(Command::SaveCopy { path });
    }

    /// The current document's recovery copy is no longer needed (saved, or
    /// deliberately discarded): remove it.
    pub(crate) fn drop_autosave(&mut self) {
        if let Some((_, _, p)) = self.autosave.last.take() {
            let _ = std::fs::remove_file(&p);
            if !self.autosave.dropped.contains(&p) {
                self.autosave.dropped.push(p);
            }
        }
        self.autosave.last_time = None;
    }

    /// A `SaveCopy` reply: a copy written after it was dropped is stale.
    pub(crate) fn autosave_written(&mut self, path: &std::path::Path) {
        if let Some(i) = self.autosave.dropped.iter().position(|d| d == path) {
            let _ = std::fs::remove_file(path);
            self.autosave.dropped.remove(i);
        }
    }

    /// At exit: let queued work (a pending `SaveCopy`) finish, up to
    /// `timeout`, then remove every dropped copy, so a deliberate Quit never
    /// leaves a recovery copy behind.
    pub fn shutdown(&mut self, timeout: Duration) {
        let start = Instant::now();
        while self.busy() && start.elapsed() < timeout {
            match self.engine.poll() {
                Some(r) => self.handle_reply(r),
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        while let Some(r) = self.engine.poll() {
            self.handle_reply(r);
        }
        for p in std::mem::take(&mut self.autosave.dropped) {
            let _ = std::fs::remove_file(p);
        }
    }

    /// "Recover unsaved work" window (copies found at start-up).
    pub(crate) fn recovery_ui(&mut self, ctx: &egui::Context) {
        if self.autosave.offered.is_empty() {
            return;
        }
        let mut open = None;
        let mut discard = None;
        egui::Window::new("Recover unsaved work")
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("These documents had unsaved changes when R-CAD last stopped:");
                for p in self.autosave.offered.clone() {
                    ui.horizontal(|ui| {
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().trim_end_matches(SUFFIX).to_string())
                            .unwrap_or_default();
                        let when = std::fs::metadata(&p)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.elapsed().ok())
                            .map(|d| format!("{} min ago", d.as_secs() / 60))
                            .unwrap_or_default();
                        ui.label(RichText::new(name).strong());
                        ui.weak(when);
                        if ui.button("Open copy").probe("Open recovery copy").clicked() {
                            open = Some(p.clone());
                        }
                        if ui.button("Discard").probe("Discard recovery copy").clicked() {
                            discard = Some(p.clone());
                        }
                    });
                }
                ui.small("Open copy opens it as an unsaved document (Save asks for a path); the copy file stays until you save or discard.");
            });
        if let Some(p) = discard {
            let _ = std::fs::remove_file(&p);
            self.autosave.offered.retain(|x| *x != p);
        }
        if let Some(p) = open {
            self.autosave.offered.retain(|x| *x != p);
            self.guarded(crate::app::Guarded::OpenCopy(p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ExecMode;
    use rcad_commands::SketchTemplate;
    use rcad_document::SketchPlane;

    fn pump(app: &mut RcadApp) {
        while let Some(r) = app.engine.poll() {
            app.handle_reply(r);
        }
    }

    /// Review run 4 #19: a recovery copy written after it was dropped (Save /
    /// Discard / Quit while a `SaveCopy` was still queued) is removed when its
    /// reply lands, and `shutdown` removes any dropped copy at exit.
    #[test]
    fn a_copy_written_after_it_was_dropped_is_removed() {
        let dir = std::env::temp_dir().join(format!("rcad-dropped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = RcadApp::new(ExecMode::Sync, None);
        app.autosave.dir = Some(dir.clone());
        app.autosave.interval = Duration::ZERO;
        app.enable_autosave();
        app.request(Command::NewDocument);
        app.request(Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 10.0,
                height: 5.0,
                center: None,
            },
        });
        pump(&mut app);
        assert!(app.summary.dirty);
        app.autosave_tick();
        let path = app
            .autosave
            .last
            .as_ref()
            .expect("copy requested")
            .2
            .clone();
        // Dropped before its reply was handled; the queued write then lands.
        app.drop_autosave();
        std::fs::write(&path, b"late").unwrap();
        pump(&mut app);
        assert!(!path.exists(), "stale copy removed when its write landed");
        assert!(app.autosave.dropped.is_empty());
        // At exit: a dropped copy whose write never reported is removed too.
        std::fs::write(&path, b"late").unwrap();
        app.autosave.dropped.push(path.clone());
        app.shutdown(Duration::from_millis(100));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
