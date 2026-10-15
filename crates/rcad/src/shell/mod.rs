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
pub mod keymap;
pub mod prefs;
pub mod registry;

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
}

/// Shell state that is view-only (never part of the document or undo).
#[derive(Debug, Clone, Default)]
pub struct ShellState {
    /// Preferences (keymap layer, theme, dialog width, recent files).
    pub prefs: prefs::Prefs,
    /// Where preferences are saved (None: tests and scripted runs keep
    /// them in memory only).
    pub prefs_path: Option<std::path::PathBuf>,
    /// Section to force open on its next draw.
    pub focus: Option<Focus>,
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
