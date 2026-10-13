//! Typed, serializable command API shared by the UI, the CLI, and tests.
//!
//! Every modeling action available in the app is a [`Command`]; [`Session::execute`]
//! returns a [`Response`] with feature ids, measurements, and structured errors.
//! See docs/COMMANDS.md.

pub mod acceptance;
pub mod assembly;
pub mod drawings;
pub mod mass;
pub mod measure_faces;
pub mod print;
pub mod printables;
pub mod scenarios;
pub mod sketches;

use rcad_document::{
    BodyGeometry, BodyId, BodySource, DocError, Document, Extent, ExtrudeOptions, FaceCandidate,
    FaceRef, FaceRole, FeatureId, FeatureKind, Model, Operation, SketchPlane, load_file, save_file,
    write_atomic,
};
use rcad_kernel::{self as k, Frame, Measurements, MeshCheck};
use rcad_sketch::{Sketch, SolveStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "template", rename_all = "snake_case")]
pub enum SketchTemplate {
    /// Dimensioned rectangle centred on `center` (default: sketch origin).
    /// Parameters: "width", "height", "center_x", "center_y".
    CenteredRectangle {
        width: f64,
        height: f64,
        #[serde(default)]
        center: Option<[f64; 2]>,
    },
    /// Circle centred on `center` (default: sketch origin).
    /// Parameters: "diameter", "center_x", "center_y".
    CenteredCircle {
        diameter: f64,
        #[serde(default)]
        center: Option<[f64; 2]>,
    },
    /// Closed polygon of lines through `vertices` (sketch-plane mm), sharing
    /// endpoints, with no other constraints (vertices stay editable).
    Polygon { vertices: Vec<[f64; 2]> },
    /// A complete sketch as produced by the interactive editor.
    Custom { sketch: Sketch },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    NewDocument,
    CreateSketch {
        #[serde(default)]
        name: Option<String>,
        plane: SketchPlane,
        #[serde(flatten)]
        template: SketchTemplate,
    },
    /// Extrude a sketch profile (ADR 0003): a new body (default), or joined to /
    /// cut from `target` (default: the only body, else required). `through_all`
    /// (Cut only) ignores `distance`. `reverse` extrudes against the sketch normal.
    Extrude {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
        #[serde(default)]
        distance: f64,
        #[serde(default, skip_serializing_if = "ExtrudeOp::is_new_body")]
        op: ExtrudeOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<BodyId>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        through_all: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reverse: bool,
        /// New body: its name (default "BodyN").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body_name: Option<String>,
        /// One bounded region of the sketch to extrude (identity from
        /// `Sketch::regions`); default: the whole profile (outer loop with
        /// its holes) as before. See docs/behavior/sketching.md.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<rcad_sketch::RegionRef>,
        /// Run-6 options (symmetric, two sides `distance2`, `start_offset`,
        /// up `to` a parallel plane, `thin` wall, `taper`, further `regions`);
        /// see `rcad_document::ExtrudeOptions`. Format v6 when used.
        #[serde(default, skip_serializing_if = "ExtrudeOptions::is_plain")]
        options: ExtrudeOptions,
    },
    /// New body: revolve a sketch about its local Y axis (WorldXZ sketch -> world Z axis),
    /// or (run 6) its local X axis or one of its lines (`axis`); `reverse` sweeps toward
    /// +normal instead of −normal.
    Revolve {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
        angle_deg: f64,
        #[serde(
            default,
            skip_serializing_if = "rcad_document::RevolveAxis::is_default"
        )]
        axis: rcad_document::RevolveAxis,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reverse: bool,
        /// Run 6: `join` / `cut` with `target` instead of a new body.
        #[serde(default, skip_serializing_if = "ExtrudeOp::is_new_body")]
        op: ExtrudeOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<BodyId>,
    },
    /// Exact loft (run 5, ADR 0008) between the closed profiles of sketches
    /// `a` and `b` (parallel planes) as a new body: equal-count polygons with
    /// parallel corresponding edges, or coaxial circles. Refused otherwise
    /// with the reason (ProductLimit), document unchanged.
    Loft {
        #[serde(default)]
        name: Option<String>,
        a: FeatureId,
        b: FeatureId,
        /// Run 6: intermediate section sketches, in order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        via: Vec<FeatureId>,
        #[serde(default)]
        body_name: Option<String>,
    },
    /// Exact sweep (run 5, ADR 0009): the closed profile of sketch `profile`
    /// along the open line/arc chain of sketch `path` (starting at the profile
    /// sketch's origin along its normal), as a new body.
    Sweep {
        #[serde(default)]
        name: Option<String>,
        profile: FeatureId,
        path: FeatureId,
        #[serde(default)]
        body_name: Option<String>,
    },
    CutThroughAll {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
    },
    /// Blind circular pocket from the extrusion cap the sketch is on, `depth` mm
    /// into the material (0 < depth < thickness; parameter "depth").
    Pocket {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
        depth: f64,
    },
    /// Repeat an Extrude Cut `count - 1` more times at `spacing` mm along
    /// `direction` (the seed sketch's local 2D axes; e.g. `[1, 0]` = sketch x),
    /// on the seed's target body. Parameters "count", "spacing".
    LinearPattern {
        #[serde(default)]
        name: Option<String>,
        feature: FeatureId,
        count: u32,
        spacing: f64,
        direction: [f64; 2],
    },
    /// Repeat an Extrude Join/Cut `count - 1` more times, each rotated a further
    /// `angle_deg` counter-clockwise about the seed sketch's normal through
    /// `center` (seed sketch 2D coordinates). Parameters "count", "angle_deg".
    CircularPattern {
        #[serde(default)]
        name: Option<String>,
        feature: FeatureId,
        count: u32,
        angle_deg: f64,
        center: [f64; 2],
    },
    /// Add the mirror image of an Extrude Join/Cut's tool across `plane` (world
    /// plane through the origin, or a planar face) to the seed's target body.
    Mirror {
        #[serde(default)]
        name: Option<String>,
        feature: FeatureId,
        plane: SketchPlane,
    },
    /// Chamfer the straight convex edge between two planar faces (semantic
    /// references, e.g. from `FeatureSummary.faces`), `distance` mm along each.
    Chamfer {
        #[serde(default)]
        name: Option<String>,
        a: FaceRef,
        b: FaceRef,
        distance: f64,
    },
    /// Exact solid fillet (run 5) of the straight convex edge between planar
    /// faces `a` and `b` (semantic references), `radius` mm: an exact cylinder
    /// tangent to both. Body prismatic along the edge with perpendicular end
    /// faces; refused otherwise. Parameter `radius`.
    Fillet {
        #[serde(default)]
        name: Option<String>,
        a: FaceRef,
        b: FaceRef,
        radius: f64,
    },
    /// Direct edit (run 6, ADR 0010) of an imported STEP body at imported face
    /// `face` (`FaceRole::Imported`, stable import index; see
    /// [`Command::ImportedFaces`]): offset a planar face, resize or remove the
    /// hole/boss of a cylindrical face. Exact; refused with the reason when the
    /// preconditions do not hold. Parameters `distance` / `diameter`.
    DirectEdit {
        #[serde(default)]
        name: Option<String>,
        face: FaceRef,
        op: rcad_document::DirectOp,
    },
    /// Exact countersink (run 6) at the entry of Hole feature `hole`: `diameter`
    /// mm at the entry face, included angle `angle_deg` (default 90).
    /// Parameters `diameter`, `angle_deg`.
    Countersink {
        #[serde(default)]
        name: Option<String>,
        hole: FeatureId,
        diameter: f64,
        #[serde(default = "default_csink_angle")]
        angle_deg: f64,
    },
    /// Modeled thread (run 6, ADR 0011) as a new body: a threaded rod, or a
    /// square nut when `nut_width` is given (internal thread with radial
    /// `clearance`, default 0.1). Axis along the normal of `plane` through
    /// `center`; `length` a whole number of pitches; faceted with chord
    /// tolerance `chord_tol` (default 0.01 mm).
    Thread {
        #[serde(default)]
        name: Option<String>,
        plane: SketchPlane,
        #[serde(default)]
        center: [f64; 2],
        diameter: f64,
        pitch: f64,
        length: f64,
        #[serde(default)]
        left_hand: bool,
        #[serde(default = "default_chord_tol")]
        chord_tol: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nut_width: Option<f64>,
        #[serde(default = "default_clearance")]
        clearance: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body_name: Option<String>,
    },
    /// Query (run 6): the faces of imported body `body` that direct edits can
    /// name, as JSON in `message`: `[{"face": FaceRef, "kind": DirectFace}]`
    /// in current B-rep order. Fails when the body is not an imported body
    /// changed only by direct edits.
    ImportedFaces {
        body: BodyId,
    },
    /// Round hole (run 4) from a planar face of `target` (default: the body
    /// owning the face), centred at `center` (plane coordinates): `diameter`,
    /// `depth` mm (omit for through all), optional `counterbore`
    /// `[diameter, depth]`. Patternable and mirrorable like an Extrude Cut.
    Hole {
        #[serde(default)]
        name: Option<String>,
        plane: SketchPlane,
        center: [f64; 2],
        diameter: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        counterbore: Option<[f64; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<BodyId>,
    },
    /// Scoped shell (docs/SHELL.md): hollow the body of the New Body extrusion
    /// `extrude` (single closed loop or circle) to walls and floor `thickness`
    /// mm thick, open at `open` (`ExtrudeEndCap` default, or `ExtrudeStartCap`).
    Shell {
        #[serde(default)]
        name: Option<String>,
        extrude: FeatureId,
        #[serde(default = "default_shell_open")]
        open: FaceRole,
        thickness: f64,
    },
    SetParameter {
        feature: FeatureId,
        name: String,
        value: f64,
    },
    /// Create or redefine the named user parameter `name` (docs/PARAMETERS.md).
    /// `expression` may use numbers with units, other parameters, + - * /,
    /// parentheses, min/max/abs. `unit` (default mm) is the declared unit.
    /// One undoable step: every bound feature value is re-derived and the model
    /// regenerated; any error leaves the document unchanged.
    SetUserParameter {
        name: String,
        expression: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<rcad_document::ParamUnit>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
    },
    /// Set several user parameters in ONE transaction (one undo step): no
    /// intermediate combination has to be valid, only the final one.
    SetUserParameters {
        parameters: Vec<ParameterDef>,
    },
    /// Rename a user parameter, rewriting every expression that uses it.
    RenameUserParameter {
        name: String,
        new_name: String,
    },
    /// Delete a user parameter; refused while anything uses it.
    DeleteUserParameter {
        name: String,
    },
    /// Bind feature parameter `name` (e.g. "distance", a sketch dimension) to
    /// `expression`. A plain number sets the value and removes the binding; an
    /// empty expression removes the binding and keeps the current value.
    SetExpression {
        feature: FeatureId,
        name: String,
        expression: String,
    },
    /// Replace a sketch feature's definition (one undoable step; the editor's
    /// Finish Sketch). Plane and downstream references are kept.
    EditSketch {
        feature: FeatureId,
        sketch: Sketch,
    },
    /// Add a text item to a sketch (run 6, docs/behavior/sketching.md "Sketch
    /// text"): the font is resolved now from the installed fonts and the glyph
    /// outlines are stored in the document. One undoable step. The response's
    /// `text` is the new text id.
    AddSketchText {
        feature: FeatureId,
        text: rcad_sketch::TextSpec,
    },
    /// Change a text item. A new string or font re-resolves the outlines and
    /// needs the font installed (refused otherwise, nothing changes); height,
    /// position, anchor, rotation and alignment edits reuse the stored outlines.
    EditSketchText {
        feature: FeatureId,
        id: u32,
        text: rcad_sketch::TextSpec,
    },
    /// Remove a text item from a sketch.
    RemoveSketchText {
        feature: FeatureId,
        id: u32,
    },
    RenameFeature {
        feature: FeatureId,
        name: String,
    },
    RenameBody {
        body: BodyId,
        name: String,
    },
    SetBodyVisible {
        body: BodyId,
        visible: bool,
    },
    /// Physical material of a body for mass properties (run 6, format v6; one
    /// undo step). `material`: a catalog key (`rcad_document::material::CATALOG`,
    /// e.g. "pla"); or `density` (in `density_unit`, default g/cm³) with an
    /// optional `name` for a user-defined material; neither: clear it. Both
    /// `material` and `density` is refused (ambiguous provenance).
    SetBodyMaterial {
        body: BodyId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        material: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        density: Option<f64>,
        #[serde(default)]
        density_unit: rcad_document::DensityUnit,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Body Combine (run 6, format v6; one undo step): join `tools` to,
    /// cut them from, or intersect them with `target`
    /// (`FeatureKind::Combine`). Without `keep_tools` the tool bodies are
    /// consumed (they leave the model; their creators stay in history).
    /// Refused (document unchanged) when Truck fails, the result is not one
    /// closed solid, or the independent check disagrees (`GeometryCheck`).
    Combine {
        #[serde(default)]
        name: Option<String>,
        target: BodyId,
        tools: Vec<BodyId>,
        op: k::CombineOp,
        #[serde(default)]
        keep_tools: bool,
    },
    /// Components / occurrences / joints (run 6 pull-forward of Run-7 §1-2,
    /// docs/ASSEMBLIES.md; format v6). A component groups bodies of this
    /// document and gets a first occurrence at identity.
    ///
    /// Run 7: `occurrences` (siblings with the same owner) move into the new
    /// component's definition, which becomes a sub-assembly; its first
    /// occurrence takes their former owner at identity, so nothing moves.
    /// With neither bodies nor occurrences an empty component is made (activate
    /// it to model into it).
    CreateComponent {
        name: String,
        #[serde(default)]
        bodies: Vec<BodyId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part_number: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        occurrences: Vec<rcad_document::assembly::OccurrenceId>,
    },
    /// Remove a component and its occurrences (its bodies become loose bodies).
    DeleteComponent {
        component: rcad_document::assembly::ComponentId,
    },
    SetPartNumber {
        component: rcad_document::assembly::ComponentId,
        part_number: String,
    },
    /// A further linked occurrence: rotate `angle_deg` about `axis` (default +Z)
    /// through the world origin, then translate.
    ///
    /// Run 7: with `owner`, the occurrence is a child in that component's
    /// definition (a sub-assembly; placement relative to it). Cyclic
    /// containment is refused.
    AddOccurrence {
        component: rcad_document::assembly::ComponentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<rcad_document::assembly::ComponentId>,
        #[serde(default)]
        translate: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<[f64; 3]>,
        #[serde(default)]
        angle_deg: f64,
    },
    SetOccurrenceTransform {
        occurrence: rcad_document::assembly::OccurrenceId,
        #[serde(default)]
        translate: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<[f64; 3]>,
        #[serde(default)]
        angle_deg: f64,
    },
    DeleteOccurrence {
        occurrence: rcad_document::assembly::OccurrenceId,
    },
    SetGrounded {
        occurrence: rcad_document::assembly::OccurrenceId,
        grounded: bool,
    },
    /// Joint (`rigid`, `revolute` value in degrees, `slider` value in mm) moving
    /// `child` with `parent`; `origin`/`axis` in the parent's definition
    /// coordinates. The child keeps its current pose at `value`.
    AddJoint {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        kind: rcad_document::assembly::JointKind,
        parent: rcad_document::assembly::OccurrenceId,
        child: rcad_document::assembly::OccurrenceId,
        #[serde(default)]
        origin: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<[f64; 3]>,
        /// Take `origin`/`axis` from a cylinder face or the line where two
        /// planar faces of the parent's definition bodies meet (overrides them).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis_from: Option<assembly::AxisFrom>,
        #[serde(default)]
        value: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        /// Run 7: second/third values with limits (cylindrical, pin-slot,
        /// planar, ball).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra: Vec<rcad_document::assembly::DofValue>,
        /// Run 7: slot direction (pin-slot) / in-plane u (planar, ball).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dir2: Option<[f64; 3]>,
    },
    /// Set value `index` (default 0) of a joint; refused outside its limits.
    SetJointValue {
        joint: rcad_document::assembly::JointId,
        value: f64,
        #[serde(default, skip_serializing_if = "assembly::is_zero_usize")]
        index: usize,
    },
    SetJointLimits {
        joint: rcad_document::assembly::JointId,
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "assembly::is_zero_usize")]
        index: usize,
    },
    /// Run 7: set all values of a multi-DOF joint at once (atomic).
    SetJointValues {
        joint: rcad_document::assembly::JointId,
        values: Vec<f64>,
    },
    DeleteJoint {
        joint: rcad_document::assembly::JointId,
    },
    /// Flat bill of materials (`Response::assembly.bom`). Read-only.
    Bom,
    /// Pairwise interference between occurrences (`Response::assembly.interference`).
    /// Read-only; a static check, not collision avoidance.
    Interference {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        occurrences: Option<Vec<rcad_document::assembly::OccurrenceId>>,
    },
    /// Every occurrence's placed bodies (and loose visible bodies) as separate
    /// named objects (STEP solids or 3MF objects).
    ExportAssembly {
        path: PathBuf,
        format: assembly::AssemblyFormat,
    },
    /// Run 7: rename a component (definition name; BOM row).
    RenameComponent {
        component: rcad_document::assembly::ComponentId,
        name: String,
    },
    /// Run 7: rename an occurrence.
    RenameOccurrence {
        occurrence: rcad_document::assembly::OccurrenceId,
        name: String,
    },
    /// Run 7: make `component` the active component (bodies created while it
    /// is active join its definition); `null` returns to the top level.
    ActivateComponent {
        #[serde(default)]
        component: Option<rcad_document::assembly::ComponentId>,
    },
    /// Run 7: hide/show an occurrence (display only; still in BOM, mass,
    /// interference and export).
    SetOccurrenceHidden {
        occurrence: rcad_document::assembly::OccurrenceId,
        hidden: bool,
    },
    /// Run 7: break the link of `occurrence`: a new component (independent
    /// copy) with copies of the definition bodies, made from the current
    /// geometry by body-copy features; the occurrence keeps its placement and
    /// stops following later edits of the source definition.
    MakeIndependent {
        occurrence: rcad_document::assembly::OccurrenceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Run 7: rigid group: rigid joints from the first occurrence to each of
    /// the others (siblings, free, not grounded); one undo step.
    CreateRigidGroup {
        occurrences: Vec<rcad_document::assembly::OccurrenceId>,
    },
    /// Run 7: exploded-view offset (world mm) of the instance `path` ("O3/O7");
    /// display state only. A zero offset removes the entry.
    SetExplodeOffset {
        path: String,
        offset: [f64; 3],
    },
    /// Run 7: show or hide the exploded view.
    SetExploded {
        exploded: bool,
    },
    /// Run 7: move a free occurrence so that planar face `from` (of its
    /// definition) lies on planar face `to` of another occurrence, facing it
    /// (normals opposite) with `offset` mm gap; translation + minimal rotation.
    AlignOccurrence {
        occurrence: rcad_document::assembly::OccurrenceId,
        from: measure_faces::FacePick,
        target: rcad_document::assembly::OccurrenceId,
        to: measure_faces::FacePick,
        #[serde(default)]
        offset: f64,
    },
    /// Exact geometry of face `a` (plane, or cylinder with radius/diameter)
    /// and, with `b`, the distance or angle between the two faces
    /// (`Response::face_measure`; read-only; world frame, mm, degrees).
    MeasureFaces {
        a: measure_faces::FacePick,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        b: Option<measure_faces::FacePick>,
    },
    /// Volume, mass, centre of mass and inertia of `bodies` (default: every
    /// visible body) and their total (`Response::mass_properties`). Read-only.
    MassProperties {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bodies: Option<Vec<BodyId>>,
        /// Occurrences to include (default: all when `bodies` is absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        occurrences: Option<Vec<rcad_document::assembly::OccurrenceId>>,
    },
    /// Suppress (or restore) a feature: kept in history, not evaluated (run
    /// 5; one undo step). With `with_dependents`, every feature depending on
    /// it (transitively) gets the same state; otherwise the change is refused
    /// when it would leave an evaluated feature depending on a suppressed one.
    SetSuppressed {
        feature: FeatureId,
        suppressed: bool,
        #[serde(default)]
        with_dependents: bool,
    },
    /// Project geometry into a sketch, associatively (run 6, SK03; one undo
    /// step): every boundary edge of a planar `Face` parallel to the sketch,
    /// or the non-construction curves of another `Sketch` on a parallel plane.
    /// The projected lines/arcs/circles are pinned by fixed points that follow
    /// the source at every regeneration (`ProjectionLink`); as `construction`
    /// they stay out of profiles. Refused for a later or dependent source
    /// (cycle), an oblique source, or edges other than lines and arcs.
    Project {
        sketch: FeatureId,
        source: rcad_document::ProjectSource,
        #[serde(default)]
        construction: bool,
    },
    /// Break projection `link` (0-based) of `sketch` (one undo step): its
    /// geometry stays where the last regeneration put it, as plain fixed
    /// curves that no longer follow the source.
    UnlinkProjection {
        sketch: FeatureId,
        link: usize,
    },
    /// Rigid move of a body, or an independent copy (run 6; one undo step):
    /// rotate `angle_deg` about the axis through `origin` along `axis`
    /// (default +Z), then translate. With `copy`, a new body (`body_name`)
    /// holds the moved result and the source stays.
    MoveBody {
        #[serde(default)]
        name: Option<String>,
        body: BodyId,
        #[serde(default)]
        translate: [f64; 3],
        #[serde(default)]
        origin: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<[f64; 3]>,
        #[serde(default)]
        angle_deg: f64,
        #[serde(default)]
        copy: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body_name: Option<String>,
    },
    /// Create or replace configuration `name` (run 6; one undo step):
    /// user-parameter expressions, the features it suppresses (they become
    /// configured features), part number. Re-applied when it is active.
    SetConfiguration {
        name: String,
        #[serde(default)]
        values: std::collections::BTreeMap<String, String>,
        #[serde(default)]
        suppressed: Vec<FeatureId>,
        #[serde(default)]
        part_number: String,
        /// Component occurrences this configuration excludes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        excluded_occurrences: Vec<rcad_document::assembly::OccurrenceId>,
    },
    /// Exclude (or include) a component occurrence (run 6; one undo step):
    /// a suppressed occurrence leaves display, BOM, mass, interference and
    /// assembly export.
    SetOccurrenceSuppressed {
        occurrence: rcad_document::assembly::OccurrenceId,
        suppressed: bool,
    },
    DeleteConfiguration {
        name: String,
    },
    /// Run 7: replace every configuration at once (the configuration table
    /// editor; one undo step). Each row is checked like `set_configuration`
    /// and names must be unique; the active configuration is re-applied (or
    /// cleared when it is no longer in the table). Features and occurrences
    /// that no configuration controls any more are restored (unsuppressed).
    SetConfigurationTable {
        configurations: Vec<rcad_document::Configuration>,
    },
    /// Apply configuration `name` (one undo step): its parameter values and
    /// the suppression of every configured feature; refused unchanged when the
    /// result does not regenerate.
    ActivateConfiguration {
        name: String,
    },
    /// Export every configuration (or `configurations`) as
    /// `<dir>/<stem>-<configuration>[-<part number>].<ext>` for each requested
    /// format; the document, its undo/redo history and the dirty flag are
    /// restored afterwards (the activations leave no history). Names are sanitized; an existing file is overwritten only
    /// with `overwrite`.
    ExportConfigurations {
        dir: PathBuf,
        stem: String,
        #[serde(default)]
        configurations: Vec<String>,
        #[serde(default)]
        step: bool,
        #[serde(default)]
        stl: bool,
        #[serde(default, rename = "3mf")]
        threemf: bool,
        #[serde(default)]
        overwrite: bool,
    },
    /// Sheet-metal contour flange (run 6; docs/SHEET_METAL.md): the open
    /// line chain of `sketch` with `thickness`, inner bend `radius`,
    /// `k_factor` (default 0.44) and `width`; `flip` puts the material on the
    /// left of the chain. A new body (`body_name`).
    SheetMetal {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
        thickness: f64,
        radius: f64,
        #[serde(default = "default_k")]
        k_factor: f64,
        width: f64,
        #[serde(default)]
        flip: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body_name: Option<String>,
    },
    /// Write the flat pattern of sheet-metal feature `feature` as an R12 DXF
    /// (mm): outline on layer CUT, bend centre lines on layer BEND. The
    /// response message carries the flat length.
    ExportFlatPattern {
        feature: FeatureId,
        path: PathBuf,
    },
    /// Align the body of planar face `from` flush onto face `to` (run 6; see
    /// `FeatureKind::AlignBody`).
    AlignBody {
        #[serde(default)]
        name: Option<String>,
        from: FaceRef,
        to: FaceRef,
        #[serde(default)]
        offset: f64,
    },
    /// Uniform scale of a body about `center` (run 6; one undo step).
    ScaleBody {
        #[serde(default)]
        name: Option<String>,
        body: BodyId,
        #[serde(default)]
        center: [f64; 3],
        factor: f64,
    },
    /// Shell from any planar face of a body prismatic along its normal (run
    /// 6; see `FeatureKind::ShellFace`).
    ShellFace {
        #[serde(default)]
        name: Option<String>,
        face: FaceRef,
        thickness: f64,
    },
    /// Split `body` by `plane` (run 6; one undo step): the body keeps the
    /// part behind the plane, a new body (`body_name`) the part in front.
    SplitBody {
        #[serde(default)]
        name: Option<String>,
        body: BodyId,
        plane: SketchPlane,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body_name: Option<String>,
    },
    /// Timeline marker (run 6; one undo step): evaluate history up to and
    /// including `at`; later features are kept but not evaluated, and new
    /// features are inserted after the marker. `at: null` rolls to the end.
    SetRollback {
        #[serde(default)]
        at: Option<FeatureId>,
    },
    /// Reorder history (run 6; one undo step): move `feature` before
    /// `before` (null: to the end). Refused when a dependency would come after
    /// it or a dependent before it, or when the new order does not regenerate
    /// (the document is unchanged).
    MoveFeature {
        feature: FeatureId,
        #[serde(default)]
        before: Option<FeatureId>,
    },
    /// Re-attach a sketch to another plane or face (repair of a broken face
    /// reference; one undo step). The sketch's local geometry is kept.
    SetSketchPlane {
        feature: FeatureId,
        plane: SketchPlane,
    },
    /// Import exact solids from a STEP file as new bodies (one per solid). The
    /// file is embedded in the document. `units_to_mm` is used only when the
    /// file declares no length unit.
    ImportStep {
        path: PathBuf,
        #[serde(default)]
        units_to_mm: Option<f64>,
    },
    /// Import an STL file as a mesh body (display/export only). STL is unitless:
    /// `units_to_mm` (mm per file unit) is required.
    ImportStl {
        path: PathBuf,
        #[serde(default)]
        units_to_mm: Option<f64>,
    },
    /// Import the 2D geometry of a DXF file as a new (unconstrained) sketch on
    /// `plane`. `units_to_mm` is used only when the file declares no unit.
    ImportDxf {
        path: PathBuf,
        plane: SketchPlane,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        units_to_mm: Option<f64>,
        /// Skip unsupported entities (reported) instead of refusing the file.
        #[serde(default)]
        allow_partial: bool,
        #[serde(default)]
        layers: Option<Vec<String>>,
    },
    /// Import every build item of a 3MF package as a mesh body (units from the
    /// file; source embedded). Display/export only, like STL.
    #[serde(rename = "import_3mf")]
    Import3mf {
        path: PathBuf,
    },
    /// Write `bodies` (default: all visible) as one 3MF object each: B-rep
    /// bodies tessellated at `chord` (watertightness checked), mesh bodies as
    /// imported; `unit` (default millimeter) is the file's declared unit.
    #[serde(rename = "export_3mf")]
    Export3mf {
        path: PathBuf,
        #[serde(default)]
        chord: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bodies: Option<Vec<BodyId>>,
        #[serde(default)]
        unit: Option<rcad_exchange::threemf::ThreeMfUnit>,
        /// Print options (angle, placement, bed, strict mesh policy): docs/PRINT_EXPORT.md.
        #[serde(flatten, default)]
        print: print::PrintOptions,
    },
    /// Import the planar paths of an SVG file (subset: docs/formats/svg.md) as a
    /// new unconstrained sketch on `plane`. `units_to_mm` (mm per user unit) is
    /// used only when the file declares no physical size.
    ImportSvg {
        path: PathBuf,
        plane: SketchPlane,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        units_to_mm: Option<f64>,
        #[serde(default)]
        allow_partial: bool,
    },
    /// Write a sketch's lines/arcs/circles as SVG (local 2D coordinates, mm, y
    /// negated for SVG's y-down axis).
    ExportSvg {
        path: PathBuf,
        sketch: FeatureId,
    },
    /// Write a sketch's lines/arcs/circles as an R12 ASCII DXF (mm).
    ExportDxf {
        path: PathBuf,
        sketch: FeatureId,
    },
    /// Remove a feature that nothing depends on (one undoable step). Refused
    /// with the dependent features named otherwise.
    DeleteFeature {
        feature: FeatureId,
    },
    Undo,
    Redo,
    Save {
        path: PathBuf,
    },
    Open {
        path: PathBuf,
    },
    /// Write the document to `path` (atomic) without changing the session's
    /// path or dirty state: autosave / recovery copies. Deferred measurements
    /// are not forced (the cache may omit them).
    SaveCopy {
        path: PathBuf,
    },
    /// Open `path` as a new, unsaved, untitled document (a recovery copy:
    /// Save asks for a path instead of overwriting the copy).
    OpenCopy {
        path: PathBuf,
    },
    Close,
    /// Binary STL of `bodies` (default: all visible bodies). B-rep bodies are
    /// tessellated (watertightness checked); mesh bodies are written as imported.
    ExportStl {
        path: PathBuf,
        #[serde(default)]
        chord: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bodies: Option<Vec<BodyId>>,
        /// Print options (angle, placement, bed, separate files, strict mesh
        /// policy): docs/PRINT_EXPORT.md.
        #[serde(flatten, default)]
        print: print::PrintOptions,
    },
    /// STEP (AP214 via truck-stepio) of the exact B-rep `bodies` (default: all
    /// visible B-rep bodies; visible mesh bodies are named in the message as not
    /// written), one solid per body. An explicit selection with a mesh is refused.
    ExportStep {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bodies: Option<Vec<BodyId>>,
    },
    /// Drawing sheet (run 6, format v6; docs/DRAWINGS.md). With `bodies`, a
    /// Front base view (auto scale) and, unless `standard_views: false`, the
    /// views from above and from the right. `Response.drawing` is the new id.
    CreateDrawing {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<rcad_document::drawing::SheetSize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection: Option<rcad_document::drawing::Projection>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part_number: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        material: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bodies: Vec<BodyId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        standard_views: Option<bool>,
        /// Run 7: an assembly drawing: the base view shows the evaluated
        /// assembly (every placed occurrence, plus `bodies` as loose bodies),
        /// with balloons on it and a BOM table on the sheet.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        assembly: bool,
    },
    /// Change drawing fields (empty `part_number` / `material`: from the model).
    EditDrawing {
        drawing: rcad_document::drawing::DrawingId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<rcad_document::drawing::SheetSize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection: Option<rcad_document::drawing::Projection>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part_number: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        material: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auto_dimensions: Option<bool>,
    },
    DeleteDrawing {
        drawing: rcad_document::drawing::DrawingId,
    },
    /// Add a base view (`{"kind": "base", "bodies", "orientation", "scale"?,
    /// "position"?}`) or a projected view (`{"kind": "projected", "parent",
    /// "from"}`). `Response.view` is the new view id.
    AddDrawingView {
        drawing: rcad_document::drawing::DrawingId,
        view: drawings::ViewSpec,
    },
    /// Scale, position (sheet mm) or orientation of a base view.
    EditDrawingView {
        drawing: rcad_document::drawing::DrawingId,
        view: rcad_document::drawing::ViewId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<[f64; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        orientation: Option<rcad_document::drawing::Orientation>,
    },
    /// Remove a view (refused while projected views depend on it).
    RemoveDrawingView {
        drawing: rcad_document::drawing::DrawingId,
        view: rcad_document::drawing::ViewId,
    },
    /// Semantic diameter callout of Hole feature `feature` in a view.
    AddHoleCallout {
        drawing: rcad_document::drawing::DrawingId,
        view: rcad_document::drawing::ViewId,
        feature: FeatureId,
    },
    RemoveHoleCallout {
        drawing: rcad_document::drawing::DrawingId,
        view: rcad_document::drawing::ViewId,
        feature: FeatureId,
    },
    /// Run 7: a driven dimension in a view between semantic references
    /// (`{"face": {feature, role}}` planar face or `{"hole": F}` hole axis),
    /// or the diameter of a hole (`b` omitted), with an optional tolerance
    /// (`{"type": "symmetric", "value"}`, `deviation`/`limits` with
    /// `upper`/`lower`). Measured at every render; a reference that stops
    /// resolving is shown and reported as dangling.
    AddDrawingDimension {
        drawing: rcad_document::drawing::DrawingId,
        view: rcad_document::drawing::ViewId,
        a: rcad_document::drawing::DimRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        b: Option<rcad_document::drawing::DimRef>,
        #[serde(default)]
        tolerance: rcad_document::drawing::Tolerance,
    },
    /// Change the tolerance of annotation `index` (a dimension) of a view.
    SetDimensionTolerance {
        drawing: rcad_document::drawing::DrawingId,
        view: rcad_document::drawing::ViewId,
        index: usize,
        tolerance: rcad_document::drawing::Tolerance,
    },
    /// Remove annotation `index` (0-based, in the view's annotation list) of a view.
    RemoveDrawingAnnotation {
        drawing: rcad_document::drawing::DrawingId,
        view: rcad_document::drawing::ViewId,
        index: usize,
    },
    /// Show (or hide) BOM balloons in a view.
    SetBalloons {
        drawing: rcad_document::drawing::DrawingId,
        view: rcad_document::drawing::ViewId,
        show: bool,
    },
    /// Show (or hide) the BOM table; `position`: its bottom-left corner
    /// (sheet mm; default on the title block).
    SetBomTable {
        drawing: rcad_document::drawing::DrawingId,
        show: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<[f64; 2]>,
    },
    /// Add a text note at a sheet position (mm); `Response.view` unused.
    AddDrawingNote {
        drawing: rcad_document::drawing::DrawingId,
        text: String,
        position: [f64; 2],
    },
    RemoveDrawingNote {
        drawing: rcad_document::drawing::DrawingId,
        index: usize,
    },
    /// Render the drawing from the current geometry and write it (`format`
    /// default from the extension: .svg / .pdf / .dxf). `Response.drawing_report`.
    ExportDrawing {
        drawing: rcad_document::drawing::DrawingId,
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<drawings::DrawingFormat>,
    },
    Measure,
    Inspect,
}

impl Command {
    /// A new-body distance extrusion (the common case).
    pub fn extrude(name: Option<String>, sketch: FeatureId, distance: f64) -> Self {
        Command::Extrude {
            name,
            sketch,
            distance,
            op: ExtrudeOp::NewBody,
            target: None,
            through_all: false,
            reverse: false,
            body_name: None,
            region: None,
            options: ExtrudeOptions::default(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Command::NewDocument => "new_document",
            Command::CreateSketch { .. } => "create_sketch",
            Command::Extrude { .. } => "extrude",
            Command::CutThroughAll { .. } => "cut_through_all",
            Command::Pocket { .. } => "pocket",
            Command::Revolve { .. } => "revolve",
            Command::Loft { .. } => "loft",
            Command::Sweep { .. } => "sweep",
            Command::LinearPattern { .. } => "linear_pattern",
            Command::Mirror { .. } => "mirror",
            Command::CircularPattern { .. } => "circular_pattern",
            Command::Chamfer { .. } => "chamfer",
            Command::Fillet { .. } => "fillet",
            Command::DirectEdit { .. } => "direct_edit",
            Command::Countersink { .. } => "countersink",
            Command::Thread { .. } => "thread",
            Command::ImportedFaces { .. } => "imported_faces",
            Command::SetParameter { .. } => "set_parameter",
            Command::Hole { .. } => "hole",
            Command::Shell { .. } => "shell",
            Command::SetUserParameter { .. } => "set_user_parameter",
            Command::SetUserParameters { .. } => "set_user_parameters",
            Command::RenameUserParameter { .. } => "rename_user_parameter",
            Command::DeleteUserParameter { .. } => "delete_user_parameter",
            Command::SetExpression { .. } => "set_expression",
            Command::EditSketch { .. } => "edit_sketch",
            Command::AddSketchText { .. } => "add_sketch_text",
            Command::EditSketchText { .. } => "edit_sketch_text",
            Command::RemoveSketchText { .. } => "remove_sketch_text",
            Command::RenameFeature { .. } => "rename_feature",
            Command::RenameBody { .. } => "rename_body",
            Command::SetSuppressed { .. } => "set_suppressed",
            Command::SetRollback { .. } => "set_rollback",
            Command::SetConfiguration { .. } => "set_configuration",
            Command::DeleteConfiguration { .. } => "delete_configuration",
            Command::SetConfigurationTable { .. } => "set_configuration_table",
            Command::SetOccurrenceSuppressed { .. } => "set_occurrence_suppressed",
            Command::ActivateConfiguration { .. } => "activate_configuration",
            Command::ExportConfigurations { .. } => "export_configurations",
            Command::MoveBody { .. } => "move_body",
            Command::SplitBody { .. } => "split_body",
            Command::ShellFace { .. } => "shell_face",
            Command::ScaleBody { .. } => "scale_body",
            Command::AlignBody { .. } => "align_body",
            Command::SheetMetal { .. } => "sheet_metal",
            Command::ExportFlatPattern { .. } => "export_flat_pattern",
            Command::Project { .. } => "project",
            Command::UnlinkProjection { .. } => "unlink_projection",
            Command::MoveFeature { .. } => "move_feature",
            Command::SetBodyVisible { .. } => "set_body_visible",
            Command::SetBodyMaterial { .. } => "set_body_material",
            Command::MassProperties { .. } => "mass_properties",
            Command::Combine { .. } => "combine",
            Command::MeasureFaces { .. } => "measure_faces",
            Command::CreateComponent { .. } => "create_component",
            Command::DeleteComponent { .. } => "delete_component",
            Command::SetPartNumber { .. } => "set_part_number",
            Command::AddOccurrence { .. } => "add_occurrence",
            Command::SetOccurrenceTransform { .. } => "set_occurrence_transform",
            Command::DeleteOccurrence { .. } => "delete_occurrence",
            Command::SetGrounded { .. } => "set_grounded",
            Command::AddJoint { .. } => "add_joint",
            Command::SetJointValue { .. } => "set_joint_value",
            Command::SetJointLimits { .. } => "set_joint_limits",
            Command::DeleteJoint { .. } => "delete_joint",
            Command::Bom => "bom",
            Command::Interference { .. } => "interference",
            Command::ExportAssembly { .. } => "export_assembly",
            Command::SetJointValues { .. } => "set_joint_values",
            Command::RenameComponent { .. } => "rename_component",
            Command::RenameOccurrence { .. } => "rename_occurrence",
            Command::ActivateComponent { .. } => "activate_component",
            Command::SetOccurrenceHidden { .. } => "set_occurrence_hidden",
            Command::MakeIndependent { .. } => "make_independent",
            Command::CreateRigidGroup { .. } => "create_rigid_group",
            Command::SetExplodeOffset { .. } => "set_explode_offset",
            Command::SetExploded { .. } => "set_exploded",
            Command::AlignOccurrence { .. } => "align_occurrence",
            Command::SetSketchPlane { .. } => "set_sketch_plane",
            Command::ImportStep { .. } => "import_step",
            Command::ImportStl { .. } => "import_stl",
            Command::ImportDxf { .. } => "import_dxf",
            Command::ExportDxf { .. } => "export_dxf",
            Command::ImportSvg { .. } => "import_svg",
            Command::ExportSvg { .. } => "export_svg",
            Command::Import3mf { .. } => "import_3mf",
            Command::Export3mf { .. } => "export_3mf",
            Command::DeleteFeature { .. } => "delete_feature",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::Save { .. } => "save",
            Command::Open { .. } => "open",
            Command::Close => "close",
            Command::SaveCopy { .. } => "save_copy",
            Command::OpenCopy { .. } => "open_copy",
            Command::ExportStl { .. } => "export_stl",
            Command::ExportStep { .. } => "export_step",
            Command::CreateDrawing { .. } => "create_drawing",
            Command::EditDrawing { .. } => "edit_drawing",
            Command::DeleteDrawing { .. } => "delete_drawing",
            Command::AddDrawingView { .. } => "add_drawing_view",
            Command::EditDrawingView { .. } => "edit_drawing_view",
            Command::RemoveDrawingView { .. } => "remove_drawing_view",
            Command::AddHoleCallout { .. } => "add_hole_callout",
            Command::RemoveHoleCallout { .. } => "remove_hole_callout",
            Command::AddDrawingDimension { .. } => "add_drawing_dimension",
            Command::SetDimensionTolerance { .. } => "set_dimension_tolerance",
            Command::RemoveDrawingAnnotation { .. } => "remove_drawing_annotation",
            Command::SetBalloons { .. } => "set_balloons",
            Command::SetBomTable { .. } => "set_bom_table",
            Command::AddDrawingNote { .. } => "add_drawing_note",
            Command::RemoveDrawingNote { .. } => "remove_drawing_note",
            Command::ExportDrawing { .. } => "export_drawing",
            Command::Measure => "measure",
            Command::Inspect => "inspect",
        }
    }
}

fn default_k() -> f64 {
    0.44
}

/// One entry of [`Command::SetUserParameters`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterDef {
    pub name: String,
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<rcad_document::ParamUnit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// What an extrusion does with its tool body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExtrudeOp {
    #[default]
    NewBody,
    Join,
    Cut,
    /// Keep only the target's material inside the tool (run 6).
    Intersect,
}

impl ExtrudeOp {
    fn is_new_body(&self) -> bool {
        *self == ExtrudeOp::NewBody
    }
}

/// Inspectable per-body state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodySummary {
    pub id: BodyId,
    pub name: String,
    pub visible: bool,
    /// "brep" (exact solid) or "mesh" (triangle mesh, STL import).
    pub kind: String,
    pub source: BodySource,
    pub created_by: FeatureId,
    pub last_feature: FeatureId,
    /// Exact B-rep measurements (None: deferred or a mesh body).
    #[serde(default)]
    pub measurements: Option<Measurements>,
    /// Mesh bodies: diagnostics (closed, boundary edges, volume if closed...).
    #[serde(default)]
    pub mesh: Option<rcad_document::import::MeshDiagnostics>,
    /// B-rep bodies: the plane of every planar face (outward normal), for
    /// clients that must apply regeneration's face rules (extrude preview).
    #[serde(default)]
    pub planes: Vec<rcad_kernel::FacePlane>,
    /// B-rep bodies: exact bounds, available even while measurement is
    /// deferred (inspection, Through All preview of hidden bodies).
    #[serde(default)]
    pub bounds: Option<rcad_kernel::Bounds>,
    /// Why measuring this body failed (stable until its geometry changes).
    #[serde(default)]
    pub measure_error: Option<String>,
    /// Run 6: imported bodies changed only by their import and direct edits:
    /// per current B-rep face (display order), its semantic reference and
    /// kind, for direct editing (ADR 0010). Empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imported_faces: Vec<ImportedFace>,
    /// Run 6: physical material (None: not assigned).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<rcad_document::Material>,
    /// Run 6: mass in g = density × the measured volume (None without a
    /// material or a volume). Labelled like the volume (exact or estimate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass: Option<f64>,
}

