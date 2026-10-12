//! Autosave and recovery (SESSION_4 §7.7). While a document has unsaved
//! changes, a copy is written every [`Autosave::interval`] to the recovery
//! directory with `save_copy` (atomic; the document's own path, dirty state and
//! undo history are untouched). A successful Save, or deliberately leaving the
//! document (New/Open/Close after the unsaved-changes prompt, Quit), removes
//! the copy; a crash leaves it, and the next start offers to open it as an
//! unsaved document (`open_copy`) or discard it.
//!
//! Directory: `F420_RECOVERY_DIR`, else `$XDG_STATE_HOME/fission420/recovery`,
//! else `~/.local/state/fission420/recovery`. Interval: `F420_AUTOSAVE_SECS`
//! (default 60; `0` = after every committed change, for tests).
use crate::app::F420App;
use crate::probe::Probe as _;
use eframe::egui::{self, RichText};
use f420_commands::Command;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SUFFIX: &str = ".autosave.f420.json";

#[derive(Debug)]
pub struct Autosave {
    /// Off unless the shell enables it ([`F420App::enable_autosave`]); tests
    /// and scripted scenes never write recovery files.
    pub enabled: bool,
    pub dir: Option<PathBuf>,
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
}

impl Default for Autosave {
    fn default() -> Self {
        let dir = std::env::var_os("F420_RECOVERY_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_STATE_HOME")
                    .map(|d| PathBuf::from(d).join("fission420/recovery"))
            })
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".local/state/fission420/recovery"))
            });
        let interval = std::env::var("F420_AUTOSAVE_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(Duration::from_secs(60), Duration::from_secs);
        Self {
            enabled: false,
            dir,
            interval,
            last: None,
            last_time: None,
            offered: vec![],
            replaced: None,
            opening: None,
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
        .filter(|p| p.to_string_lossy().ends_with(SUFFIX))
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

impl F420App {
    /// Turn autosave on (the desktop shell does) and offer the recovery
    /// copies left by an earlier session.
    pub fn enable_autosave(&mut self) {
        self.autosave.enabled = true;
        self.autosave.offered = self
            .autosave
            .dir
            .as_deref()
            .map(find_copies)
            .unwrap_or_default();
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
                            .trim_end_matches(".f420.json")
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
        self.request(Command::SaveCopy { path });
    }

    /// The current document's recovery copy is no longer needed (saved, or
    /// deliberately discarded): remove it.
    pub(crate) fn drop_autosave(&mut self) {
        if let Some((_, _, p)) = self.autosave.last.take() {
            let _ = std::fs::remove_file(p);
        }
        self.autosave.last_time = None;
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
                ui.label("These documents had unsaved changes when Fission420 last stopped:");
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
