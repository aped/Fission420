//! Named, scripted UI states for documentation screenshots (`--scene NAME`
//! with `--screenshot`). Each scene is a queue of document commands and UI
//! actions run through the same paths as user input (requests to the engine,
//! dialogs, selection), one step per idle frame. Fixture paths point at the
//! repository's exchange fixtures (compile-time manifest dir).

use std::collections::VecDeque;
use std::path::PathBuf;

use rcad_commands::{Command, ExtrudeOp, SketchTemplate, end_cap_of};
use rcad_document::{BodyId, FeatureId, SketchPlane};

use crate::app::{Dialog, RcadApp};
use crate::exchange::ExportScope;
use crate::extrude::ExtrudeForm;

#[allow(clippy::large_enum_variant)] // a static list of scene steps
pub enum SceneStep {
    Cmd(Command),
    Ui(fn(&mut RcadApp)),
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
    "tray_params",
    "tray_section",
    "tray_modifier_face",
    "tray_finished",
    "tray_export",
    "tray_refusal",
    "sketch_text",
    "sketch_text_edit",
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
fn import(app: &mut RcadApp, path: PathBuf, unit: usize, partial: bool) {
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
/// Run 5: the organizer tray example (committed document).
fn tray(finished: bool) -> SceneStep {
    SceneStep::Cmd(Command::Open {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(if finished {
            "../../examples/organizer_tray_finished.rcad.json"
        } else {
            "../../examples/organizer_tray.rcad.json"
        }),
    })
}

fn enclosure() -> SceneStep {
    SceneStep::Cmd(Command::Open {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/enclosure.rcad.json"),
    })
}

/// Display group of the planar face of `body` with outward normal `n` through
/// the plane `n . x = d`.
fn face_group(a: &RcadApp, body: u32, n: [f64; 3], d: f64) -> Option<u32> {
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
            options: Default::default(),
        }),
    ]
}

/// Run 6: plate 60 x 30 x 3 with a "B8" sketch text (12 mm, centred) on its
/// top face (sketch F3, not yet extruded).
fn text_plate() -> Vec<SceneStep> {
    use SceneStep::*;
    let mut empty = rcad_sketch::Sketch::default();
    empty.ensure_origin();
    let mut t = rcad_sketch::TextSpec::new("B8", "Liberation Sans", 12.0, [0.0, 0.0]);
    t.halign = rcad_sketch::HAlign::Center;
    t.valign = rcad_sketch::VAlign::Middle;
    vec![
        Cmd(Command::NewDocument),
        Cmd(rect(60.0, 30.0, [0.0, 0.0], SketchPlane::WorldXY)),
        Cmd(Command::extrude(None, FeatureId(1), 3.0)),
        Cmd(Command::CreateSketch {
            name: Some("Label".into()),
            plane: end_cap_of(FeatureId(2)),
            template: SketchTemplate::Custom { sketch: empty },
        }),
        Cmd(Command::AddSketchText {
            feature: FeatureId(3),
            text: t,
        }),
    ]
}