/// One face of [`BodySummary::imported_faces`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedFace {
    pub face: FaceRef,
    pub kind: k::DirectFace,
}

/// Inspectable per-feature state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureSummary {
    pub id: FeatureId,
    pub name: String,
    pub kind: String,
    pub parameters: Vec<(String, f64)>,
    pub depends_on: Vec<FeatureId>,
    pub plane: Option<SketchPlane>,
    pub solve_status: Option<SolveStatus>,
    pub solve_dof: Option<usize>,
    pub solve_residual: Option<f64>,
    pub frame: Option<Frame>,
    /// Sketches: labels of conflicting/redundant constraints from the last solve.
    #[serde(default)]
    pub solve_conflicts: Vec<String>,
    /// Sketches: solved geometry (for display and as the editor's seed).
    #[serde(default)]
    pub sketch: Option<Sketch>,
    /// Sketches: why this is not a usable closed profile, if it is not.
    #[serde(default)]
    pub profile_error: Option<String>,
    /// Extrusions/imports: semantic faces that can host a sketch, with their frames.
    #[serde(default)]
    pub faces: Vec<FaceSummary>,
    /// Extrusions: operation, extent, direction.
    #[serde(default)]
    pub operation: Option<Operation>,
    #[serde(default)]
    pub extent: Option<Extent>,
    #[serde(default)]
    pub reverse: bool,
    /// Extrusions: the picked sketch region (None: the whole profile).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<rcad_sketch::RegionRef>,
    /// Body created or modified by this feature.
    #[serde(default)]
    pub body: Option<BodyId>,
    /// Bodies created by this feature.
    #[serde(default)]
    pub creates: Vec<BodyId>,
    /// Imports: units, counts, warnings.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Parameters bound to expressions (name -> expression).
    #[serde(default)]
    pub expressions: std::collections::BTreeMap<String, String>,
    /// Suppressed: in history, not evaluated (run 5).
    #[serde(default)]
    pub suppressed: bool,
    /// After the timeline marker: in history, not evaluated (run 6).
    #[serde(default)]
    pub rolled_back: bool,
    /// Sketches: sources of their associative projections (run 6), in link
    /// order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projections: Vec<rcad_document::ProjectSource>,
}

