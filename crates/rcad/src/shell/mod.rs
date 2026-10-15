//! Application shell: the command registry that drives the command bars,
//! the keymap and the generated shortcut sheet, plus the per-window shell
//! state (keymap layer, focus requests for browser/inspector sections).
//!
//! Every UI action goes through a [`registry::CommandDef`]: its label is the
//! widget's accessible name and probe key (scripts and kittests address the
//! command by that label), its enabled predicate returns the reason it cannot
//! run, and its run action is the same code path whether it is reached from a
//! button, a menu or a key.

pub mod actions;
pub mod icons;
pub mod keymap;
pub mod prefs;
pub mod registry;
pub mod ribbon;
pub mod timeline;

pub use keymap::KeymapLayer;
pub use registry::CmdId;

/// A browser or inspector section a command asks to open (Fusion places
/// these commands in the ribbon; their forms still live in the panels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Parameters,
    Configurations,
    ConfigTable,
    Drawings,
    Components,
    MoveCopy,
    Split,
    Combine,
    Material,
    MeshRepair,
    MeshEdit,
    SheetFlange,
    SheetBase,
    SurfaceFromSketch,
}

/// Shell state that is view-only (never part of the document or undo).
#[derive(Debug, Clone)]
pub struct ShellState {
    /// Preferences (keymap layer, theme, dialog width, recent files).
    pub prefs: prefs::Prefs,
    /// Where preferences are saved (None: tests and scripted runs keep
    /// them in memory only).
    pub prefs_path: Option<std::path::PathBuf>,
    /// Section to force open on its next draw.
    pub focus: Option<Focus>,
    /// Active ribbon tab.
    pub tab: registry::Tab,
    pub show_prefs: bool,
    /// Model revision shown when the current face was picked (commands
    /// built from the pick run only at that revision).
    pub pick_revision: Option<u64>,
    /// Face pick seen by the open dialog (new picks feed its input).
    pub last_pick: Option<u32>,
    /// The dialog's selection input was cleared and waits for a pick.
    pub pick_armed: bool,
    /// Feature the open dialog edits (Edit Feature), if any.
    pub editing: Option<rcad_document::FeatureId>,
    /// Theme applied to the context (re-applied when it changes).
    pub applied_theme: Option<prefs::Theme>,
}

impl ShellState {
    /// `Some(true)` once when `f` was requested (for
    /// `CollapsingHeader::open`), then `None`.
    pub fn take_focus(&mut self, f: Focus) -> Option<bool> {
        if self.focus == Some(f) {
            self.focus = None;
            Some(true)
        } else {
            None
        }
    }
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            prefs: Default::default(),
            prefs_path: None,
            focus: None,
            tab: registry::Tab::Solid,
            show_prefs: false,
            editing: None,
            last_pick: None,
            pick_revision: None,
            pick_armed: false,
            applied_theme: None,
        }
    }
}
