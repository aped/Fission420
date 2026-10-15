//! Project and Intersect inside the sketch editor (docs/behavior/sketching.md
//! "Project inside the sketch").
//!
//! Projection reads the model, which lives in the engine, so it runs as the
//! document command `Command::Project` against the sketch feature:
//!
//! 1. the editor's work so far is committed (`CreateSketch` for a new sketch,
//!    `EditSketch` when modified),
//! 2. `Project` adds the linked curves (then `UnlinkProjection` when the
//!    Projection Link option is off),
//! 3. the editor continues on the committed sketch with the projected curves
//!    marked read-only (`SketchEditor::rebase`).
//!
//! The session counts the document steps it made; Cancel Sketch undoes them,
//! so a cancelled session still leaves the document as it was. While a step
//! is in flight the session is `committing` (input and Finish wait).
//!
//! A new sketch on a planar face starts with that face's edges projected as
//! construction geometry (Fusion's default), unless "Auto-project face edges"
//! is off.
use crate::app::{RcadApp, SketchTarget};
use eframe::egui;
use rcad_commands::{Command, Response, SketchTemplate};
use rcad_document::{FeatureId, ProjectSource, SketchPlane};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Commit,
    Project,
    Unlink,
}

#[derive(Clone, Debug)]
struct Pending {
    source: ProjectSource,
    at: Option<[f64; 3]>,
    construction: bool,
    linked: bool,
    stage: Stage,
    /// The automatic projection of the host face (status text differs).
    auto: bool,
}

/// Per-session projection state.
#[derive(Clone, Debug, Default)]
pub struct ProjectState {
    pending: Option<Pending>,
    /// Document steps this session committed (undone by Cancel Sketch).
    pub doc_commits: usize,
}

impl ProjectState {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
}

impl RcadApp {
    /// After sketch input: resolve a Project / Intersect click against the
    /// model (face or body under the cursor) and start the projection.
    pub(crate) fn sketch_project_poll(&mut self) {
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let Some(pick) = s.editor.project_pick.take() else {
            return;
        };
        if s.project.busy() || s.committing {
            return;
        }
        let fail = |app: &mut RcadApp, text: String| {
            if let Some(s) = app.sketch.as_mut() {
                s.editor.feedback = Some(crate::sketch_edit::Feedback { error: true, text });
            }
        };
        let Some((g, at)) = self.face_hit(pick.at) else {
            let what = if pick.section {
                "body"
            } else {
                "face next to the edge"
            };
            fail(
                self,
                format!("Project: no model face under the cursor; click a {what}."),
            );
            return;
        };
        let source = if pick.section {
            let Some(b) = self.scene.body_of(g).map(|b| b.id) else {
                fail(self, "Intersect: that face belongs to no body.".into());
                return;
            };
            // `after` is derived by the command from the body's history.
            ProjectSource::Section {
                body: b,
                after: FeatureId(0),
            }
        } else {
            match self
                .faces
                .iter()
                .find(|f| f.group == g)
                .and_then(|f| f.reference)
            {
                Some(face) => ProjectSource::Edge(face),
                None => {
                    let why = self
                        .faces
                        .iter()
                        .find(|f| f.group == g)
                        .and_then(|f| f.sketch_blocker())
                        .unwrap_or("this face cannot be referenced");
                    fail(
                        self,
                        format!(
                            "Project: {why}. Click a planar face that borders the edge (its \
                             edges are projected; curved faces have no edge reference)."
                        ),
                    );
                    return;
                }
            }
        };
        let at = (!pick.section).then_some(at);
        // A projected edge is an open curve, which would make the sketch's
        // profile fail (profiles need closed loops): it comes in as
        // construction (reference) geometry. A section is closed loops and
        // stays profile geometry.
        let construction = !pick.section;
        self.start_projection(source, at, construction, pick.linked, false);
    }