impl RcadApp {
    /// Queue scene `name`. Returns false for an unknown name.
    pub fn start_scene(&mut self, name: &str) -> bool {
        use SceneStep::*;
        let steps: Vec<SceneStep> = match name {
            // Committed examples (assembly, drawings/configurations,
            // sheet metal, surface repair) in the iso view.
            "hinge_assembly" | "drawing_set" | "sheet_u_channel" | "open_box" => {
                let file = match name {
                    "open_box" => "open_box_repair",
                    _ => name,
                };
                vec![
                    Cmd(Command::Open {
                        path: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join(format!("../../examples/{file}.rcad.json")),
                    }),
                    Ui(|a| a.set_view(rcad_viewport::ViewPreset::Iso)),
                ]
            }
            // Run 7: mesh repair inspector on the broken-cube STL fixture.
            "mesh_repair" => vec![
                Cmd(Command::NewDocument),
                Cmd(Command::ImportStl {
                    path: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../tools/fixtures/stl/broken_cube.stl"),
                    units_to_mm: Some(1.0),
                }),
                Ui(|a| {
                    a.set_view(rcad_viewport::ViewPreset::Iso);
                    a.select_body(Some(BodyId(1)));
                }),
            ],
            // Run 6: the rounded labelled enclosure (committed example).
            "rounded_enclosure" => vec![
                Cmd(Command::Open {
                    path: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../examples/rounded_enclosure.rcad.json"),
                }),
                Ui(|a| a.set_view(rcad_viewport::ViewPreset::Iso)),
            ],
            // Run 6: sheet-metal U-channel (flange inspector selected).
            "sheet_metal" => {
                let mut sk = rcad_sketch::Sketch::default();
                let p: Vec<_> = [[0.0, 22.0], [0.0, 0.0], [64.0, 0.0], [64.0, 22.0]]
                    .iter()
                    .map(|q| sk.add_point(q[0], q[1]))
                    .collect();
                for i in 0..3 {
                    sk.add_line(p[i], p[i + 1]);
                }
                vec![
                    Cmd(Command::NewDocument),
                    Cmd(Command::CreateSketch {
                        name: None,
                        plane: rcad_document::SketchPlane::WorldXY,
                        template: SketchTemplate::Custom { sketch: sk },
                    }),
                    Cmd(Command::SheetMetal {
                        name: None,
                        sketch: FeatureId(1),
                        thickness: 1.0,
                        radius: 2.0,
                        k_factor: 0.4,
                        width: 40.0,
                        flip: false,
                        body_name: Some("U-channel".into()),
                    }),
                    Ui(|a| {
                        a.set_view(rcad_viewport::ViewPreset::Iso);
                        a.select(Some(FeatureId(2)));
                    }),
                ]
            }
            // Run 6: modeled (faceted) thread pair, rod and nut side by side.
            "thread" => {
                let t = |center: [f64; 2], nut: Option<f64>| Command::Thread {
                    name: None,
                    plane: rcad_document::SketchPlane::WorldXY,
                    center,
                    diameter: 20.0,
                    pitch: 2.0,
                    length: 12.0,
                    left_hand: false,
                    chord_tol: 0.01,
                    nut_width: nut,
                    clearance: 0.1,
                    body_name: None,
                };
                vec![
                    Cmd(Command::NewDocument),
                    Cmd(t([0.0, 0.0], None)),
                    Cmd(t([40.0, 0.0], Some(30.0))),
                    Ui(|a| a.set_view(rcad_viewport::ViewPreset::Iso)),
                ]
            }
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
                        fixture("rcad-kernel/tests/fixtures/step/two_boxes_compound_mm.step"),
                        0,
                        false,
                    )
                }),
                Ui(|a| {
                    import(
                        a,
                        fixture("rcad-kernel/tests/fixtures/step/plate_hole_mm.step"),
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
                    options: Default::default(),
                }),
                Ui(|a| {
                    let s = a.feature_by_name("Side slot").map(|f| f.id);
                    a.select(s);
                    a.set_view(rcad_viewport::ViewPreset::Iso);
                }),
            ],
            "import" => vec![
                Cmd(Command::NewDocument),
                Ui(|a| {
                    import(
                        a,
                        fixture("rcad-exchange/tests/fixtures/stl/box_open_freecad.stl"),
                        0,
                        false,
                    )
                }),
                Ui(|a| {
                    import(
                        a,
                        fixture("rcad-exchange/tests/fixtures/dxf/unsupported_entities.dxf"),
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
                        fixture("rcad-kernel/tests/fixtures/step/two_boxes_compound_mm.step"),
                        0,
                        false,
                    )
                }),
                Ui(|a| {
                    import(
                        a,
                        fixture("rcad-exchange/tests/fixtures/stl/box_freecad_binary.stl"),
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
                    a.set_view(rcad_viewport::ViewPreset::Iso);
                }),
            ],
            // Run 4: exact two-face measurement (lip clearance across bodies).
            "measure" => vec![
                enclosure(),
                Ui(|a| {
                    a.set_view(rcad_viewport::ViewPreset::Iso);
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
                Ui(|a| a.set_view(rcad_viewport::ViewPreset::Iso)),
            ],
            // Run 4: print export dialog (lid face down, layout, P1S bed).
            "print_export" => vec![
                enclosure(),
                Ui(|a| {
                    a.set_view(rcad_viewport::ViewPreset::Iso);
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
                        form.set_orientation(BodyId(2), rcad_commands::print::Orient::FaceDown(r));
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
                    a.set_view(rcad_viewport::ViewPreset::Iso);
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
                                offset: "0".into(),
                            });
                        }
                    }
                }),
            ],
            // Run 5: tray parameters and the pattern's bound count/spacing.
            "tray_params" => vec![
                tray(false),
                Ui(|a| {
                    let s = a.feature_by_name("Compartments").map(|f| f.id);
                    a.select(s);
                    a.set_view(rcad_viewport::ViewPreset::Iso);
                }),
            ],
            // Run 5: display-only section through the compartments.
            "tray_section" => vec![
                tray(false),
                Ui(|a| {
                    a.set_view(rcad_viewport::ViewPreset::Iso);
                    a.section = Some(crate::section::SectionState {
                        axis: crate::section::SectionAxis::Y,
                        offset: "40".into(),
                        flip: false,
                    });
                }),
            ],
            // Run 5: the floor of the second compartment (a pattern occurrence)
            // selected: it can host a sketch (semantic Copy reference).
            "tray_modifier_face" => vec![
                tray(false),
                Ui(|a| {
                    a.set_view(rcad_viewport::ViewPreset::Iso);
                    let g = a.faces.iter().find_map(|f| match f.reference {
                        Some(rcad_document::FaceRef {
                            role:
                                rcad_document::FaceRole::Copy {
                                    instance: 1,
                                    of: rcad_document::CopiedRole::EndCap,
                                    ..
                                },
                            ..
                        }) => Some(f.group),
                        _ => None,
                    });
                    a.select_face(g);
                }),
            ],
            // Run 5: finished variant (finger notch, corner chamfers).
            "tray_finished" => vec![
                tray(true),
                Ui(|a| a.set_view(rcad_viewport::ViewPreset::Iso)),
            ],
            // Run 5: placed 3MF export of the tray for the P1S.
            "tray_export" => vec![
                tray(true),
                Ui(|a| {
                    a.set_view(rcad_viewport::ViewPreset::Iso);
                    let mut form = crate::exchange::PrintForm::new(
                        false,
                        "organizer_tray.3mf".into(),
                        ExportScope::AllVisible,
                    );
                    form.place = true;
                    form.layout = true;
                    form.bed = crate::exchange::BedChoice::P1s;
                    a.dialog = Some(Dialog::Export3mf(form));
                }),
            ],
            // Run 5: an impossible divider count refused, model unchanged.
            // Run 6: raised "B8" (each glyph region joined 1 mm; counters stay open).
            "sketch_text" => {
                let mut v = text_plate();
                v.push(Ui(|a| {
                    let sk = a
                        .summary
                        .features
                        .iter()
                        .find(|f| f.id == FeatureId(3))
                        .and_then(|f| f.sketch.clone());
                    let Some(mut sk) = sk else { return };
                    sk.solve();
                    for r in sk.regions().unwrap_or_default() {
                        if r.depth == 0 {
                            a.request(Command::Extrude {
                                name: None,
                                sketch: FeatureId(3),
                                distance: 1.0,
                                op: ExtrudeOp::Join,
                                target: None,
                                through_all: false,
                                reverse: false,
                                body_name: None,
                                region: Some(r.id),
                                options: Default::default(),
                            });
                        }
                    }
                }));
                v.push(Ui(|a| a.set_view(rcad_viewport::ViewPreset::Iso)));
                v
            }
            // Run 6: the sketch editor with the Text tool's dialog on "B8".
            "sketch_text_edit" => {
                let mut v = text_plate();
                v.push(Ui(|a| {
                    a.begin_sketch(crate::app::SketchTarget::Existing(FeatureId(3)))
                }));
                v.push(Ui(|a| {
                    if let Some(s) = a.sketch_session_mut() {
                        s.editor.set_tool(crate::sketch_edit::SketchTool::Text);
                        s.editor.begin_text_edit(1);
                    }
                }));
                v
            }
            "tray_refusal" => vec![
                tray(false),
                Cmd(Command::SetUserParameter {
                    name: "divider_count".into(),
                    expression: "60".into(),
                    unit: None,
                    comment: None,
                }),
                Ui(|a| a.set_view(rcad_viewport::ViewPreset::Iso)),
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
