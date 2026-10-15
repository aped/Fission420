//! Keyboard bindings. Two layers: `Fusion` (the default; Fusion-style keys
//! such as E extrude, H hole, F fillet, M move, J joint, I measure) and
//! `Classic` (R-CAD's earlier bindings, e.g. F fit and R centre rectangle).
//! Model-mode bindings come from the command registry; the sketch layer is a
//! separate table here. Shortcuts are ignored while a text field has focus.

use eframe::egui::{Key, KeyboardShortcut, Modifiers};
use serde::{Deserialize, Serialize};

use crate::sketch_edit::SketchTool;

/// Which key layer is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeymapLayer {
    #[default]
    Fusion,
    Classic,
}

impl KeymapLayer {
    pub fn label(self) -> &'static str {
        match self {
            KeymapLayer::Fusion => "Fusion-style",
            KeymapLayer::Classic => "R-CAD classic",
        }
    }
    /// `RCAD_KEYMAP=classic|fusion` (scripts and tests pin a layer).
    pub fn from_env() -> Option<Self> {
        match std::env::var("RCAD_KEYMAP").ok()?.trim() {
            "classic" => Some(KeymapLayer::Classic),
            "fusion" => Some(KeymapLayer::Fusion),
            _ => None,
        }
    }
}

/// A key with modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub mods: Modifiers,
    pub key: Key,
}

pub const NONE: Modifiers = Modifiers::NONE;
pub const CMD: Modifiers = Modifiers::COMMAND;
pub const SHIFT: Modifiers = Modifiers::SHIFT;
pub const CMD_SHIFT: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: true,
    mac_cmd: false,
    command: true,
};

pub const fn k(key: Key) -> Chord {
    Chord { mods: NONE, key }
}
pub const fn cmd(key: Key) -> Chord {
    Chord { mods: CMD, key }
}
pub const fn shift(key: Key) -> Chord {
    Chord { mods: SHIFT, key }
}
pub const fn cmd_shift(key: Key) -> Chord {
    Chord {
        mods: CMD_SHIFT,
        key,
    }
}

impl Chord {
    pub fn shortcut(self) -> KeyboardShortcut {
        KeyboardShortcut::new(self.mods, self.key)
    }
    /// "Ctrl+Shift+S", "E", "F6".
    pub fn text(self) -> String {
        let mut s = String::new();
        if self.mods.command || self.mods.ctrl {
            s.push_str("Ctrl+");
        }
        if self.mods.alt {
            s.push_str("Alt+");
        }
        if self.mods.shift {
            s.push_str("Shift+");
        }
        s.push_str(match self.key {
            Key::Delete => "Del",
            Key::Escape => "Esc",
            Key::Enter => "Enter",
            k => k.symbol_or_name(),
        });
        s
    }
    /// Number of modifiers (more specific chords are matched first: egui's
    /// logical matching would let Ctrl+Z consume Ctrl+Shift+Z).
    pub fn specificity(self) -> u8 {
        u8::from(self.mods.shift) + u8::from(self.mods.alt) + u8::from(self.mods.command)
    }
}

/// What a sketch-mode key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SketchKey {
    Tool(SketchTool),
    Construction,
    Undo,
    Redo,
    Delete,
    FinishChain,
    Escape,
    LookAt,
}

/// Sketch-mode bindings of `layer`, most specific first.
pub fn sketch_bindings(layer: KeymapLayer) -> Vec<(Chord, SketchKey)> {
    use SketchKey as S;
    use SketchTool as T;
    let mut v = vec![
        (cmd_shift(Key::Z), S::Redo),
        (cmd(Key::Y), S::Redo),
        (cmd(Key::Z), S::Undo),
    ];
    match layer {
        KeymapLayer::Fusion => v.extend([
            // Fusion: R is the 2-point rectangle; Shift+R (R-CAD) the centre one.
            (shift(Key::R), S::Tool(T::CenterRectangle)),
            (k(Key::R), S::Tool(T::CornerRectangle)),
        ]),
        KeymapLayer::Classic => v.extend([
            (shift(Key::R), S::Tool(T::CornerRectangle)),
            (k(Key::R), S::Tool(T::CenterRectangle)),
            (k(Key::S), S::Tool(T::Select)),
            (k(Key::E), S::Tool(T::Extend)),
        ]),
    }
    v.extend([
        (k(Key::L), S::Tool(T::Line)),
        (k(Key::C), S::Tool(T::Circle)),
        (k(Key::A), S::Tool(T::Arc)),
        (k(Key::X), S::Construction),
        (k(Key::D), S::Tool(T::Dimension)),
        (k(Key::T), S::Tool(T::Trim)),
        (k(Key::O), S::Tool(T::Offset)),
        (k(Key::Delete), S::Delete),
        (k(Key::Backspace), S::Delete),
        (k(Key::Enter), S::FinishChain),
        (k(Key::Escape), S::Escape),
        (k(Key::F), S::LookAt),
    ]);
    v
}

/// The sketch key bound to `tool` in `layer` (tooltips).
pub fn sketch_key_for(layer: KeymapLayer, tool: SketchTool) -> Option<String> {
    sketch_bindings(layer)
        .into_iter()
        .find(|(_, a)| *a == SketchKey::Tool(tool))
        .map(|(c, _)| c.text())
}

/// One line of sketch keys for the help sheet ("L line · R 2-point rectangle · …").
pub fn sketch_help(layer: KeymapLayer) -> String {
    let mut out: Vec<String> = vec![];
    for (c, a) in sketch_bindings(layer) {
        let what = match a {
            SketchKey::Tool(t) => t.label().to_lowercase(),
            SketchKey::Construction => "construction toggle".into(),
            SketchKey::Undo => "sketch step undo".into(),
            SketchKey::Redo => "sketch step redo".into(),
            SketchKey::Delete => "delete".into(),
            SketchKey::FinishChain => "end line chain".into(),
            SketchKey::Escape => "cancel gesture, then tool".into(),
            SketchKey::LookAt => "look at the sketch".into(),
        };
        if c.key == Key::Backspace {
            continue;
        }
        out.push(format!("{} {what}", c.text()));
    }
    out.join(" · ")
}