    /// Commit the session's work if needed, then project `source`.
    pub(crate) fn start_projection(
        &mut self,
        source: ProjectSource,
        at: Option<[f64; 3]>,
        construction: bool,
        linked: bool,
        auto: bool,
    ) {
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let mut p = Pending {
            source,
            at,
            construction,
            linked,
            stage: Stage::Commit,
            auto,
        };
        s.editor.cancel_gesture();
        let cmd = match s.target.clone() {
            SketchTarget::New { plane } => Command::CreateSketch {
                name: None,
                plane,
                template: SketchTemplate::Custom {
                    sketch: s.editor.sketch.clone(),
                },
            },
            SketchTarget::Existing(id) if s.editor.modified() => Command::EditSketch {
                feature: id,
                sketch: s.editor.sketch.clone(),
            },
            SketchTarget::Existing(id) => {
                p.stage = Stage::Project;
                project_command(id, &p)
            }
        };
        s.project.pending = Some(p);
        s.committing = true;
        self.request(cmd);
    }

    /// Advance a pending projection on each reply (called for every reply).
    pub(crate) fn sketch_project_reply(&mut self, command: &Command, response: &Response) {
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let Some(p) = s.project.pending.clone() else {
            return;
        };
        let error = || {
            response
                .error
                .as_ref()
                .map_or_else(|| "failed".to_string(), |e| e.to_string())
        };
        match (p.stage, command) {
            (Stage::Commit, Command::CreateSketch { .. } | Command::EditSketch { .. }) => {
                if !response.ok {
                    s.project.pending = None;
                    s.committing = false;
                    s.editor.feedback = Some(crate::sketch_edit::Feedback {
                        error: true,
                        text: format!("Project: the sketch could not be committed: {}", error()),
                    });
                    return;
                }
                s.project.doc_commits += 1;
                let id = match (s.target.clone(), response.feature) {
                    (SketchTarget::Existing(id), _) => id,
                    (SketchTarget::New { .. }, Some(id)) => {
                        s.target = SketchTarget::Existing(id);
                        id
                    }
                    (SketchTarget::New { .. }, None) => {
                        s.project.pending = None;
                        s.committing = false;
                        return;
                    }
                };
                // The committed sketch is the editor's new baseline.
                let links = self.links_of(id);
                let sk = self.stored_sketch_of(id);
                if let (Some(s), Some(sk)) = (self.sketch.as_mut(), sk) {
                    s.editor.rebase(sk, &links);
                }
                if let Some(s) = self.sketch.as_mut()
                    && let Some(p) = s.project.pending.as_mut()
                {
                    p.stage = Stage::Project;
                    let cmd = project_command(id, p);
                    self.request(cmd);
                }
            }
            (Stage::Project, Command::Project { sketch, .. }) => {
                let id = *sketch;
                if !response.ok {
                    let what = if p.auto {
                        "Auto-project of the face edges"
                    } else {
                        "Project"
                    };
                    let text = format!("{what} refused: {}", error());
                    self.finish_projection(id, Some(text), true);
                    return;
                }
                s.project.doc_commits += 1;
                if !p.linked {
                    let link = self.links_of(id).len().saturating_sub(1);
                    if let Some(s) = self.sketch.as_mut()
                        && let Some(p) = s.project.pending.as_mut()
                    {
                        p.stage = Stage::Unlink;
                    }
                    self.request(Command::UnlinkProjection { sketch: id, link });
                    return;
                }
                let n = self.links_of(id).last().map_or(0, Vec::len);
                let text = if p.auto {
                    format!("Projected the face's {n} edge(s) as construction (read-only, linked).")
                } else {
                    format!("Projected {n} curve(s) (read-only, linked to the model).")
                };
                self.finish_projection(id, Some(text), false);
            }
            (Stage::Unlink, Command::UnlinkProjection { sketch, .. }) => {
                let id = *sketch;
                if response.ok {
                    s.project.doc_commits += 1;
                    self.finish_projection(id, Some("Projected (unlinked copy).".into()), false);
                } else {
                    let text = format!("Unlink refused: {}", error());
                    self.finish_projection(id, Some(text), true);
                }
            }
            _ => {}
        }
    }

    fn finish_projection(&mut self, id: FeatureId, text: Option<String>, error: bool) {
        let links = self.links_of(id);
        let sk = self.stored_sketch_of(id);
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        s.project.pending = None;
        s.committing = false;
        if let Some(sk) = sk {
            s.editor.rebase(sk, &links);
        }
        if let Some(text) = text {
            s.editor.feedback = Some(crate::sketch_edit::Feedback { error, text });
        }
    }

