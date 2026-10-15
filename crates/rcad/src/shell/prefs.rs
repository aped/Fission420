//! User preferences: a small versioned JSON file in the XDG config directory
//! (`$XDG_CONFIG_HOME/r-cad/prefs.json`, else `~/.config/r-cad/prefs.json`),
//! written atomically (temporary file + rename). Preferences are view state
//! only; a missing, unreadable or newer-version file falls back to defaults
//! and never blocks startup.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::KeymapLayer;

pub const PREFS_VERSION: u32 = 1;
const MAX_RECENT: usize = 8;

/// Colour theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the operating system's light/dark setting.
    #[default]
    FollowOs,
    Light,
    Dark,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::FollowOs => "Follow system",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    pub version: u32,
    #[serde(default)]
    pub keymap: KeymapLayer,
    #[serde(default)]
    pub theme: Theme,
    /// Width of the docked command-dialog area (points).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog_width: Option<f32>,
    /// Most recent first.
    #[serde(default)]
    pub recent: Vec<PathBuf>,
    /// Sketch: a new sketch on a planar face starts with the face's edges
    /// projected (as construction). Fusion's default: on.
    #[serde(default = "yes")]
    pub auto_project_face_edges: bool,
    /// Sketch: a dimension click on a model edge (no sketch geometry under
    /// the cursor) projects that edge first, then dimensions it. Fusion's
    /// "auto project edges on reference"; off by default here.
    #[serde(default)]
    pub auto_project_on_reference: bool,
}

fn yes() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            version: PREFS_VERSION,
            keymap: KeymapLayer::Fusion,
            theme: Theme::FollowOs,
            dialog_width: None,
            recent: vec![],
            auto_project_face_edges: true,
            auto_project_on_reference: false,
        }
    }
}

impl Prefs {
    /// The per-user preferences file (None without a config/home directory).
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("r-cad").join("prefs.json"))
    }

    /// Read `path`; defaults when absent, unreadable, invalid or written by
    /// a newer version (the reason is returned for the message log).
    pub fn load(path: &Path) -> (Prefs, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Prefs::default(), None),
            Err(e) => return (Prefs::default(), Some(format!("preferences not read: {e}"))),
        };
        match serde_json::from_str::<Prefs>(&text) {
            Ok(p) if p.version > PREFS_VERSION => (
                Prefs::default(),
                Some(format!(
                    "preferences version {} is newer than this build ({PREFS_VERSION}); using defaults",
                    p.version
                )),
            ),
            Ok(mut p) => {
                p.version = PREFS_VERSION;
                p.recent.truncate(MAX_RECENT);
                if p.dialog_width
                    .is_some_and(|w| !w.is_finite() || !(200.0..=900.0).contains(&w))
                {
                    p.dialog_width = None;
                }
                (p, None)
            }
            Err(e) => (
                Prefs::default(),
                Some(format!("preferences file is invalid ({e}); using defaults")),
            ),
        }
    }

    /// Write atomically: a sibling temporary file, then rename over `path`.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Remember `p` as the most recent document.
    pub fn push_recent(&mut self, p: PathBuf) {
        self.recent.retain(|x| *x != p);
        self.recent.insert(0, p);
        self.recent.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_missing_invalid_and_newer() {
        let dir = std::env::temp_dir().join(format!("rcad-prefs-{}", std::process::id()));
        let path = dir.join("r-cad").join("prefs.json");
        assert_eq!(Prefs::load(&path), (Prefs::default(), None));
        let mut p = Prefs {
            keymap: KeymapLayer::Classic,
            theme: Theme::Dark,
            dialog_width: Some(340.0),
            ..Prefs::default()
        };
        for i in 0..12 {
            p.push_recent(PathBuf::from(format!("/d/{i}.rcad.json")));
        }
        p.push_recent(PathBuf::from("/d/3.rcad.json"));
        assert_eq!(p.recent.len(), MAX_RECENT);
        assert_eq!(p.recent[0], PathBuf::from("/d/3.rcad.json"));
        p.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), (p.clone(), None));
        std::fs::write(&path, "{ not json").unwrap();
        let (d, why) = Prefs::load(&path);
        assert_eq!(d, Prefs::default());
        assert!(why.unwrap().contains("invalid"));
        std::fs::write(&path, r#"{"version": 99, "keymap": "classic"}"#).unwrap();
        let (d, why) = Prefs::load(&path);
        assert_eq!(d.keymap, KeymapLayer::Fusion);
        assert!(why.unwrap().contains("newer"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
