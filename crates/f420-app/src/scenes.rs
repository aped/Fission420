//! Named, scripted UI states for documentation screenshots (`--scene NAME`
//! with `--screenshot`). Each scene is a queue of document commands and UI
//! actions run through the same paths as user input (requests to the engine,
//! dialogs, selection), one step per idle frame. Fixture paths point at the
//! repository's exchange fixtures (compile-time manifest dir).

use std::collections::VecDeque;
use std::path::PathBuf;

use f420_commands::{Command, ExtrudeOp, SketchTemplate, end_cap_of};
use f420_document::{BodyId, FeatureId, SketchPlane};

use crate::app::{Dialog, F420App};
use crate::exchange::ExportScope;
use crate::extrude::ExtrudeForm;

pub enum SceneStep {
    Cmd(Command),
    Ui(fn(&mut F420App)),
}

pub const SCENES: &[&str] = &[
    "multibody",
    "extrude",
    "step",
    "sideface",
    "import",
    "export",
];

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel)
}

fn rect(w: f64, h: f64, c: [f64; 2], plane: SketchPlane) -> Command {
    Command::CreateSketch {
        name: None,
        plane,
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: h,
            center: Some(c),
        },
    }
}

/// Import through the dialog path (summary + fit on reply).
fn import(app: &mut F420App, path: PathBuf, unit: usize, partial: bool) {
    app.open_import_dialog();
    let cmd = match &mut app.dialog {
        Some(Dialog::Import(f)) => {
            f.path = path.display().to_string();
            f.unit = unit;
            f.allow_partial = partial;
            f.command().ok()
        }
        _ => None,
    };
    if let Some(cmd) = cmd {
        let token = app.request_token(cmd);
        if let Some(Dialog::Import(f)) = &mut app.dialog {
            f.pending = token;
        }
    }
}

fn two_bodies() -> Vec<SceneStep> {
    use SceneStep::*;
    vec![
        Cmd(Command::NewDocument),
        Cmd(rect(40.0, 30.0, [20.0, 15.0], SketchPlane::WorldXY)),
        Cmd(Command::extrude(None, FeatureId(1), 10.0)),
        Cmd(rect(20.0, 20.0, [60.0, 10.0], SketchPlane::WorldXY)),
        Cmd(Command::extrude(None, FeatureId(3), 10.0)),
        Cmd(rect(20.0, 10.0, [20.0, 15.0], end_cap_of(FeatureId(2)))),
        Cmd(Command::Extrude {
            name: None,
            sketch: FeatureId(5),
            distance: 5.0,
            op: ExtrudeOp::Join,
            target: Some(BodyId(1)),
            through_all: false,
            reverse: false,
            body_name: None,
        }),
    ]
}