    fn links_of(&self, id: FeatureId) -> Vec<Vec<rcad_sketch::EntityId>> {
        self.summary
            .features
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.projection_entities.clone())
            .unwrap_or_default()
    }

    fn stored_sketch_of(&self, id: FeatureId) -> Option<rcad_sketch::Sketch> {
        self.summary
            .features
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| f.sketch.clone())
    }

    /// Unlink the projection of the selected projected curve (its geometry
    /// stays as plain fixed curves).
    pub(crate) fn unlink_selected_projection(&mut self) {
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        let link = s.editor.selection.iter().find_map(|i| match i {
            crate::sketch_edit::Item::Entity(e) => s.editor.projected.get(e).copied(),
            _ => None,
        });
        let (Some(link), SketchTarget::Existing(id)) = (link, s.target.clone()) else {
            return;
        };
        if s.editor.modified() {
            // Commit the work first so the unlink applies to it.
            let cmd = Command::EditSketch {
                feature: id,
                sketch: s.editor.sketch.clone(),
            };
            s.project.doc_commits += 1;
            self.request(cmd);
        }
        let Some(s) = self.sketch.as_mut() else {
            return;
        };
        s.project.pending = Some(Pending {
            source: ProjectSource::Sketch(id),
            at: None,
            construction: false,
            linked: false,
            stage: Stage::Unlink,
            auto: false,
        });
        s.committing = true;
        self.request(Command::UnlinkProjection { sketch: id, link });
    }

    /// Cancel Sketch: undo the document steps this session's projections made.
    pub(crate) fn undo_session_commits(&mut self) {
        let n = self.sketch.as_ref().map_or(0, |s| s.project.doc_commits);
        for _ in 0..n {
            self.request(Command::Undo);
        }
    }

    /// The auto-project preference (a new sketch on a planar face starts
    /// with the face's edges projected as construction).
    pub fn set_auto_project_faces(&mut self, on: bool) {
        self.auto_project_faces = on;
    }

    /// Start the automatic face-edge projection of a new sketch on a face.
    pub(crate) fn auto_project_host_face(&mut self) {
        if !self.auto_project_faces {
            return;
        }
        let Some(SketchTarget::New {
            plane: SketchPlane::Face(face),
        }) = self.sketch.as_ref().map(|s| s.target.clone())
        else {
            return;
        };
        self.start_projection(ProjectSource::Face(face), None, true, true, true);
    }
}

/// Project tool options, Unlink for a selected projected curve, and the
/// auto-project preference (sketch toolbar). Returns true when Unlink was
/// clicked (the caller runs `unlink_selected_projection`).
pub(crate) fn options_ui(
    ui: &mut egui::Ui,
    ed: &mut crate::sketch_edit::SketchEditor,
    auto_project: &mut bool,
) -> bool {
    use crate::probe::Probe;
    if matches!(
        ed.tool,
        crate::sketch_edit::SketchTool::Project | crate::sketch_edit::SketchTool::Intersect
    ) {
        ui.checkbox(&mut ed.project_linked, "Projection Link")
            .probe("Projection Link")
            .on_hover_text("On: the projected curves follow the model. Off: an unlinked copy.");
        ui.checkbox(auto_project, "Auto-project face edges")
            .probe("Auto-project face edges")
            .on_hover_text(
                "A new sketch on a planar face starts with the face's edges projected as \
                 construction geometry",
            );
    }
    let projected_selected = ed
        .selection
        .iter()
        .any(|i| matches!(i, crate::sketch_edit::Item::Entity(e) if ed.projected.contains_key(e)));
    projected_selected
        && ui
            .button("Unlink projection")
            .probe("Unlink projection")
            .on_hover_text("Break the link: the curves stay as plain fixed geometry")
            .clicked()
}

fn project_command(sketch: FeatureId, p: &Pending) -> Command {
    Command::Project {
        sketch,
        source: p.source,
        construction: p.construction,
        at: p.at,
    }
}