/// A named user parameter with its evaluated value and relationships.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterSummary {
    pub name: String,
    pub expression: String,
    pub unit: rcad_document::ParamUnit,
    #[serde(default)]
    pub comment: String,
    /// Value in `unit` (None if the parameter set does not evaluate).
    pub value: Option<f64>,
    /// Parameters this expression refers to.
    pub depends_on: Vec<String>,
    /// Parameters and feature slots ("F2 Base.distance") that use it.
    pub used_by: Vec<String>,
}

/// A semantically referenceable face and its current placement frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceSummary {
    pub reference: FaceRef,
    pub frame: Frame,
    #[serde(default)]
    pub body: BodyId,
    /// When the plane holds several faces of the body: a point inside this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<[f64; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSummary {
    /// Identity of the open document; changes on New/Open/Close so clients can
    /// discard state that belongs to a previous document.
    #[serde(default)]
    pub doc_id: u64,
    pub open: bool,
    /// The body's measurements were deferred and are not available yet
    /// (interactive clients only; see `Session::set_defer_measurements`).
    #[serde(default)]
    pub measuring: bool,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub revision: u64,
    pub can_undo: Option<String>,
    pub can_redo: Option<String>,
    pub features: Vec<FeatureSummary>,
    /// Bodies in creation order.
    #[serde(default)]
    pub bodies: Vec<BodySummary>,
    /// Named user parameters in display order.
    #[serde(default)]
    pub parameters: Vec<ParameterSummary>,
    /// Components, occurrences, joints and DOF (run 6, docs/ASSEMBLIES.md).
    #[serde(default)]
    pub assembly: assembly::AssemblySummary,
    /// Named configurations (run 6) and the active one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configurations: Vec<rcad_document::Configuration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_configuration: Option<String>,
    /// Drawing sheets (run 6; docs/DRAWINGS.md).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drawings: Vec<rcad_document::drawing::Drawing>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Response {
    pub ok: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<FeatureId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurements: Option<Measurements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh_check: Option<MeshCheck>,
    /// Print exports: per-body/file report (docs/PRINT_EXPORT.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<print::ExportReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<DocSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DocError>,
    /// Assembly commands: created ids, BOM, interference (run 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly: Option<assembly::AssemblyOut>,
    /// `measure_faces`: exact face geometry and pair measurement (run 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_measure: Option<measure_faces::FaceMeasureReport>,
    /// `mass_properties`: per-body and total mass properties (run 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass_properties: Option<mass::MassReport>,
    /// `add_sketch_text`: the new text item's id (run 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<u32>,
    /// Drawing commands (run 6): the drawing and view acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drawing: Option<rcad_document::drawing::DrawingId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<rcad_document::drawing::ViewId>,
    /// `export_drawing`: per-view report and issues (run 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drawing_report: Option<drawings::DrawingReport>,
    /// Set by a queueing front end (the app engine) when this command was not
    /// executed because a newer edit of the same parameter was already queued
    /// behind it. `ok` is true and nothing changed. [`Session`] never sets it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub superseded: bool,
    /// Wall time (ms) the front end spent on this command, including any work
    /// it did after [`Session::execute`] (e.g. display tessellation). [`Session`]
    /// leaves it `None`; profiling only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
}

/// `(doc_id, revision, [(body, deep copy)])` awaiting measurement.
pub type PendingMeasurement = (u64, u64, Vec<(BodyId, rcad_kernel::Body)>);

