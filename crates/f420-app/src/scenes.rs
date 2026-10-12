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
    "params",
    "measure",
    "param_error",
    "print_export",
    "hole",
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

/// The committed parametric enclosure example.
fn enclosure() -> SceneStep {
    SceneStep::Cmd(Command::Open {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/enclosure.f420.json"),
    })
}

/// Display group of the planar face of `body` with outward normal `n` through
/// the plane `n . x = d`.
fn face_group(a: &F420App, body: u32, n: [f64; 3], d: f64) -> Option<u32> {
    a.faces.iter().find_map(|f| {
        let p = f.plane?;
        let dn = (0..3).map(|i| (p.normal[i] - n[i]).abs()).sum::<f64>();
        let off = (0..3).map(|i| p.normal[i] * p.point[i]).sum::<f64>();
        (f.body == Some(BodyId(body)) && dn < 1e-9 && (off - d).abs() < 1e-9).then_some(f.group)
    })
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
            region: None,
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
                    region: None,
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
            // Run 4: parameters panel and expression-bound dimensions.
            "params" => vec![
                enclosure(),
                Ui(|a| {
                    let s = a.feature_by_name("Lip outline").map(|f| f.id);
                    a.select(s);
                    a.set_view(f420_viewport::ViewPreset::Iso);
                }),
            ],
            // Run 4: exact two-face measurement (lip clearance across bodies).
            "measure" => vec![
                enclosure(),
                Ui(|a| {
                    a.set_view(f420_viewport::ViewPreset::Iso);
                    if let Some(g) = face_group(a, 1, [1.0, 0.0, 0.0], 2.0) {
                        a.select_face(Some(g));
                        a.measure_from = a.selected_face().cloned();
                    }
                    if let Some(g) = face_group(a, 2, [-1.0, 0.0, 0.0], -2.25) {
                        a.select_face(Some(g));
                    }
                }),
            ],
            // Run 4: an illegal parameter edit, refused with the model unchanged.
            "param_error" => vec![
                enclosure(),
                Cmd(Command::SetUserParameter {
                    name: "wall".into(),
                    expression: "45".into(),
                    unit: None,
                    comment: None,
                }),
                Ui(|a| a.set_view(f420_viewport::ViewPreset::Iso)),
            ],
            // Run 4: print export dialog (lid face down, layout, P1S bed).
            "print_export" => vec![
                enclosure(),
                Ui(|a| {
                    a.set_view(f420_viewport::ViewPreset::Iso);
                    // Lid plate top face (z = 2, +Z): goes on the bed, lip up.
                    if let Some(g) = face_group(a, 2, [0.0, 0.0, 1.0], 2.0) {
                        a.select_face(Some(g));
                    }
                    let mut form = crate::exchange::PrintForm::new(
                        false,
                        "enclosure.3mf".into(),
                        ExportScope::AllVisible,
                    );
                    form.place = true;
                    form.layout = true;
                    form.bed = crate::exchange::BedChoice::P1s;
                    if let Some(r) = a.selected_face().and_then(|f| f.reference) {
                        form.set_orientation(BodyId(2), f420_commands::print::Orient::FaceDown(r));
                    }
                    a.dialog = Some(Dialog::Export3mf(form));
                }),
            ],
            // Run 4: Hole dialog with a counterbore on the bracket template.
            "hole" => vec![
                Cmd(Command::NewDocument),
                Ui(|a| {
                    for c in crate::app::PrintTemplate::Bracket
                        .commands()
                        .into_iter()
                        .skip(1)
                    {
                        a.request(c);
                    }
                }),
                Ui(|a| {
                    a.set_view(f420_viewport::ViewPreset::Iso);
                    if let Some(g) = face_group(a, 1, [0.0, 0.0, 1.0], 5.0) {
                        a.select_face(Some(g));
                        let info = a.selected_face().cloned();
                        if let Some((info, r)) = info.and_then(|i| i.reference.map(|r| (i, r))) {
                            let [cx, cy] = a.face_centre_local(&info, &r).unwrap_or([0.0, 0.0]);
                            a.dialog = Some(Dialog::Hole {
                                face: r,
                                cx: crate::params_ui::fmt_value(cx),
                                cy: crate::params_ui::fmt_value(cy - 8.0),
                                diameter: "5.5".into(),
                                through: true,
                                depth: "8".into(),
                                counterbore: true,
                                cb_diameter: "10".into(),
                                cb_depth: "3".into(),
                            });
                        }
                    }
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