impl F420App {
    /// Queue scene `name`. Returns false for an unknown name.
    pub fn start_scene(&mut self, name: &str) -> bool {
        use SceneStep::*;
        let steps: Vec<SceneStep> = match name {
            "multibody" => {
                let mut v = two_bodies();
                v.push(Ui(|a| a.select_body(Some(BodyId(2)))));
                v
            }
            "extrude" => {
                let mut v = two_bodies();
                v.push(Cmd(rect(10.0, 5.0, [6.0, 25.0], end_cap_of(FeatureId(2)))));
                v.push(Ui(|a| {
                    let s = a.summary.features.last().map(|f| f.id);
                    a.select(s);
                    let mut form = ExtrudeForm::new(a, s);
                    form.op = ExtrudeOp::Cut;
                    form.through_all = true;
                    a.dialog = Some(Dialog::Extrude(form));
                }));
                v
            }
            "step" => vec![
                Cmd(Command::NewDocument),
                Ui(|a| {
                    import(
                        a,
                        fixture("f420-kernel/tests/fixtures/step/two_boxes_compound_mm.step"),
                        0,
                        false,
                    )
                }),
                Ui(|a| {
                    import(
                        a,
                        fixture("f420-kernel/tests/fixtures/step/plate_hole_mm.step"),
                        0,
                        false,
                    )
                }),
            ],
            "sideface" => vec![
                Cmd(Command::NewDocument),
                Cmd(rect(60.0, 40.0, [30.0, 20.0], SketchPlane::WorldXY)),
                Cmd(Command::extrude(None, FeatureId(1), 20.0)),
                Ui(|a| {
                    // The x = 60 side face and a 10x6 slot centred at (60, 20, 10).
                    let Some(fs) = a
                        .summary
                        .features
                        .iter()
                        .flat_map(|f| f.faces.iter())
                        .find(|f| f.frame.z[0] > 0.99)
                        .cloned()
                    else {
                        return;
                    };
                    let fr = fs.frame;
                    let v = [
                        60.0 - fr.origin[0],
                        20.0 - fr.origin[1],
                        10.0 - fr.origin[2],
                    ];
                    let d = |a: [f64; 3]| v[0] * a[0] + v[1] * a[1] + v[2] * a[2];
                    let (w, h) = if fr.x[1].abs() > 0.5 {
                        (10.0, 6.0)
                    } else {
                        (6.0, 10.0)
                    };
                    a.request(rect(
                        w,
                        h,
                        [d(fr.x), d(fr.y)],
                        SketchPlane::Face(fs.reference),
                    ));
                }),
                Cmd(Command::Extrude {
                    name: Some("Side slot".into()),
                    sketch: FeatureId(3),
                    distance: 15.0,
                    op: ExtrudeOp::Cut,
                    target: Some(BodyId(1)),
                    through_all: false,
                    reverse: false,
                    body_name: None,
                }),
                Ui(|a| {
                    let s = a.feature_by_name("Side slot").map(|f| f.id);
                    a.select(s);
                    a.set_view(f420_viewport::ViewPreset::Iso);
                }),
            ],
            "import" => vec![
                Cmd(Command::NewDocument),
                Ui(|a| {
                    import(
                        a,
                        fixture("f420-exchange/tests/fixtures/stl/box_open_freecad.stl"),
                        0,
                        false,
                    )
                }),
                Ui(|a| {
                    import(
                        a,
                        fixture("f420-exchange/tests/fixtures/dxf/unsupported_entities.dxf"),
                        0,
                        true,
                    )
                }),
                Ui(|a| {
                    let b = a.summary.bodies.first().map(|b| b.id);
                    a.select_body(b);
                }),
            ],
            "export" => vec![
                Cmd(Command::NewDocument),
                Ui(|a| {
                    import(
                        a,
                        fixture("f420-kernel/tests/fixtures/step/two_boxes_compound_mm.step"),
                        0,
                        false,
                    )
                }),
                Ui(|a| {
                    import(
                        a,
                        fixture("f420-exchange/tests/fixtures/stl/box_freecad_binary.stl"),
                        0,
                        false,
                    )
                }),
                Ui(|a| {
                    a.import_summary = None;
                    let b = a.summary.bodies.last().map(|b| b.id);
                    a.select_body(b);
                    a.dialog = Some(Dialog::ExportStep {
                        path: "parts.step".into(),
                        scope: ExportScope::Selected(b.into_iter().collect()),
                    });
                }),
            ],
            _ => return false,
        };
        self.scene_steps = VecDeque::from(steps);
        self.scene_fitted = false;
        true
    }

    /// Run the next scene step once the engine is idle.
    pub(crate) fn advance_scene(&mut self) {
        if self.engine.busy() || self.summary.measuring {
            return;
        }
        match self.scene_steps.pop_front() {
            Some(SceneStep::Cmd(c)) => self.request(c),
            Some(SceneStep::Ui(f)) => f(self),
            None if !self.scene_fitted => {
                self.scene_fitted = true;
                self.fit_view();
                self.scene_hold =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(800));
            }
            None => {}
        }
    }
}