/// An application session: at most one open document.
pub struct Session {
    model: Option<Model>,
    path: Option<PathBuf>,
    /// Bumped whenever the open document is replaced or closed.
    doc_id: u64,
    /// Skip B-rep measurement during edits (see `set_defer_measurements`).
    defer_measurements: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            model: None,
            path: None,
            doc_id: 0,
            defer_measurements: false,
        }
    }

    pub fn model(&self) -> Option<&Model> {
        self.model.as_ref()
    }

    /// Interactive clients: skip the expensive B-rep measurements while editing
    /// and supply them later with [`Session::adopt_measurements`]. Responses then
    /// carry `measurements: None` and `summary.measuring: true` until adopted.
    /// `save` and `measure` always measure synchronously first. Off by default
    /// (CLI, tests and the acceptance runner get measurements in every response).
    pub fn set_defer_measurements(&mut self, on: bool) {
        self.defer_measurements = on;
        if let Some(m) = &mut self.model {
            m.set_defer_measurements(on);
        }
    }

    /// Bodies awaiting deferred measurement: `(doc_id, revision, bodies)`. The
    /// bodies are deep copies (Truck bodies share geometry behind mutexes, so a
    /// shallow clone measured on another thread races with this session's own use).
    /// A body that cannot be copied is marked failed (stable, shown in its
    /// summary) and the others are still returned.
    pub fn pending_measurement(&mut self) -> Option<PendingMeasurement> {
        let m = self.model.as_mut()?;
        if !m.measurements_pending() {
            return None;
        }
        let mut bodies = Vec::new();
        let mut failed = Vec::new();
        for (id, b) in m.evaluation().unmeasured() {
            match b.deep_copy() {
                Ok(c) => bodies.push((id, c)),
                Err(e) => {
                    failed.push((id, format!("could not copy the body for measurement: {e}")))
                }
            }
        }
        let revision = m.revision;
        if !failed.is_empty() {
            m.fail_body_measurements(revision, failed);
        }
        (!bodies.is_empty()).then_some((self.doc_id, revision, bodies))
    }

    /// Bodies whose deferred measurement failed (see
    /// `Model::fail_body_measurements`). True when it applied.
    pub fn fail_body_measurements(
        &mut self,
        doc_id: u64,
        revision: u64,
        failures: Vec<(BodyId, String)>,
    ) -> bool {
        doc_id == self.doc_id
            && self
                .model
                .as_mut()
                .is_some_and(|m| m.fail_body_measurements(revision, failures))
    }

    /// The deferred measurement of `revision` failed: stop reporting it as
    /// pending. Returns true when it applied to the current document revision.
    pub fn fail_measurements(&mut self, doc_id: u64, revision: u64) -> bool {
        doc_id == self.doc_id
            && self
                .model
                .as_mut()
                .is_some_and(|model| model.fail_measurements(revision))
    }

    /// Install out-of-band measurements if they still describe the current
    /// document revision. Returns true when adopted.
    pub fn adopt_measurements(
        &mut self,
        doc_id: u64,
        revision: u64,
        m: Vec<(BodyId, Measurements)>,
    ) -> bool {
        doc_id == self.doc_id
            && self
                .model
                .as_mut()
                .is_some_and(|model| model.adopt_measurements(revision, m))
    }

    fn fresh_model_settings(&mut self) {
        let defer = self.defer_measurements;
        if let Some(m) = &mut self.model {
            m.set_defer_measurements(defer);
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn model_mut(&mut self) -> Result<&mut Model, DocError> {
        self.model
            .as_mut()
            .ok_or(DocError::Validation("no document is open".into()))
    }

    /// Execute a command. Never panics on user input; failures leave state unchanged.
    pub fn execute(&mut self, cmd: &Command) -> Response {
        let mut r = Response {
            command: cmd.name().into(),
            ..Default::default()
        };
        match self.run(cmd, &mut r) {
            Ok(()) => r.ok = true,
            Err(e) => {
                r.ok = false;
                r.error = Some(e);
            }
        }
        if r.ok
            && !matches!(
                cmd,
                Command::Inspect | Command::Measure | Command::ExportStl { .. }
            )
        {
            r.measurements = self
                .model
                .as_ref()
                .and_then(|m| m.evaluation().measurements.clone());
        }
        r
    }

    fn run(&mut self, cmd: &Command, r: &mut Response) -> Result<(), DocError> {
        if self.run_drawing(cmd, r)? {
            return Ok(());
        }
        match cmd {
            Command::NewDocument => {
                self.model = Some(Model::new());
                self.path = None;
                self.doc_id += 1;
                self.fresh_model_settings();
            }
            Command::CreateSketch {
                name,
                plane,
                template,
            } => {
                let v = |r: Result<Sketch, rcad_sketch::SketchError>| {
                    r.map_err(|e| DocError::Validation(e.to_string()))
                };
                let (sketch, center) = match template {
                    SketchTemplate::CenteredRectangle {
                        width,
                        height,
                        center,
                    } => (v(Sketch::centered_rectangle(*width, *height))?, *center),
                    SketchTemplate::CenteredCircle { diameter, center } => {
                        (v(Sketch::centered_circle(*diameter))?, *center)
                    }
                    SketchTemplate::Polygon { vertices } => {
                        (solved(v(polygon_sketch(vertices))?)?, None)
                    }
                    SketchTemplate::Custom { sketch } => (solved(sketch.clone())?, None),
                };
                let mut sketch = sketch;
                if let Some([cx, cy]) = center {
                    if !(cx.is_finite() && cy.is_finite()) {
                        return Err(DocError::Validation(format!(
                            "center must be finite, got [{cx}, {cy}]"
                        )));
                    }
                    for c in &mut sketch.constraints {
                        if let rcad_sketch::Constraint::FixedPoint { x, y, .. } = c {
                            (*x, *y) = (cx, cy);
                        }
                    }
                }
                let (name, plane) = (name.clone(), *plane);
                let id = self.model_mut()?.transact("Create sketch", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Sketch"));
                    Ok(d.push(
                        n,
                        FeatureKind::Sketch {
                            plane,
                            sketch,
                            links: Vec::new(),
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Extrude {
                name,
                sketch,
                distance,
                op,
                target,
                through_all,
                reverse,
                body_name,
                region,
                options,
            } => {
                let (name, sketch, distance, reverse) =
                    (name.clone(), *sketch, *distance, *reverse);
                let region = region.clone();
                let options = options.clone();
                let extent = if *through_all {
                    if *op != ExtrudeOp::Cut {
                        return Err(DocError::Validation(
                            "'through all' is only available for Cut".into(),
                        ));
                    }
                    Extent::ThroughAll
                } else {
                    // Up to a plane: the plane gives the length.
                    if options.to.is_none() && (!distance.is_finite() || distance <= 0.0) {
                        return Err(DocError::Validation(format!(
                            "extrusion distance must be > 0 mm, got {distance}"
                        )));
                    }
                    Extent::Distance
                };
                let m = self.model_mut()?;
                let target = match op {
                    ExtrudeOp::NewBody => None,
                    _ => Some(match target {
                        Some(t) => *t,
                        None => default_target(m, sketch)?,
                    }),
                };
                let body_name = body_name.clone();
                let id = m.transact("Extrude", |d| {
                    let operation = match (op, target) {
                        (ExtrudeOp::Join, Some(target)) => Operation::Join { target },
                        (ExtrudeOp::Cut, Some(target)) => Operation::Cut { target },
                        (ExtrudeOp::Intersect, Some(target)) => Operation::Intersect { target },
                        _ => {
                            let n = match &body_name {
                                Some(n) => valid_name(n, "body")?,
                                None => d.default_body_name(),
                            };
                            if d.bodies.iter().any(|b| b.name == n) {
                                return Err(DocError::Validation(format!(
                                    "another body is already named '{n}'"
                                )));
                            }
                            Operation::NewBody {
                                body: d.add_body(n),
                            }
                        }
                    };
                    let n = name.unwrap_or_else(|| d.default_name("Extrude"));
                    Ok(d.push(
                        n,
                        FeatureKind::Extrude {
                            sketch,
                            distance: if extent == Extent::ThroughAll {
                                0.0
                            } else {
                                distance
                            },
                            operation,
                            extent,
                            reverse,
                            region,
                            options,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Revolve {
                name,
                sketch,
                angle_deg,
                axis,
                reverse,
                op,
                target,
            } => {
                let (name, sketch, a, axis, reverse) =
                    (name.clone(), *sketch, *angle_deg, *axis, *reverse);
                let merge = match (op, target) {
                    (ExtrudeOp::NewBody, _) => None,
                    (_, None) => {
                        return Err(DocError::Validation(
                            "a revolve Join/Cut needs a target body".into(),
                        ));
                    }
                    (ExtrudeOp::Join, Some(t)) => Some(Operation::Join { target: *t }),
                    (ExtrudeOp::Cut, Some(t)) => Some(Operation::Cut { target: *t }),
                    (ExtrudeOp::Intersect, Some(t)) => Some(Operation::Intersect { target: *t }),
                };
                if !a.is_finite() || a <= 0.0 || a > 360.0 {
                    return Err(DocError::Validation(format!(
                        "revolve angle must be in (0, 360] degrees, got {a}"
                    )));
                }
                let id = self.model_mut()?.transact("Revolve", |d| {
                    let body = if merge.is_some() {
                        BodyId(0)
                    } else {
                        d.add_body(d.default_body_name())
                    };
                    let n = name.unwrap_or_else(|| d.default_name("Revolve"));
                    Ok(d.push(
                        n,
                        FeatureKind::Revolve {
                            sketch,
                            angle: a.to_radians(),
                            body,
                            axis,
                            reverse,
                            merge,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Thread {
                name,
                plane,
                center,
                diameter,
                pitch,
                length,
                left_hand,
                chord_tol,
                nut_width,
                clearance,
                body_name,
            } => {
                let (name, body_name) = (name.clone(), body_name.clone());
                let kind = |body| FeatureKind::Thread {
                    plane: *plane,
                    center: *center,
                    diameter: *diameter,
                    pitch: *pitch,
                    length: *length,
                    left_hand: *left_hand,
                    chord_tol: *chord_tol,
                    nut: nut_width.map(|width| rcad_document::NutSpec {
                        width,
                        clearance: *clearance,
                    }),
                    body,
                };
                let what = if nut_width.is_some() { "Nut" } else { "Thread" };
                let id = self.model_mut()?.transact(what, |d| {
                    let body = d.add_body(body_name.unwrap_or_else(|| d.default_body_name()));
                    let n = name.unwrap_or_else(|| d.default_name(what));
                    Ok(d.push(n, kind(body)))
                })?;
                r.feature = Some(id);
            }
            Command::Loft {
                name,
                a,
                b,
                via,
                body_name,
            } => {
                let (name, a, b, body_name) = (name.clone(), *a, *b, body_name.clone());
                let via = via.clone();
                if a == b {
                    return Err(DocError::Validation(
                        "loft needs two different sketches".into(),
                    ));
                }
                let id = self.model_mut()?.transact("Loft", |d| {
                    let body = d.add_body(body_name.unwrap_or_else(|| d.default_body_name()));
                    let n = name.unwrap_or_else(|| d.default_name("Loft"));
                    Ok(d.push(n, FeatureKind::Loft { a, b, body, via }))
                })?;
                r.feature = Some(id);
            }
            Command::Sweep {
                name,
                profile,
                path,
                body_name,
            } => {
                let (name, profile, path, body_name) =
                    (name.clone(), *profile, *path, body_name.clone());
                if profile == path {
                    return Err(DocError::Validation(
                        "sweep needs a profile sketch and a separate path sketch".into(),
                    ));
                }
                let id = self.model_mut()?.transact("Sweep", |d| {
                    let body = d.add_body(body_name.unwrap_or_else(|| d.default_body_name()));
                    let n = name.unwrap_or_else(|| d.default_name("Sweep"));
                    Ok(d.push(
                        n,
                        FeatureKind::Sweep {
                            profile,
                            path,
                            body,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::CutThroughAll { name, sketch } => {
                let (name, sketch) = (name.clone(), *sketch);
                let m = self.model_mut()?;
                let target = default_target(m, sketch)?;
                let id = m.transact("Cut", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Cut"));
                    Ok(d.push(
                        n,
                        FeatureKind::Cut {
                            sketch,
                            extent: rcad_document::CutExtent::ThroughAll,
                            target,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Pocket {
                name,
                sketch,
                depth,
            } => {
                let (name, sketch, depth) = (name.clone(), *sketch, *depth);
                if !depth.is_finite() || depth <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "pocket depth must be > 0 mm, got {depth}"
                    )));
                }
                let m = self.model_mut()?;
                let target = default_target(m, sketch)?;
                let id = m.transact("Pocket", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Pocket"));
                    Ok(d.push(
                        n,
                        FeatureKind::Pocket {
                            sketch,
                            depth,
                            target,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::LinearPattern {
                name,
                feature,
                count,
                spacing,
                direction,
            } => {
                let (name, seed, count, spacing, direction) =
                    (name.clone(), *feature, *count, *spacing, *direction);
                let id = self.model_mut()?.transact("Pattern", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Pattern"));
                    Ok(d.push(
                        n,
                        FeatureKind::LinearPattern {
                            seed,
                            count,
                            spacing,
                            direction,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::CircularPattern {
                name,
                feature,
                count,
                angle_deg,
                center,
            } => {
                let (name, seed, count, a, center) =
                    (name.clone(), *feature, *count, *angle_deg, *center);
                if !a.is_finite() || a <= 0.0 || a >= 360.0 {
                    return Err(DocError::Validation(format!(
                        "angle between instances must be in (0, 360) degrees, got {a}"
                    )));
                }
                let id = self.model_mut()?.transact("Circular pattern", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Circular pattern"));
                    Ok(d.push(
                        n,
                        FeatureKind::CircularPattern {
                            seed,
                            count,
                            angle: a.to_radians(),
                            center,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Mirror {
                name,
                feature,
                plane,
            } => {
                let (name, seed, plane) = (name.clone(), *feature, *plane);
                let id = self.model_mut()?.transact("Mirror", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Mirror"));
                    Ok(d.push(n, FeatureKind::Mirror { seed, plane }))
                })?;
                r.feature = Some(id);
            }
            Command::Chamfer {
                name,
                a,
                b,
                distance,
            } => {
                let (name, a, b, distance) = (name.clone(), *a, *b, *distance);
                if !distance.is_finite() || distance <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "chamfer distance must be > 0 mm, got {distance}"
                    )));
                }
                let id = self.model_mut()?.transact("Chamfer", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Chamfer"));
                    Ok(d.push(n, FeatureKind::Chamfer { a, b, distance }))
                })?;
                r.feature = Some(id);
            }
            Command::Fillet { name, a, b, radius } => {
                let (name, a, b, radius) = (name.clone(), *a, *b, *radius);
                if !radius.is_finite() || radius <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "fillet radius must be > 0 mm, got {radius}"
                    )));
                }
                let id = self.model_mut()?.transact("Fillet", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Fillet"));
                    Ok(d.push(n, FeatureKind::Fillet { a, b, radius }))
                })?;
                r.feature = Some(id);
            }
            Command::DirectEdit { name, face, op } => {
                let (name, face, op) = (name.clone(), *face, *op);
                if !matches!(face.role, FaceRole::Imported { .. }) {
                    return Err(DocError::Validation(
                        "a direct edit needs a face of an imported (STEP) body".into(),
                    ));
                }
                let id = self.model_mut()?.transact("Direct edit", |d| {
                    let n = name.unwrap_or_else(|| {
                        d.default_name(match op {
                            rcad_document::DirectOp::OffsetFace { .. } => "Offset face",
                            rcad_document::DirectOp::ResizeCylinder { .. } => "Resize hole",
                            rcad_document::DirectOp::RemoveCylinder => "Remove hole",
                            rcad_document::DirectOp::ReplaceFace { .. } => "Tilt face",
                        })
                    });
                    Ok(d.push(n, FeatureKind::DirectEdit { face, op }))
                })?;
                r.feature = Some(id);
            }
            Command::Countersink {
                name,
                hole,
                diameter,
                angle_deg,
            } => {
                let (name, hole, diameter, angle_deg) =
                    (name.clone(), *hole, *diameter, *angle_deg);
                if !(diameter.is_finite() && diameter > 0.0) {
                    return Err(DocError::Validation(format!(
                        "countersink diameter must be > 0 mm, got {diameter}"
                    )));
                }
                if !(angle_deg > 0.0 && angle_deg < 180.0) {
                    return Err(DocError::Validation(format!(
                        "countersink angle must be between 0 and 180 degrees, got {angle_deg}"
                    )));
                }
                let id = self.model_mut()?.transact("Countersink", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Countersink"));
                    Ok(d.push(
                        n,
                        FeatureKind::Countersink {
                            hole,
                            diameter,
                            angle: angle_deg.to_radians(),
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::ImportedFaces { body } => {
                let m = self
                    .model
                    .as_ref()
                    .ok_or(DocError::Validation("no document is open".into()))?;
                r.message = Some(imported_faces_json(m.evaluation(), *body)?);
            }
            Command::SetParameter {
                feature,
                name,
                value,
            } => {
                let c = self.model_mut()?.set_parameter(*feature, name, *value)?;
                r.feature = Some(*feature);
                r.message = Some(format!("{}.{}: {} -> {}", c.feature, c.name, c.old, c.new));
            }
            Command::Hole {
                name,
                plane,
                center,
                diameter,
                depth,
                counterbore,
                target,
            } => {
                let (name, plane, center, diameter, depth) =
                    (name.clone(), *plane, *center, *diameter, *depth);
                let counterbore = counterbore.map(|[d, h]| rcad_document::Counterbore {
                    diameter: d,
                    depth: h,
                });
                let m = self.model_mut()?;
                let target = match target {
                    Some(t) => *t,
                    None => {
                        // A face, or a construction plane offset from one: the
                        // face's body (run 5).
                        let r = match plane {
                            SketchPlane::Face(r)
                            | SketchPlane::Offset {
                                base: rcad_document::PlaneBase::Face(r),
                                ..
                            } => r,
                            _ => {
                                return Err(DocError::Validation(
                                    "choose a target body for a hole on a world plane".into(),
                                ));
                            }
                        };
                        m.evaluation()
                            .faces
                            .iter()
                            .find(|c| c.reference == r)
                            .map(|c| c.body)
                            .ok_or_else(|| DocError::BrokenReference {
                                from: FeatureId(0),
                                detail: "the hole's face does not resolve to one face".into(),
                            })?
                    }
                };
                let id = m.transact("Hole", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Hole"));
                    Ok(d.push(
                        n,
                        FeatureKind::Hole {
                            plane,
                            center,
                            diameter,
                            depth,
                            counterbore,
                            target,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Shell {
                name,
                extrude,
                open,
                thickness,
            } => {
                let (name, extrude, open, thickness) = (name.clone(), *extrude, *open, *thickness);
                if !thickness.is_finite() || thickness <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "shell thickness must be > 0 mm, got {thickness}"
                    )));
                }
                let id = self.model_mut()?.transact("Shell", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Shell"));
                    Ok(d.push(
                        n,
                        FeatureKind::Shell {
                            extrude,
                            open,
                            thickness,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::SetUserParameter {
                name,
                expression,
                unit,
                comment,
            } => {
                use rcad_document::params;
                let name = name.trim().to_string();
                params::valid_name(&name).map_err(|e| DocError::Expression {
                    name: name.clone(),
                    detail: e,
                })?;
                params::check_syntax(expression).map_err(|e| DocError::Expression {
                    name: name.clone(),
                    detail: format!("'{expression}': {e}"),
                })?;
                let (expression, unit, comment) =
                    (expression.trim().to_string(), *unit, comment.clone());
                let label = format!("Set parameter {name} = {expression}");
                let cause = |e: DocError| match e {
                    DocError::Expression { name: n, detail } if n != name => DocError::Expression {
                        name: n,
                        detail: format!("{detail} (caused by setting {name} = {expression})"),
                    },
                    other => other,
                };
                self.model_mut()?
                    .transact(&label, |d| {
                        match d.parameters.iter_mut().find(|p| p.name == name) {
                            Some(p) => {
                                p.expression = expression.clone();
                                if let Some(u) = unit {
                                    p.unit = u;
                                }
                                if let Some(c) = comment {
                                    p.comment = c;
                                }
                            }
                            None => d.parameters.push(rcad_document::Parameter {
                                name: name.clone(),
                                expression: expression.clone(),
                                unit: unit.unwrap_or_default(),
                                comment: comment.unwrap_or_default(),
                            }),
                        }
                        Ok(())
                    })
                    .map_err(cause)?;
                let v = self
                    .model
                    .as_ref()
                    .and_then(|m| params::evaluate_parameters(m.document()).ok())
                    .and_then(|e| e.get(&name).map(|q| q.value));
                r.message = Some(match v {
                    Some(v) => format!("{name} = {expression} = {v}"),
                    None => format!("{name} = {expression}"),
                });
            }
            Command::SetUserParameters { parameters } => {
                use rcad_document::params;
                if parameters.is_empty() {
                    return Err(DocError::Validation("no parameters given".into()));
                }
                for p in parameters {
                    params::valid_name(p.name.trim()).map_err(|e| DocError::Expression {
                        name: p.name.clone(),
                        detail: e,
                    })?;
                    params::check_syntax(&p.expression).map_err(|e| DocError::Expression {
                        name: p.name.clone(),
                        detail: format!("'{}': {e}", p.expression),
                    })?;
                }
                let defs = parameters.clone();
                let label = format!(
                    "Set {}",
                    defs.iter()
                        .map(|p| format!("{} = {}", p.name.trim(), p.expression.trim()))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let label: String = label.chars().take(200).collect();
                self.model_mut()?.transact(&label, |d| {
                    for p in &defs {
                        let name = p.name.trim().to_string();
                        let expression = p.expression.trim().to_string();
                        match d.parameters.iter_mut().find(|x| x.name == name) {
                            Some(x) => {
                                x.expression = expression;
                                if let Some(u) = p.unit {
                                    x.unit = u;
                                }
                                if let Some(c) = &p.comment {
                                    x.comment = c.clone();
                                }
                            }
                            None => d.parameters.push(rcad_document::Parameter {
                                name,
                                expression,
                                unit: p.unit.unwrap_or_default(),
                                comment: p.comment.clone().unwrap_or_default(),
                            }),
                        }
                    }
                    Ok(())
                })?;
                r.message = Some(format!("set {} parameter(s) in one step", defs.len()));
            }
            Command::RenameUserParameter { name, new_name } => {
                use rcad_document::params;
                let (old, new) = (name.clone(), new_name.trim().to_string());
                params::valid_name(&new).map_err(|e| DocError::Expression {
                    name: new.clone(),
                    detail: e,
                })?;
                let m = self.model_mut()?;
                if !m.document().parameters.iter().any(|p| p.name == old) {
                    return Err(DocError::Expression {
                        name: old,
                        detail: "no such parameter".into(),
                    });
                }
                if old != new && m.document().parameters.iter().any(|p| p.name == new) {
                    return Err(DocError::Expression {
                        name: new,
                        detail: "another parameter already has this name".into(),
                    });
                }
                let users = params::users_of(m.document(), &old);
                m.transact(&format!("Rename parameter {old} to {new}"), |d| {
                    for p in &mut d.parameters {
                        if p.name == old {
                            p.name = new.clone();
                        }
                        p.expression = params::rename_in(&p.expression, &old, &new);
                    }
                    for f in &mut d.features {
                        for e in f.expressions.values_mut() {
                            *e = params::rename_in(e, &old, &new);
                        }
                    }
                    Ok(())
                })?;
                r.message = Some(format!(
                    "renamed {old} to {new}; updated {} reference(s)",
                    users.len()
                ));
            }
            Command::DeleteUserParameter { name } => {
                use rcad_document::params;
                let name = name.clone();
                let m = self.model_mut()?;
                if !m.document().parameters.iter().any(|p| p.name == name) {
                    return Err(DocError::Expression {
                        name,
                        detail: "no such parameter".into(),
                    });
                }
                let users = params::users_of(m.document(), &name);
                if !users.is_empty() {
                    return Err(DocError::Expression {
                        name,
                        detail: format!(
                            "still used by {}; change those expressions first",
                            users.join(", ")
                        ),
                    });
                }
                m.transact(&format!("Delete parameter {name}"), |d| {
                    d.parameters.retain(|p| p.name != name);
                    Ok(())
                })?;
            }
            Command::SetExpression {
                feature,
                name,
                expression,
            } => {
                use rcad_document::params;
                let (feature, slot, src) = (*feature, name.clone(), expression.trim().to_string());
                let m = self.model_mut()?;
                let f = m.document().feature(feature)?;
                if params::slot_kind(&f.kind, &slot).is_none() {
                    return Err(DocError::UnknownParameter {
                        feature,
                        name: slot,
                    });
                }
                let label = format!("{}.{slot}", f.name);
                if !src.is_empty() && params::plain_number(&src).is_none() {
                    params::check_syntax(&src).map_err(|e| DocError::Expression {
                        name: label.clone(),
                        detail: format!("'{src}': {e}"),
                    })?;
                }
                let title = if src.is_empty() {
                    format!("Unbind {label}")
                } else {
                    format!("Set {label} = {src}")
                };
                m.transact(&title, |d| {
                    let f = d.feature_mut(feature)?;
                    if src.is_empty() {
                        f.expressions.remove(&slot);
                    } else if let Some(v) = params::plain_number(&src) {
                        f.expressions.remove(&slot);
                        f.set_parameter(&slot, v)?;
                    } else {
                        f.expressions.insert(slot.clone(), src.clone());
                    }
                    Ok(())
                })?;
                let v = self
                    .model
                    .as_ref()
                    .and_then(|m| m.document().feature(feature).ok())
                    .and_then(|f| f.parameters().into_iter().find(|(n, _)| *n == slot))
                    .map(|(_, v)| v);
                r.feature = Some(feature);
                r.message = Some(format!(
                    "{label} = {}{}",
                    if src.is_empty() { "(unbound)" } else { &src },
                    v.map(|v| format!(" = {v}")).unwrap_or_default()
                ));
            }
            Command::AddSketchText { feature, text } => {
                let mut s = self.stored_sketch(*feature)?;
                let id = s.add_text(text.clone()).map_err(sketch_err)?;
                self.run(
                    &Command::EditSketch {
                        feature: *feature,
                        sketch: s,
                    },
                    r,
                )?;
                r.text = Some(id);
            }
            Command::EditSketchText { feature, id, text } => {
                let mut s = self.stored_sketch(*feature)?;
                s.edit_text(*id, text.clone()).map_err(sketch_err)?;
                self.run(
                    &Command::EditSketch {
                        feature: *feature,
                        sketch: s,
                    },
                    r,
                )?;
                r.text = Some(*id);
            }
            Command::RemoveSketchText { feature, id } => {
                let mut s = self.stored_sketch(*feature)?;
                if !s.remove_text(*id) {
                    return Err(DocError::Validation(format!(
                        "sketch {feature} has no text {id}"
                    )));
                }
                self.run(
                    &Command::EditSketch {
                        feature: *feature,
                        sketch: s,
                    },
                    r,
                )?;
            }
            Command::EditSketch { feature, sketch } => {
                let (feature, new) = (*feature, solved(sketch.clone())?);
                let m = self.model_mut()?;
                let name = m.document().feature(feature)?.name.clone();
                // A dimension bound to an expression keeps its expression's
                // value: an edit that changes it would be silently reverted, so
                // it is refused with the expression named (review run 4 #16).
                {
                    let f = m.document().feature(feature)?;
                    if let FeatureKind::Sketch { sketch: old, .. } = &f.kind {
                        let old_dims = old.dimensions();
                        for (n, v) in new.dimensions() {
                            let changed = old_dims
                                .iter()
                                .find(|(o, _)| *o == n)
                                .is_some_and(|(_, ov)| (ov - v).abs() > 1e-9 * ov.abs().max(1.0));
                            if changed && let Some(e) = f.expressions.get(&n) {
                                return Err(DocError::Validation(format!(
                                    "dimension '{n}' of {name} is bound to the expression '{e}'; \
                                     change the expression or its parameters, or unbind it in the \
                                     inspector (type a number) before editing it in the sketch"
                                )));
                            }
                        }
                    }
                }
                let dropped = m.transact(&format!("Edit sketch {name}"), |d| {
                    let f = d.feature_mut(feature)?;
                    match &mut f.kind {
                        FeatureKind::Sketch { sketch, .. } => {
                            *sketch = keep_entity_ids(sketch, new)?;
                        }
                        _ => {
                            return Err(DocError::Validation(format!("{feature} is not a sketch")));
                        }
                    }
                    // A dimension removed by the edit takes its expression
                    // binding with it (reported, never silently re-bound).
                    let names: Vec<String> = f.parameters().into_iter().map(|(n, _)| n).collect();
                    let gone: Vec<(String, String)> = f
                        .expressions
                        .iter()
                        .filter(|(k, _)| !names.contains(k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    for (k, _) in &gone {
                        f.expressions.remove(k);
                    }
                    Ok(gone)
                })?;
                r.feature = Some(feature);
                if !dropped.is_empty() {
                    r.message = Some(format!(
                        "removed the expression binding of deleted dimension(s): {}",
                        dropped
                            .iter()
                            .map(|(k, v)| format!("{k} = {v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            Command::DeleteFeature { feature } => {
                let feature = *feature;
                let m = self.model_mut()?;
                let doc = m.document();
                let name = doc.feature(feature)?.name.clone();
                // Explicit references plus Join/Cut targets of bodies it creates.
                let dependents: Vec<String> = doc
                    .dependents_of(feature)
                    .into_iter()
                    .filter_map(|id| doc.feature(id).ok())
                    .map(|f| format!("{} {}", f.id, f.name))
                    .collect();
                if !dependents.is_empty() {
                    return Err(DocError::Validation(format!(
                        "{name} is used by {}; delete those first",
                        dependents.join(", ")
                    )));
                }
                m.transact(&format!("Delete {name}"), |d| {
                    // The marker on the deleted feature moves to its predecessor.
                    if d.rollback == Some(feature) {
                        let p = d.features.iter().position(|f| f.id == feature);
                        d.rollback = p.and_then(|p| p.checked_sub(1)).map(|q| d.features[q].id);
                        if d.rollback.is_none() {
                            return Err(DocError::Validation(
                                "the first feature holds the timeline marker; roll to the end \
                                 before deleting it"
                                    .into(),
                            ));
                        }
                    }
                    d.features.retain(|f| f.id != feature);
                    // Review run 6 #4: configurations stop controlling it.
                    for c in d.configurations.iter_mut() {
                        c.suppressed.remove(&feature);
                    }
                    d.prune_bodies();
                    d.prune_blobs();
                    Ok(())
                })?;
                r.message = Some(format!("deleted {feature} {name}"));
            }
            Command::RenameBody { body, name } => {
                let (body, name) = (*body, valid_name(name, "body")?);
                let m = self.model_mut()?;
                if m.document()
                    .bodies
                    .iter()
                    .any(|b| b.id != body && b.name == name)
                {
                    return Err(DocError::Validation(format!(
                        "another body is already named '{name}'"
                    )));
                }
                let old = m.document().body(body)?.name.clone();
                m.transact(&format!("Rename {old} to {name}"), |d| {
                    d.body_mut(body)?.name = name.clone();
                    Ok(())
                })?;
            }
            Command::SetSuppressed {
                feature,
                suppressed,
                with_dependents,
            } => {
                let (feature, suppressed, cascade) = (*feature, *suppressed, *with_dependents);
                let m = self.model_mut()?;
                let doc = m.document();
                let name = doc.feature(feature)?.name.clone();
                let ids = if cascade {
                    doc.with_dependents(feature)
                } else {
                    vec![feature]
                };
                m.transact(
                    &format!(
                        "{} {name}{}",
                        if suppressed { "Suppress" } else { "Restore" },
                        if ids.len() > 1 {
                            format!(" (+{} dependent)", ids.len() - 1)
                        } else {
                            String::new()
                        }
                    ),
                    |d| {
                        for id in &ids {
                            d.feature_mut(*id)?.suppressed = suppressed;
                        }
                        Ok(())
                    },
                )?;
                r.feature = Some(feature);
                r.message = Some(format!(
                    "{} {}",
                    if suppressed { "suppressed" } else { "restored" },
                    ids.iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Command::Project {
                sketch,
                source,
                construction,
            } => {
                let (sk_id, source, construction) = (*sketch, *source, *construction);
                let m = self.model_mut()?;
                let curves = rcad_document::projection_curves(m.document(), sk_id, &source)?;
                if curves.iter().all(Vec::is_empty) {
                    return Err(DocError::Validation("nothing to project".into()));
                }
                let name = m.document().feature(sk_id)?.name.clone();
                let n = curves.iter().map(Vec::len).sum::<usize>();
                m.transact(&format!("Project into {name}"), |d| {
                    let FeatureKind::Sketch { sketch, links, .. } = &mut d.feature_mut(sk_id)?.kind
                    else {
                        return Err(DocError::Validation(format!("{sk_id} is not a sketch")));
                    };
                    let mut made: Vec<([f64; 2], rcad_sketch::PointId)> = Vec::new();
                    let mut point = |sk: &mut rcad_sketch::Sketch, p: [f64; 2]| {
                        if let Some((_, id)) = made
                            .iter()
                            .find(|(q, _)| (q[0] - p[0]).hypot(q[1] - p[1]) <= 1e-7)
                        {
                            return *id;
                        }
                        let id = sk.add_point(p[0], p[1]);
                        sk.constraints.push(rcad_sketch::Constraint::FixedPoint {
                            p: id,
                            x: p[0],
                            y: p[1],
                        });
                        made.push((p, id));
                        id
                    };
                    let mut loops = Vec::new();
                    for l in &curves {
                        let mut out = Vec::new();
                        for (c, from) in l {
                            let (entity, points) = match *c {
                                rcad_kernel::Curve2::Line { a, b } => {
                                    let (pa, pb) = (point(sketch, a), point(sketch, b));
                                    (sketch.add_line(pa, pb), vec![pa, pb])
                                }
                                rcad_kernel::Curve2::Arc { c, a, b, ccw } => {
                                    let (a, b) = if ccw { (a, b) } else { (b, a) };
                                    let (pa, pb) = (point(sketch, a), point(sketch, b));
                                    // The centre follows from the fixed ends and
                                    // the arc's equal-radius equation plus one
                                    // coordinate (the well-conditioned one: x
                                    // when the chord is steep, else y), so
                                    // nothing is pinned twice.
                                    let pc = sketch.add_point(c[0], c[1]);
                                    sketch.constraints.push(
                                        if (b[1] - a[1]).abs() >= (b[0] - a[0]).abs() {
                                            rcad_sketch::Constraint::FixedX { p: pc, x: c[0] }
                                        } else {
                                            rcad_sketch::Constraint::FixedY { p: pc, y: c[1] }
                                        },
                                    );
                                    let id = sketch.next_entity_id();
                                    sketch.entities.push(rcad_sketch::Entity::Arc {
                                        id,
                                        center: pc,
                                        start: pa,
                                        end: pb,
                                    });
                                    (id, vec![pc, pa, pb])
                                }
                                rcad_kernel::Curve2::Circle { c, r } => {
                                    let pc = point(sketch, c);
                                    let rim = point(sketch, [c[0] + r, c[1]]);
                                    let id = sketch.add_circle(pc, r);
                                    sketch.constraints.push(
                                        rcad_sketch::Constraint::PointOnCircle {
                                            point: rim,
                                            circle: id,
                                        },
                                    );
                                    (id, vec![pc, rim])
                                }
                            };
                            if construction {
                                sketch.construction.insert(entity);
                            }
                            out.push(rcad_document::LinkedCurve {
                                entity,
                                points,
                                from: *from,
                            });
                        }
                        loops.push(out);
                    }
                    links.push(rcad_document::ProjectionLink { source, loops });
                    Ok(())
                })?;
                r.feature = Some(sk_id);
                r.message = Some(format!("projected {n} curve(s) into {name}"));
            }
            Command::UnlinkProjection { sketch, link } => {
                let (sk_id, li) = (*sketch, *link);
                let m = self.model_mut()?;
                // Freeze at the last regenerated positions.
                let solved = m
                    .evaluation()
                    .features
                    .iter()
                    .find(|f| f.id == sk_id)
                    .and_then(|f| f.solved_sketch.clone());
                let name = m.document().feature(sk_id)?.name.clone();
                m.transact(&format!("Unlink projection {} of {name}", li + 1), |d| {
                    let FeatureKind::Sketch { sketch, links, .. } = &mut d.feature_mut(sk_id)?.kind
                    else {
                        return Err(DocError::Validation(format!("{sk_id} is not a sketch")));
                    };
                    if li >= links.len() {
                        return Err(DocError::Validation(format!(
                            "{name} has {} projection(s); no link {}",
                            links.len(),
                            li + 1
                        )));
                    }
                    let l = links.remove(li);
                    if let Some(sv) = &solved {
                        for q in l.loops.iter().flatten().flat_map(|c| c.points.iter()) {
                            let Some(p) = sv.points.iter().find(|p| p.id == *q) else {
                                continue;
                            };
                            for c in sketch.constraints.iter_mut() {
                                match c {
                                    rcad_sketch::Constraint::FixedPoint { p: r, x, y }
                                        if r == q =>
                                    {
                                        (*x, *y) = (p.x, p.y);
                                    }
                                    rcad_sketch::Constraint::FixedX { p: r, x } if r == q => {
                                        *x = p.x;
                                    }
                                    rcad_sketch::Constraint::FixedY { p: r, y } if r == q => {
                                        *y = p.y;
                                    }
                                    _ => {}
                                }
                            }
                            if let Some(sp) = sketch.points.iter_mut().find(|sp| sp.id == *q) {
                                (sp.x, sp.y) = (p.x, p.y);
                            }
                        }
                    }
                    Ok(())
                })?;
                r.feature = Some(sk_id);
            }
            Command::MoveBody {
                name,
                body,
                translate,
                origin,
                axis,
                angle_deg,
                copy,
                body_name,
            } => {
                let (name, body, translate, origin) = (name.clone(), *body, *translate, *origin);
                let axis = axis.unwrap_or([0.0, 0.0, 1.0]);
                if !angle_deg.is_finite() || angle_deg.abs() > 360.0 {
                    return Err(DocError::Validation(format!(
                        "the rotation must be within ±360°, got {angle_deg}"
                    )));
                }
                let angle = angle_deg.to_radians();
                // Validate before the transaction (a clear message).
                rcad_kernel::Rigid::new(translate, origin, axis, angle)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                let (copy, body_name) = (*copy, body_name.clone());
                let m = self.model_mut()?;
                let src = m.document().body(body)?.name.clone();
                let id = m.transact(
                    &format!("{} {src}", if copy { "Copy" } else { "Move" }),
                    |d| {
                        let copy = if copy {
                            let n = match &body_name {
                                Some(n) => valid_name(n, "body")?,
                                None => d.default_body_name(),
                            };
                            if d.bodies.iter().any(|b| b.name == n) {
                                return Err(DocError::Validation(format!(
                                    "another body is already named '{n}'"
                                )));
                            }
                            let mat = d.body(body)?.material.clone();
                            let id = d.add_body(n);
                            d.body_mut(id)?.material = mat;
                            Some(id)
                        } else {
                            None
                        };
                        let n = name.unwrap_or_else(|| {
                            d.default_name(if copy.is_some() { "Copy" } else { "Move" })
                        });
                        Ok(d.push(
                            n,
                            FeatureKind::MoveBody {
                                body,
                                translate,
                                origin,
                                axis,
                                angle,
                                copy,
                            },
                        ))
                    },
                )?;
                r.feature = Some(id);
            }
            Command::SetConfiguration {
                name,
                values,
                suppressed,
                part_number,
                excluded_occurrences,
            } => {
                let c = rcad_document::Configuration {
                    name: name.trim().to_string(),
                    values: values.clone(),
                    suppressed: suppressed.iter().copied().collect(),
                    part_number: part_number.trim().to_string(),
                    excluded_occurrences: excluded_occurrences.iter().copied().collect(),
                };
                let m = self.model_mut()?;
                check_configuration(m.document(), &c)?;
                let name = c.name.clone();
                m.transact(&format!("Configuration {name}"), |d| {
                    match d.configurations.iter_mut().find(|x| x.name == name) {
                        Some(x) => *x = c,
                        None => d.configurations.push(c),
                    }
                    if d.active_configuration.as_deref() == Some(&name) {
                        d.apply_configuration(&name)?;
                    }
                    Ok(())
                })?;
            }
            Command::SetConfigurationTable { configurations } => {
                let mut list = configurations.clone();
                let m = self.model_mut()?;
                for (i, c) in list.iter_mut().enumerate() {
                    c.name = c.name.trim().to_string();
                    c.part_number = c.part_number.trim().to_string();
                    check_configuration(m.document(), c)?;
                    if list_has_name(&configurations[..i], &c.name) {
                        return Err(DocError::Validation(format!(
                            "configuration '{}' appears twice in the table",
                            c.name
                        )));
                    }
                }
                m.transact("Edit configuration table", |d| {
                    // Features/occurrences no configuration controls any more
                    // return to unsuppressed (instead of keeping whatever the
                    // last activation left).
                    let old_f = d.configured_features();
                    let old_o: std::collections::BTreeSet<_> = d
                        .configurations
                        .iter()
                        .flat_map(|c| c.excluded_occurrences.iter().copied())
                        .collect();
                    d.configurations = list;
                    let new_f = d.configured_features();
                    for f in old_f.difference(&new_f) {
                        d.feature_mut(*f)?.suppressed = false;
                    }
                    let new_o: std::collections::BTreeSet<_> = d
                        .configurations
                        .iter()
                        .flat_map(|c| c.excluded_occurrences.iter().copied())
                        .collect();
                    for o in old_o.difference(&new_o) {
                        if let Some(x) = d.assembly.occurrences.iter_mut().find(|x| x.id == *o) {
                            x.suppressed = false;
                        }
                    }
                    match d.active_configuration.clone() {
                        Some(a) if d.configurations.iter().any(|c| c.name == a) => {
                            d.apply_configuration(&a)?;
                        }
                        Some(_) => d.active_configuration = None,
                        None => {}
                    }
                    Ok(())
                })?;
            }
            Command::SetOccurrenceSuppressed {
                occurrence,
                suppressed,
            } => {
                let (o, sup) = (*occurrence, *suppressed);
                self.model_mut()?.transact(
                    &format!("{} occurrence", if sup { "Suppress" } else { "Restore" }),
                    |d| {
                        let x = d
                            .assembly
                            .occurrences
                            .iter_mut()
                            .find(|x| x.id == o)
                            .ok_or_else(|| {
                                DocError::Validation(format!("unknown occurrence {o:?}"))
                            })?;
                        x.suppressed = sup;
                        Ok(())
                    },
                )?;
            }
            Command::DeleteConfiguration { name } => {
                let name = name.clone();
                let m = self.model_mut()?;
                if !m.document().configurations.iter().any(|c| c.name == name) {
                    return Err(DocError::Validation(format!(
                        "no configuration named '{name}'"
                    )));
                }
                m.transact(&format!("Delete configuration {name}"), |d| {
                    d.configurations.retain(|c| c.name != name);
                    if d.active_configuration.as_deref() == Some(&name) {
                        d.active_configuration = None;
                    }
                    Ok(())
                })?;
            }
            Command::ActivateConfiguration { name } => {
                let name = name.clone();
                self.model_mut()?
                    .transact(&format!("Activate configuration {name}"), |d| {
                        d.apply_configuration(&name)
                    })?;
                r.message = Some(format!("configuration {name} active"));
            }
            Command::ExportConfigurations {
                dir,
                stem,
                configurations,
                step,
                stl,
                threemf,
                overwrite,
            } => {
                let (dir, stem, overwrite) = (dir.clone(), stem.clone(), *overwrite);
                let formats: Vec<&str> = [(*step, "step"), (*stl, "stl"), (*threemf, "3mf")]
                    .into_iter()
                    .filter_map(|(on, e)| on.then_some(e))
                    .collect();
                if formats.is_empty() {
                    return Err(DocError::Validation(
                        "choose at least one format (step, stl, 3mf)".into(),
                    ));
                }
                let all: Vec<rcad_document::Configuration> =
                    self.model_mut()?.document().configurations.clone();
                let chosen: Vec<rcad_document::Configuration> = if configurations.is_empty() {
                    all
                } else {
                    configurations
                        .iter()
                        .map(|n| {
                            all.iter().find(|c| c.name == *n).cloned().ok_or_else(|| {
                                DocError::Validation(format!("no configuration named '{n}'"))
                            })
                        })
                        .collect::<Result<_, _>>()?
                };
                if chosen.is_empty() {
                    return Err(DocError::Validation(
                        "the document has no configurations".into(),
                    ));
                }
                let clean = |t: &str| -> String {
                    t.chars()
                        .map(|c| {
                            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect()
                };
                // Plan every path first: refuse before changing anything.
                let mut plan = Vec::new();
                for c in &chosen {
                    let mut base = format!("{}-{}", clean(&stem), clean(&c.name));
                    if !c.part_number.is_empty() {
                        base.push('-');
                        base.push_str(&clean(&c.part_number));
                    }
                    for e in &formats {
                        let p = dir.join(format!("{base}.{e}"));
                        if p.exists() && !overwrite {
                            return Err(DocError::File(format!(
                                "{} exists; pass overwrite to replace it",
                                p.display()
                            )));
                        }
                        plan.push((c.name.clone(), *e, p));
                    }
                }
                // Review run 6 #11: sanitized names must not collide.
                {
                    let mut seen = std::collections::BTreeSet::new();
                    for (n, _, p) in &plan {
                        if !seen.insert(p.clone()) {
                            return Err(DocError::Validation(format!(
                                "configuration '{n}' maps to {} like another one; rename one of them",
                                p.display()
                            )));
                        }
                    }
                }
                std::fs::create_dir_all(&dir)
                    .map_err(|e| DocError::File(format!("{}: {e}", dir.display())))?;
                // Review run 6 #14: put the document, its undo/redo history and
                // the dirty flag back exactly (the activations leave no trace).
                let snapshot = self.model_mut()?.snapshot();
                let mut written = Vec::new();
                let mut result = Ok(());
                for c in &chosen {
                    let a = self.run(
                        &Command::ActivateConfiguration {
                            name: c.name.clone(),
                        },
                        &mut Response::default(),
                    );
                    if let Err(e) = a {
                        result = Err(e);
                        break;
                    }
                    for (_, e, p) in plan.iter().filter(|(n, _, _)| *n == c.name) {
                        let cmd = match *e {
                            "step" => Command::ExportStep {
                                path: p.clone(),
                                bodies: None,
                            },
                            "stl" => Command::ExportStl {
                                path: p.clone(),
                                chord: None,
                                bodies: None,
                                print: Default::default(),
                            },
                            _ => Command::Export3mf {
                                path: p.clone(),
                                chord: None,
                                bodies: None,
                                unit: None,
                                print: Default::default(),
                            },
                        };
                        if let Err(x) = self.run(&cmd, &mut Response::default()) {
                            result = Err(x);
                            break;
                        }
                        written.push(p.display().to_string());
                    }
                    if result.is_err() {
                        break;
                    }
                }
                self.model_mut()?.restore(snapshot);
                result?;
                r.message = Some(format!("wrote {}", written.join(", ")));
            }
            Command::SheetMetal {
                name,
                sketch,
                thickness,
                radius,
                k_factor,
                width,
                flip,
                body_name,
            } => {
                let (name, sketch, body_name) = (name.clone(), *sketch, body_name.clone());
                let (thickness, radius, k_factor, width, flip) =
                    (*thickness, *radius, *k_factor, *width, *flip);
                let id = self.model_mut()?.transact("Sheet metal flange", |d| {
                    let n = match &body_name {
                        Some(n) => valid_name(n, "body")?,
                        None => d.default_body_name(),
                    };
                    if d.bodies.iter().any(|b| b.name == n) {
                        return Err(DocError::Validation(format!(
                            "another body is already named '{n}'"
                        )));
                    }
                    let body = d.add_body(n);
                    let n = name.unwrap_or_else(|| d.default_name("Flange"));
                    Ok(d.push(
                        n,
                        FeatureKind::SheetMetal {
                            sketch,
                            thickness,
                            radius,
                            k_factor,
                            width,
                            flip,
                            body,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::ExportFlatPattern { feature, path } => {
                let (feature, path) = (*feature, path.clone());
                let m = self.model_mut()?;
                let FeatureKind::SheetMetal {
                    sketch,
                    thickness,
                    radius,
                    k_factor,
                    width,
                    flip,
                    ..
                } = m.document().feature(feature)?.kind.clone()
                else {
                    return Err(DocError::Validation(format!(
                        "{feature} is not a sheet-metal feature"
                    )));
                };
                let solved = m
                    .evaluation()
                    .features
                    .iter()
                    .find(|f| f.id == sketch)
                    .and_then(|f| f.solved_sketch.clone())
                    .ok_or_else(|| {
                        DocError::Validation(format!("{sketch} is not an evaluated sketch"))
                    })?;
                let chain = rcad_document::sheet::line_chain(&solved)
                    .map_err(|e| DocError::Validation(format!("{feature}: {e}")))?;
                let part = rcad_document::sheet::contour_flange(
                    feature, &chain, thickness, radius, k_factor, flip,
                )?;
                let flat = rcad_document::sheet::flat_pattern_sketch(&part, width);
                let text = rcad_exchange::dxf::export(
                    &flat,
                    &rcad_exchange::dxf::DxfExportOptions {
                        layer: "CUT".into(),
                        include_construction: true,
                        construction_layer: "BEND".into(),
                    },
                )
                .map_err(|e| DocError::File(e.to_string()))?;
                write_atomic(&path, text.as_bytes())?;
                r.message = Some(format!(
                    "flat pattern {:.6} x {width} mm, {} bend line(s)",
                    part.flat_length,
                    part.bend_lines.len()
                ));
            }
            Command::AlignBody {
                name,
                from,
                to,
                offset,
            } => {
                let (name, from, to, offset) = (name.clone(), *from, *to, *offset);
                let id = self.model_mut()?.transact("Align", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Align"));
                    Ok(d.push(n, FeatureKind::AlignBody { from, to, offset }))
                })?;
                r.feature = Some(id);
            }
            Command::ScaleBody {
                name,
                body,
                center,
                factor,
            } => {
                let (name, body, center, factor) = (name.clone(), *body, *center, *factor);
                rcad_kernel::Rigid::scaling(center, factor)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                let m = self.model_mut()?;
                let src = m.document().body(body)?.name.clone();
                let id = m.transact(&format!("Scale {src}"), |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Scale"));
                    Ok(d.push(
                        n,
                        FeatureKind::ScaleBody {
                            body,
                            center,
                            factor,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::ShellFace {
                name,
                face,
                thickness,
            } => {
                let (name, face, thickness) = (name.clone(), *face, *thickness);
                let id = self.model_mut()?.transact("Shell", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Shell"));
                    Ok(d.push(n, FeatureKind::ShellFace { face, thickness }))
                })?;
                r.feature = Some(id);
            }
            Command::SplitBody {
                name,
                body,
                plane,
                body_name,
            } => {
                let (name, body, plane, body_name) =
                    (name.clone(), *body, *plane, body_name.clone());
                let m = self.model_mut()?;
                let src = m.document().body(body)?.name.clone();
                let id = m.transact(&format!("Split {src}"), |d| {
                    let n = match &body_name {
                        Some(n) => valid_name(n, "body")?,
                        None => d.default_body_name(),
                    };
                    if d.bodies.iter().any(|b| b.name == n) {
                        return Err(DocError::Validation(format!(
                            "another body is already named '{n}'"
                        )));
                    }
                    let new_body = d.add_body(n);
                    let n = name.unwrap_or_else(|| d.default_name("Split"));
                    Ok(d.push(
                        n,
                        FeatureKind::SplitBody {
                            body,
                            plane,
                            new_body,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::SetRollback { at } => {
                let at = *at;
                let m = self.model_mut()?;
                let label = match at {
                    Some(a) => format!("Roll back to {}", m.document().feature(a)?.name),
                    None => "Roll to end".to_string(),
                };
                m.transact(&label, |d| d.set_rollback(at))?;
                r.feature = at;
            }
            Command::MoveFeature { feature, before } => {
                let (feature, before) = (*feature, *before);
                let m = self.model_mut()?;
                let name = m.document().feature(feature)?.name.clone();
                m.transact(&format!("Move {name}"), |d| d.move_feature(feature, before))?;
                r.feature = Some(feature);
            }
            Command::SetBodyMaterial {
                body,
                material,
                density,
                density_unit,
                name,
            } => {
                let body = *body;
                let mat = match (material, density) {
                    (Some(_), Some(_)) => {
                        return Err(DocError::Validation(
                            "give a catalog material or a density, not both (a changed density is a user-defined material: density + name)".into(),
                        ));
                    }
                    (Some(key), None) => Some(rcad_document::Material::from_catalog(key)?),
                    (None, Some(d)) => Some(rcad_document::Material::user(
                        name.as_deref(),
                        *d,
                        *density_unit,
                    )?),
                    (None, None) => None,
                };
                let m = self.model_mut()?;
                let bname = m.document().body(body)?.name.clone();
                let label = match &mat {
                    Some(x) => format!("Set material of {bname} to {}", x.name),
                    None => format!("Clear material of {bname}"),
                };
                m.transact(&label, |d| {
                    d.body_mut(body)?.material = mat.clone();
                    Ok(())
                })?;
            }
            Command::Combine {
                name,
                target,
                tools,
                op,
                keep_tools,
            } => {
                let (target, tools, op, keep_tools) = (*target, tools.clone(), *op, *keep_tools);
                let name = name.clone();
                let m = self.model_mut()?;
                let tname = m.document().body(target)?.name.clone();
                for t in &tools {
                    m.document().body(*t)?;
                }
                let verb = match op {
                    k::CombineOp::Join => "Join",
                    k::CombineOp::Cut => "Cut",
                    k::CombineOp::Intersect => "Intersect",
                };
                let id = m.transact(&format!("Combine ({verb}) into {tname}"), |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Combine"));
                    Ok(d.push(
                        n,
                        FeatureKind::Combine {
                            target,
                            tools,
                            op,
                            keep_tools,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::CreateComponent { .. }
            | Command::DeleteComponent { .. }
            | Command::SetPartNumber { .. }
            | Command::AddOccurrence { .. }
            | Command::SetOccurrenceTransform { .. }
            | Command::DeleteOccurrence { .. }
            | Command::SetGrounded { .. }
            | Command::AddJoint { .. }
            | Command::SetJointValue { .. }
            | Command::SetJointLimits { .. }
            | Command::DeleteJoint { .. }
            | Command::Bom
            | Command::Interference { .. }
            | Command::ExportAssembly { .. }
            | Command::SetJointValues { .. }
            | Command::RenameComponent { .. }
            | Command::RenameOccurrence { .. }
            | Command::ActivateComponent { .. }
            | Command::SetOccurrenceHidden { .. }
            | Command::MakeIndependent { .. }
            | Command::CreateRigidGroup { .. }
            | Command::SetExplodeOffset { .. }
            | Command::SetExploded { .. }
            | Command::AlignOccurrence { .. } => {
                let m = self.model_mut()?;
                assembly::run(m, cmd, r)?;
            }
            Command::MeasureFaces { a, b } => {
                let m = self.model_mut()?;
                r.face_measure = Some(measure_faces::report(m, *a, *b)?);
            }
            Command::MassProperties {
                bodies,
                occurrences,
            } => {
                let m = self.model_mut()?;
                r.mass_properties =
                    Some(mass::report(m, bodies.as_deref(), occurrences.as_deref())?);
            }
            Command::SetBodyVisible { body, visible } => {
                let (body, visible) = (*body, *visible);
                let m = self.model_mut()?;
                let name = m.document().body(body)?.name.clone();
                m.transact(
                    &format!("{} {name}", if visible { "Show" } else { "Hide" }),
                    |d| {
                        d.body_mut(body)?.visible = visible;
                        Ok(())
                    },
                )?;
            }
            Command::SetSketchPlane { feature, plane } => {
                let (feature, plane) = (*feature, *plane);
                let m = self.model_mut()?;
                let name = m.document().feature(feature)?.name.clone();
                m.transact(&format!("Re-attach {name}"), |d| {
                    match &mut d.feature_mut(feature)?.kind {
                        FeatureKind::Sketch { plane: p, .. } => {
                            *p = plane;
                            Ok(())
                        }
                        _ => Err(DocError::Validation(format!("{feature} is not a sketch"))),
                    }
                })?;
                r.feature = Some(feature);
            }
            Command::ImportStep { path, units_to_mm } => {
                let bytes = read_import(path)?;
                let unit = *units_to_mm;
                // Parse once up front for body names and an early, precise error.
                let imp = rcad_document::import::step(&bytes, unit)?;
                if imp.solids.is_empty() {
                    return Err(DocError::Validation(format!(
                        "{}: no solids found",
                        path.display()
                    )));
                }
                let names: Vec<Option<String>> =
                    imp.solids.iter().map(|s| s.name.clone()).collect();
                let file = file_name(path);
                let stem = file_stem(path);
                let notes = imp.notes;
                let id = self.model_mut()?.transact(&format!("Import {file}"), |d| {
                    let blob = d.add_blob(&file, &bytes);
                    let mut bodies = Vec::new();
                    for (i, n) in names.iter().enumerate() {
                        let base = n
                            .as_deref()
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                if names.len() == 1 {
                                    stem.clone()
                                } else {
                                    format!("{stem}-{}", i + 1)
                                }
                            });
                        let name = unique_body_name(d, &base);
                        bodies.push(d.add_body(name));
                    }
                    let n = d.default_name("Import");
                    Ok(d.push(
                        n,
                        FeatureKind::ImportStep {
                            blob,
                            unit_override_to_mm: unit,
                            bodies,
                        },
                    ))
                })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ImportStl { path, units_to_mm } => {
                let to_mm = units_to_mm.ok_or_else(|| {
                    DocError::Validation(
                        "STL files have no units: choose the file unit (units_to_mm, e.g. 1 for \
                         mm, 25.4 for inches)"
                            .into(),
                    )
                })?;
                if !to_mm.is_finite() || to_mm <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "units_to_mm must be > 0, got {to_mm}"
                    )));
                }
                let bytes = read_import(path)?;
                let (_, notes) = rcad_document::import::stl(&bytes, to_mm)?;
                let file = file_name(path);
                let stem = file_stem(path);
                let id = self.model_mut()?.transact(&format!("Import {file}"), |d| {
                    let blob = d.add_blob(&file, &bytes);
                    let body = d.add_body(unique_body_name(d, &stem));
                    let n = d.default_name("Import");
                    Ok(d.push(n, FeatureKind::ImportMesh { blob, to_mm, body }))
                })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ImportDxf {
                path,
                plane,
                name,
                units_to_mm,
                allow_partial,
                layers,
            } => {
                let bytes = read_import(path)?;
                let opts = rcad_exchange::dxf::DxfImportOptions {
                    layers: layers.clone(),
                    allow_partial: *allow_partial,
                    unit_override_to_mm: *units_to_mm,
                    ..Default::default()
                };
                let imp = rcad_exchange::dxf::import(&bytes, &opts)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                let mut notes = vec![format!(
                    "units: {} ({} mm per unit, {})",
                    imp.units.name,
                    imp.units.to_mm,
                    if imp.units.declared {
                        "declared in file"
                    } else {
                        "chosen at import"
                    }
                )];
                notes.push(format!(
                    "imported {}",
                    imp.imported
                        .iter()
                        .map(|c| format!("{} x{}", c.entity_type, c.count))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for sk in &imp.skipped {
                    notes.push(format!(
                        "skipped {} x{} ({})",
                        sk.entity_type, sk.count, sk.reason
                    ));
                }
                // Degenerate entities are listed in both `skipped` and `warnings`.
                notes.extend(
                    imp.warnings
                        .iter()
                        .filter(|w| !w.contains(": dropped "))
                        .map(|w| format!("warning: {w}")),
                );
                let (plane, name, sketch) = (*plane, name.clone(), imp.sketch);
                let stem = file_stem(path);
                let kept = notes.clone();
                let id = self
                    .model_mut()?
                    .transact(&format!("Import {stem}.dxf"), |d| {
                        let n = name.unwrap_or_else(|| d.default_name(&format!("{stem}-")));
                        let id = d.push(
                            n,
                            FeatureKind::Sketch {
                                plane,
                                sketch,
                                links: Vec::new(),
                            },
                        );
                        d.feature_mut(id)?.notes = kept;
                        Ok(id)
                    })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ImportSvg {
                path,
                plane,
                name,
                units_to_mm,
                allow_partial,
            } => {
                let bytes = read_import(path)?;
                let opts = rcad_exchange::svg::SvgImportOptions {
                    allow_partial: *allow_partial,
                    unit_override_to_mm: *units_to_mm,
                    ..Default::default()
                };
                let imp = rcad_exchange::svg::import(&bytes, &opts)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                let mut notes = vec![format!(
                    "units: {} ({} mm per user unit, {})",
                    imp.units.name,
                    imp.units.to_mm,
                    if imp.units.declared {
                        "declared in file"
                    } else {
                        "chosen at import"
                    }
                )];
                notes.push(format!(
                    "imported {}",
                    imp.imported
                        .iter()
                        .map(|c| format!("{} x{}", c.entity_type, c.count))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for sk in &imp.skipped {
                    notes.push(format!(
                        "skipped {} x{} ({})",
                        sk.entity_type, sk.count, sk.reason
                    ));
                }
                // Degenerate entities are listed in both `skipped` and `warnings`.
                notes.extend(
                    imp.warnings
                        .iter()
                        .filter(|w| !w.contains(": dropped "))
                        .map(|w| format!("warning: {w}")),
                );
                let (plane, name, sketch) = (*plane, name.clone(), imp.sketch);
                let stem = file_stem(path);
                let kept = notes.clone();
                let id = self
                    .model_mut()?
                    .transact(&format!("Import {stem}.svg"), |d| {
                        let n = name.unwrap_or_else(|| d.default_name(&format!("{stem}-")));
                        let id = d.push(
                            n,
                            FeatureKind::Sketch {
                                plane,
                                sketch,
                                links: Vec::new(),
                            },
                        );
                        d.feature_mut(id)?.notes = kept;
                        Ok(id)
                    })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ExportSvg { path, sketch } => {
                let m = self.model_mut()?;
                let solved = m
                    .evaluation()
                    .features
                    .iter()
                    .find(|f| f.id == *sketch)
                    .and_then(|f| f.solved_sketch.clone())
                    .ok_or_else(|| DocError::Validation(format!("{sketch} is not a sketch")))?;
                let text = rcad_exchange::svg::export(&solved)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                write_atomic(path, text.as_bytes())?;
                r.message = Some(format!(
                    "wrote {} ({} bytes, SVG, mm, local sketch coordinates with y negated)",
                    path.display(),
                    text.len()
                ));
            }
            Command::ExportDxf { path, sketch } => {
                let m = self.model_mut()?;
                let solved = m
                    .evaluation()
                    .features
                    .iter()
                    .find(|f| f.id == *sketch)
                    .and_then(|f| f.solved_sketch.clone())
                    .ok_or_else(|| DocError::Validation(format!("{sketch} is not a sketch")))?;
                let text = rcad_exchange::dxf::export(&solved, &Default::default())
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                write_atomic(path, text.as_bytes())?;
                r.message = Some(format!(
                    "wrote {} ({} bytes, R12 ASCII, mm)",
                    path.display(),
                    text.len()
                ));
            }
            Command::RenameFeature { feature, name } => {
                let (feature, name) = (*feature, name.trim().to_string());
                if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                    return Err(DocError::Validation(
                        "feature name must be 1-128 printable characters".into(),
                    ));
                }
                let m = self.model_mut()?;
                if m.document()
                    .features
                    .iter()
                    .any(|f| f.id != feature && f.name == name)
                {
                    return Err(DocError::Validation(format!(
                        "another feature is already named '{name}'"
                    )));
                }
                let old = m.document().feature(feature)?.name.clone();
                m.transact(&format!("Rename {old} to {name}"), |d| {
                    d.feature_mut(feature)?.name = name.clone();
                    Ok(())
                })?;
                r.feature = Some(feature);
            }
            Command::Undo => r.message = Some(format!("undid: {}", self.model_mut()?.undo()?)),
            Command::Redo => r.message = Some(format!("redid: {}", self.model_mut()?.redo()?)),
            Command::Save { path } => {
                let m = self.model_mut()?;
                m.complete_measurements()?;
                save_file(path, m.document(), Some(m.evaluation()))?;
                m.dirty = false;
                self.path = Some(path.clone());
                r.message = Some(format!("saved {}", path.display()));
            }
            Command::SaveCopy { path } => {
                if self.path.as_deref() == Some(path.as_path()) {
                    return Err(DocError::Validation(format!(
                        "{} is the open document's own file; a copy must go elsewhere",
                        path.display()
                    )));
                }
                let m = self.model_mut()?;
                save_file(path, m.document(), Some(m.evaluation()))?;
                r.message = Some(format!(
                    "wrote a copy of revision {} to {}",
                    m.revision,
                    path.display()
                ));
            }
            Command::OpenCopy { path } => {
                let nf = load_file(path)?;
                let (mut model, warning) = Model::from_document_reporting(nf.document)?;
                model.dirty = true;
                self.model = Some(model);
                self.path = None;
                self.doc_id += 1;
                self.fresh_model_settings();
                let mut msg = format!("opened a copy of {} as an unsaved document", path.display());
                if let Some(w) = warning {
                    msg += &format!(
                        "; WARNING: the parameter expressions do not evaluate ({w}); showing the \
                         stored values. Fix the named parameter or expression before editing"
                    );
                }
                r.message = Some(msg);
            }
            Command::Open { path } => {
                let nf = load_file(path)?;
                let (model, warning) = Model::from_document_reporting(nf.document)?;
                let mut msg = format!("opened {}", path.display());
                if let Some(w) = warning {
                    msg += &format!(
                        "; WARNING: the parameter expressions do not evaluate ({w}); showing the \
                         stored values. Fix the named parameter or expression before editing"
                    );
                }
                if let (Some(c), Some(m)) = (&nf.cache, &model.evaluation().measurements)
                    && let (Some(v0), k::VolumeResult::Value { volume, .. }) = (c.volume, &m.volume)
                {
                    msg += &format!("; regenerated volume {volume:.9} vs saved cache {v0:.9}");
                }
                self.model = Some(model);
                self.path = Some(path.clone());
                self.doc_id += 1;
                self.fresh_model_settings();
                r.message = Some(msg);
            }
            Command::Close => {
                self.model_mut()?;
                self.model = None;
                self.path = None;
                self.doc_id += 1;
            }
            Command::ExportStl {
                path,
                chord,
                bodies,
                print: opts,
            } => {
                let chord = chord.unwrap_or(k::tol::MESH_CHORD);
                let m = self.model_mut()?;
                print::export(
                    m,
                    path,
                    chord,
                    bodies.as_deref(),
                    opts,
                    print::Format::Stl,
                    r,
                )?;
                r.measurements = m.evaluation().measurements.clone();
            }
            Command::Export3mf {
                path,
                chord,
                bodies,
                unit,
                print: opts,
            } => {
                let chord = chord.unwrap_or(k::tol::MESH_CHORD);
                let unit = unit.unwrap_or(rcad_exchange::threemf::ThreeMfUnit::Millimeter);
                let m = self.model_mut()?;
                let format = print::Format::ThreeMf(unit);
                print::export(m, path, chord, bodies.as_deref(), opts, format, r)?;
            }
            Command::Import3mf { path } => {
                let bytes = read_import(path)?;
                let (objects, notes) = rcad_document::import::threemf(&bytes)?;
                if objects.is_empty() {
                    return Err(DocError::Validation(format!(
                        "{}: no build items",
                        path.display()
                    )));
                }
                let names: Vec<Option<String>> = objects.iter().map(|o| o.name.clone()).collect();
                let file = file_name(path);
                let stem = file_stem(path);
                let id = self.model_mut()?.transact(&format!("Import {file}"), |d| {
                    let blob = d.add_blob(&file, &bytes);
                    let mut bodies = Vec::new();
                    for (i, n) in names.iter().enumerate() {
                        let base = n
                            .as_deref()
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("{stem}-{}", i + 1));
                        let name = unique_body_name(d, &base);
                        bodies.push(d.add_body(name));
                    }
                    let n = d.default_name("Import");
                    Ok(d.push(n, FeatureKind::Import3mf { blob, bodies }))
                })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ExportStep { path, bodies } => {
                let m = self.model_mut()?;
                let ev = m.evaluation();
                let mut ids = export_selection(m, bodies.as_deref())?;
                // Default selection: visible B-rep bodies; meshes are named, not
                // silently dropped. An explicit selection with a mesh is refused.
                let mut skipped = Vec::new();
                if bodies.is_none() {
                    ids.retain(|id| match ev.body_by_id(*id) {
                        Some(b) if b.geometry.mesh().is_some() => {
                            skipped.push(b.name.clone());
                            false
                        }
                        _ => true,
                    });
                    if ids.is_empty() {
                        return Err(DocError::Validation(format!(
                            "no visible B-rep body to export (mesh bodies cannot be written \
                             to STEP: {})",
                            skipped.join(", ")
                        )));
                    }
                }
                let mut named = Vec::new();
                for id in &ids {
                    let b = ev.body_by_id(*id).expect("selection checked");
                    let body = b.geometry.brep().ok_or_else(|| {
                        DocError::Validation(format!(
                            "{} is a mesh (STL) body; STEP export needs exact B-rep bodies",
                            b.name
                        ))
                    })?;
                    named.push((b.name.as_str(), body));
                }
                let text = k::export_step_bodies(&named, "R-CAD")
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                write_atomic(path, text.as_bytes())?;
                r.message = Some(format!(
                    "wrote {} ({} bytes, {} solid{}) from committed revision {}{}",
                    path.display(),
                    text.len(),
                    ids.len(),
                    if ids.len() == 1 { "" } else { "s" },
                    m.revision,
                    if skipped.is_empty() {
                        String::new()
                    } else {
                        format!("; mesh bodies not written: {}", skipped.join(", "))
                    }
                ));
            }
            Command::Measure => {
                let m = self.model_mut()?;
                m.complete_measurements()?;
                r.measurements = m.evaluation().measurements.clone();
            }
            Command::Inspect => {}
            // Handled by `run_drawing` above.
            Command::CreateDrawing { .. }
            | Command::EditDrawing { .. }
            | Command::DeleteDrawing { .. }
            | Command::AddDrawingView { .. }
            | Command::EditDrawingView { .. }
            | Command::RemoveDrawingView { .. }
            | Command::AddHoleCallout { .. }
            | Command::RemoveHoleCallout { .. }
            | Command::AddDrawingDimension { .. }
            | Command::SetDimensionTolerance { .. }
            | Command::RemoveDrawingAnnotation { .. }
            | Command::SetBalloons { .. }
            | Command::SetBomTable { .. }
            | Command::AddDrawingNote { .. }
            | Command::RemoveDrawingNote { .. }
            | Command::ExportDrawing { .. } => {}
        }
        r.summary = Some(self.summary());
        Ok(())
    }

    pub fn summary(&self) -> DocSummary {
        let Some(m) = &self.model else {
            return DocSummary {
                doc_id: self.doc_id,
                open: false,
                measuring: false,
                path: None,
                dirty: false,
                revision: 0,
                can_undo: None,
                can_redo: None,
                features: vec![],
                bodies: vec![],
                parameters: vec![],
                configurations: vec![],
                active_configuration: None,
                assembly: Default::default(),
                drawings: vec![],
            };
        };
        DocSummary {
            doc_id: self.doc_id,
            open: true,
            measuring: m.measurements_pending(),
            path: self.path.clone(),
            dirty: m.dirty,
            revision: m.revision,
            can_undo: m.undo_label().map(Into::into),
            can_redo: m.redo_label().map(Into::into),
            features: summarize(m.document(), m),
            bodies: m
                .evaluation()
                .bodies
                .iter()
                .map(|b| BodySummary {
                    id: b.id,
                    name: b.name.clone(),
                    visible: b.visible,
                    kind: match b.geometry {
                        BodyGeometry::Brep(_) => "brep",
                        BodyGeometry::Mesh(_) => "mesh",
                    }
                    .into(),
                    source: b.source.clone(),
                    created_by: b.created_by,
                    last_feature: b.last_feature,
                    measurements: b.measurements.clone(),
                    mesh: b.geometry.mesh().map(|mb| mb.diagnostics.clone()),
                    planes: b.geometry.brep().map(k::face_planes).unwrap_or_default(),
                    measure_error: b.measure_error.clone(),
                    imported_faces: imported_faces(m.evaluation(), b.id),
                    material: m
                        .document()
                        .body(b.id)
                        .ok()
                        .and_then(|i| i.material.clone()),
                    mass: body_mass(m, b),
                    bounds: match &b.measurements {
                        Some(m) => Some(m.bounds),
                        None => b.geometry.brep().and_then(|g| k::bounds(g).ok()),
                    },
                })
                .collect(),
            parameters: parameter_summaries(m.document()),
            configurations: m.document().configurations.clone(),
            active_configuration: m.document().active_configuration.clone(),
            assembly: assembly::summary(m),
            drawings: m.document().drawings.clone(),
        }
    }
}

/// Density × measured (or mesh) volume, g.
fn body_mass(m: &Model, b: &rcad_document::EvalBody) -> Option<f64> {
    let mat = m.document().body(b.id).ok()?.material.as_ref()?;
    let v = match (&b.measurements, b.geometry.mesh()) {
        (
            Some(Measurements {
                volume: k::VolumeResult::Value { volume, .. },
                ..
            }),
            _,
        ) => *volume,
        (_, Some(mb)) if mb.diagnostics.closed => mb.diagnostics.signed_volume?,
        _ => return None,
    };
    (v > 0.0).then(|| v * mat.g_per_mm3())
}

fn summarize(doc: &Document, m: &Model) -> Vec<FeatureSummary> {
    let ev = m.evaluation();
    doc.features
        .iter()
        .map(|f| {
            let res = ev.features.iter().find(|r| r.id == f.id);
            let (kind, plane) = match &f.kind {
                FeatureKind::Sketch { plane, .. } => ("sketch", Some(*plane)),
                FeatureKind::Extrude { .. } => ("extrude", None),
                FeatureKind::Revolve { .. } => ("revolve", None),
                FeatureKind::Loft { .. } => ("loft", None),
                FeatureKind::Sweep { .. } => ("sweep", None),
                FeatureKind::Cut { .. } => ("cut_through_all", None),
                FeatureKind::Pocket { .. } => ("pocket", None),
                FeatureKind::ImportStep { .. } => ("import_step", None),
                FeatureKind::ImportMesh { .. } => ("import_mesh", None),
                FeatureKind::Import3mf { .. } => ("import_3mf", None),
                FeatureKind::LinearPattern { .. } => ("linear_pattern", None),
                FeatureKind::CircularPattern { .. } => ("circular_pattern", None),
                FeatureKind::Mirror { plane, .. } => ("mirror", Some(*plane)),
                FeatureKind::Chamfer { .. } => ("chamfer", None),
                FeatureKind::Fillet { .. } => ("fillet", None),
                FeatureKind::Hole { plane, .. } => ("hole", Some(*plane)),
                FeatureKind::Shell { .. } => ("shell", None),
                FeatureKind::MoveBody { copy: None, .. } => ("move_body", None),
                FeatureKind::MoveBody { .. } => ("copy_body", None),
                FeatureKind::SplitBody { plane, .. } => ("split_body", Some(*plane)),
                FeatureKind::SheetMetal { .. } => ("sheet_metal", None),
                FeatureKind::ShellFace { .. } => ("shell", None),
                FeatureKind::ScaleBody { .. } => ("scale_body", None),
                FeatureKind::AlignBody { .. } => ("align_body", None),
                FeatureKind::DirectEdit { .. } => ("direct_edit", None),
                FeatureKind::Combine { .. } => ("combine", None),
                FeatureKind::Countersink { .. } => ("countersink", None),
                FeatureKind::Thread { plane, .. } => ("thread", Some(*plane)),
            };
            let (operation, extent, reverse) = match &f.kind {
                FeatureKind::Extrude {
                    operation,
                    extent,
                    reverse,
                    ..
                } => (Some(*operation), Some(*extent), *reverse),
                _ => (None, None, false),
            };
            FeatureSummary {
                id: f.id,
                name: f.name.clone(),
                kind: kind.into(),
                parameters: f.parameters(),
                depends_on: doc.dependencies_of(f),
                plane,
                solve_status: res.and_then(|r| r.solve.as_ref()).map(|s| s.status),
                solve_dof: res.and_then(|r| r.solve.as_ref()).map(|s| s.dof),
                solve_residual: res.and_then(|r| r.solve.as_ref()).map(|s| s.residual),
                frame: res.and_then(|r| r.frame),
                solve_conflicts: match (&f.kind, res.and_then(|r| r.solve.as_ref())) {
                    (FeatureKind::Sketch { sketch, .. }, Some(rep)) => rep
                        .conflicting
                        .iter()
                        .map(|&i| sketch.constraint_label(i))
                        .collect(),
                    _ => vec![],
                },
                sketch: res.and_then(|r| r.solved_sketch.clone()),
                profile_error: res.and_then(|r| r.profile_error.clone()),
                faces: ev
                    .faces
                    .iter()
                    .filter(|c| c.reference.feature == f.id)
                    .map(face_summary)
                    .collect(),
                operation,
                extent,
                reverse,
                region: match &f.kind {
                    FeatureKind::Extrude { region, .. } => region.clone(),
                    _ => None,
                },
                body: res.and_then(|r| r.body),
                creates: f.created_bodies(),
                notes: f
                    .notes
                    .iter()
                    .cloned()
                    .chain(res.map(|r| r.notes.clone()).unwrap_or_default())
                    .collect(),
                expressions: f.expressions.clone(),
                suppressed: f.suppressed,
                rolled_back: doc.is_rolled_back(f.id),
                projections: match &f.kind {
                    FeatureKind::Sketch { links, .. } => links.iter().map(|l| l.source).collect(),
                    _ => vec![],
                },
            }
        })
        .collect()
}

fn default_csink_angle() -> f64 {
    90.0
}

fn default_chord_tol() -> f64 {
    0.01
}

fn default_clearance() -> f64 {
    0.1
}

/// [`BodySummary::imported_faces`] (empty when the body has no valid import
/// identity or classification fails).
fn imported_faces(ev: &rcad_document::Evaluation, body: BodyId) -> Vec<ImportedFace> {
    let (Some(ids), Some(b)) = (
        ev.imported_face_ids.get(&body),
        ev.body_by_id(body).and_then(|b| b.geometry.brep()),
    ) else {
        return vec![];
    };
    match k::classify_faces(b) {
        Ok(kinds) if kinds.len() == ids.ids.len() => kinds
            .into_iter()
            .enumerate()
            .filter_map(|(pos, kind)| {
                Some(ImportedFace {
                    face: ids.reference(body, pos)?,
                    kind,
                })
            })
            .collect(),
        _ => vec![],
    }
}

/// [`Command::ImportedFaces`] payload.
fn imported_faces_json(ev: &rcad_document::Evaluation, body: BodyId) -> Result<String, DocError> {
    let ids = ev.imported_face_ids.get(&body).ok_or_else(|| {
        DocError::Validation(format!(
            "body {body} is not an imported body changed only by its import and direct edits"
        ))
    })?;
    let b = ev
        .body_by_id(body)
        .and_then(|b| b.geometry.brep())
        .ok_or_else(|| DocError::Validation(format!("body {body} has no B-rep")))?;
    let mut out = Vec::new();
    for pos in 0..ids.ids.len() {
        let kind = k::classify_face(b, pos).map_err(|e| DocError::Kernel {
            feature: ids.valid_at,
            detail: e.to_string(),
        })?;
        out.push(serde_json::json!({ "face": ids.reference(body, pos), "kind": kind }));
    }
    serde_json::to_string(&out).map_err(|e| DocError::Validation(e.to_string()))
}

fn parameter_summaries(doc: &Document) -> Vec<ParameterSummary> {
    use rcad_document::params;
    let env = params::evaluate_parameters(doc).ok();
    doc.parameters
        .iter()
        .map(|p| ParameterSummary {
            name: p.name.clone(),
            expression: p.expression.clone(),
            unit: p.unit,
            comment: p.comment.clone(),
            value: env.as_ref().and_then(|e| e.get(&p.name)).map(|q| q.value),
            depends_on: params::references(&p.expression),
            used_by: params::users_of(doc, &p.name),
        })
        .collect()
}

fn face_summary(c: &FaceCandidate) -> FaceSummary {
    FaceSummary {
        reference: c.reference,
        frame: c.frame,
        body: c.body,
        sample: c.sample,
    }
}

/// Target body for a Join/Cut when none was given: the body owning the face
/// the sketch is on, else the only B-rep body. Never guesses among several.
fn default_target(m: &Model, sketch: FeatureId) -> Result<BodyId, DocError> {
    let ev = m.evaluation();
    if let Some(rf) = ev
        .features
        .iter()
        .find(|r| r.id == sketch)
        .and_then(|r| r.resolved_face.as_ref())
    {
        return Ok(rf.body);
    }
    let breps: Vec<_> = ev
        .bodies
        .iter()
        .filter(|b| b.geometry.brep().is_some())
        .collect();
    match breps.as_slice() {
        [one] => Ok(one.id),
        [] => Err(DocError::Validation(
            "there is no solid body to join to or cut".into(),
        )),
        many => Err(DocError::Validation(format!(
            "choose a target body ({})",
            many.iter()
                .map(|b| format!("{} {}", b.id, b.name))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn default_shell_open() -> FaceRole {
    FaceRole::ExtrudeEndCap
}

/// Bodies to export: the given ids (must exist), else every visible body.
pub(crate) fn export_selection(m: &Model, ids: Option<&[BodyId]>) -> Result<Vec<BodyId>, DocError> {
    let ev = m.evaluation();
    let ids: Vec<BodyId> = match ids {
        Some(ids) => {
            for id in ids {
                if ev.body_by_id(*id).is_none() {
                    return Err(DocError::Validation(format!("unknown body {id}")));
                }
            }
            ids.to_vec()
        }
        None => ev
            .bodies
            .iter()
            .filter(|b| b.visible)
            .map(|b| b.id)
            .collect(),
    };
    if ids.is_empty() {
        return Err(DocError::Validation(
            "document has no (visible) body to export".into(),
        ));
    }
    Ok(ids)
}

/// 1-128 printable characters, trimmed.
fn valid_name(name: &str, what: &str) -> Result<String, DocError> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(DocError::Validation(format!(
            "{what} name must be 1-128 printable characters"
        )));
    }
    Ok(name)
}

fn unique_body_name(d: &Document, base: &str) -> String {
    let base: String = base.chars().filter(|c| !c.is_control()).take(100).collect();
    let base = if base.trim().is_empty() {
        "Body".to_string()
    } else {
        base
    };
    if !d.bodies.iter().any(|b| b.name == base) {
        return base;
    }
    (2..)
        .map(|i| format!("{base} ({i})"))
        .find(|n| !d.bodies.iter().any(|b| &b.name == n))
        .unwrap_or(base)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "import".into())
}

fn file_stem(p: &Path) -> String {
    p.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "import".into())
}

/// Read an import source, bounded by the native file limit (it is embedded).
fn read_import(path: &Path) -> Result<Vec<u8>, DocError> {
    let ferr = |e: std::io::Error| DocError::File(format!("open {}: {e}", path.display()));
    let len = std::fs::metadata(path).map_err(ferr)?.len();
    let max = rcad_document::MAX_FILE_BYTES / 2;
    if len > max {
        return Err(DocError::File(format!(
            "{}: {len} bytes exceeds the import limit of {max} bytes",
            path.display()
        )));
    }
    std::fs::read(path).map_err(ferr)
}

/// Closed loop of lines through `vertices` with shared endpoints.
pub fn polygon_sketch(vertices: &[[f64; 2]]) -> Result<Sketch, rcad_sketch::SketchError> {
    if vertices.len() < 3 {
        return Err(rcad_sketch::SketchError::UnsupportedProfile(format!(
            "a polygon needs at least 3 vertices, got {}",
            vertices.len()
        )));
    }
    let mut s = Sketch::default();
    s.ensure_origin();
    let ids: Vec<_> = vertices.iter().map(|v| s.add_point(v[0], v[1])).collect();
    for i in 0..ids.len() {
        s.add_line(ids[i], ids[(i + 1) % ids.len()]);
    }
    s.validate()?;
    Ok(s)
}

/// Entity ids are persistent names (face references use them, ADR 0003): an
/// edit may keep, remove or add entities, but must not give an existing or
/// removed id to different geometry. Ids removed by the edit are retired.
fn sketch_err(e: rcad_sketch::SketchError) -> DocError {
    DocError::Validation(e.to_string())
}

impl Session {
    /// A copy of sketch feature `feature`'s stored definition.
    fn stored_sketch(&self, feature: FeatureId) -> Result<Sketch, DocError> {
        let m = self
            .model
            .as_ref()
            .ok_or_else(|| DocError::Validation("no document is open".into()))?;
        match &m.document().feature(feature)?.kind {
            FeatureKind::Sketch { sketch, .. } => Ok(sketch.clone()),
            _ => Err(DocError::Validation(format!("{feature} is not a sketch"))),
        }
    }
}

fn keep_entity_ids(old: &Sketch, mut new: Sketch) -> Result<Sketch, DocError> {
    use rcad_sketch::Entity;
    // An entity keeps its identity when its endpoints change (the editor can
    // reconnect a line), but not when its kind changes, with one exception: a
    // circle may become an arc of the SAME centre point (Trim cuts a circle to
    // an arc; review run 3 #15). That arc is a piece of the same curve, so the
    // id (a persistent face name) stays with it. Every other kind change
    // (line <-> round, arc -> circle, circle -> arc about another centre) is
    // refused.
    let shape = |e: &Entity| match e {
        Entity::Line { .. } => 0,
        Entity::Circle { .. } => 1,
        _ => 2,
    };
    let trimmed_circle = |o: &Entity, e: &Entity| matches!((o, e), (Entity::Circle { center: c0, .. }, Entity::Arc { center: c1, .. }) if c0 == c1);
    for e in &new.entities {
        let id = e.id();
        let reused = match old.entity(id) {
            Some(o) => shape(o) != shape(e) && !trimmed_circle(o, e),
            None => id.0 < old.retired_entity_ids,
        };
        if reused {
            return Err(DocError::Validation(format!(
                "sketch entity id {} is reused for different geometry; new entities need fresh \
                 ids (sketch-entity ids are persistent face names)",
                id.0
            )));
        }
    }
    let removed = old
        .entities
        .iter()
        .map(|e| e.id())
        .filter(|id| new.entity(*id).is_none())
        .map(|id| id.0.saturating_add(1))
        .max()
        .unwrap_or(0);
    new.retired_entity_ids = new
        .retired_entity_ids
        .max(old.retired_entity_ids)
        .max(removed);
    Ok(new)
}

/// Validate and solve a sketch, returning the solved definition. Refuses
/// sketches whose constraints conflict, naming the culprits.
pub fn solved(mut s: Sketch) -> Result<Sketch, DocError> {
    s.validate()
        .map_err(|e| DocError::Validation(format!("invalid sketch: {e}")))?;
    let original = s.clone();
    let rep = s.solve();
    if !rep.is_success() {
        let culprits: Vec<String> = rep
            .conflicting
            .iter()
            .map(|&i| original.constraint_label(i))
            .collect();
        return Err(DocError::Validation(format!(
            "sketch does not solve ({:?}); conflicting: {}",
            rep.status,
            if culprits.is_empty() {
                "not identified".into()
            } else {
                culprits.join("; ")
            }
        )));
    }
    Ok(s)
}

/// Convenience: the semantic reference to an extrusion's end cap ("top face").
pub fn end_cap_of(extrude: FeatureId) -> SketchPlane {
    SketchPlane::Face(FaceRef {
        feature: extrude,
        role: FaceRole::ExtrudeEndCap,
    })
}

fn list_has_name(list: &[rcad_document::Configuration], name: &str) -> bool {
    list.iter().any(|c| c.name.trim() == name)
}

/// A configuration's name, expressions, features, occurrences and parameters
/// must be valid for `d` (shared by `set_configuration` and the table).
fn check_configuration(
    d: &rcad_document::Document,
    c: &rcad_document::Configuration,
) -> Result<(), DocError> {
    use rcad_document::params;
    let name = &c.name;
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '))
    {
        return Err(DocError::Validation(format!(
            "configuration name '{name}': letters, digits, space, _ - . only"
        )));
    }
    for (p, e) in &c.values {
        params::check_syntax(e).map_err(|x| DocError::Expression {
            name: p.clone(),
            detail: format!("'{e}': {x}"),
        })?;
    }
    for f in &c.suppressed {
        d.feature(*f)?;
    }
    for o in &c.excluded_occurrences {
        if !d.assembly.occurrences.iter().any(|x| x.id == *o) {
            return Err(DocError::Validation(format!("unknown occurrence {o:?}")));
        }
    }
    for p in c.values.keys() {
        if !d.parameters.iter().any(|x| x.name == *p) {
            return Err(DocError::Expression {
                name: p.clone(),
                detail: "no such parameter".into(),
            });
        }
    }
    Ok(())
}
