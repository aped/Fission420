# ADR 0009: Exact sweep along a planar line/arc path (narrow scope)

Status: accepted (run 5, SESSION_5 §9.4).

## Decision

`rcad_kernel::sweep(profile: &Profile, frame: &Frame, path: &[PathSegment]) -> Result<Body>`
(`crates/rcad-kernel/src/sweep.rs`), with

```rust
pub enum PathSegment {
    Line { end: [f64; 3] },                    // world mm
    Arc { center: [f64; 3], end: [f64; 3] },   // world mm
}
```

* **Path.** Starts at `frame.origin` with tangent `frame.z` (the profile normal). Each
  segment continues from the previous end: a line must point along the current tangent; an
  arc leaves along it, so `start − center ⊥ tangent`; its axis is `(start − center) ×
  tangent` and its angle that of the rotation from `start − center` to `end − center` about
  that axis, in (0, 2π). Tolerances: tangency 1e-9 (direction), end on circle and in plane
  `KERNEL_LINEAR`.
* **Frame rule.** The profile is carried rigidly: translated along lines, rotated about the
  arc axis along arcs. For a planar path this is the rotation-minimizing frame (no twist
  about the tangent).
* **Construction.** Each segment is a Truck sweep of the previous segment's end-cap face
  (`tsweep` for lines, `rsweep` about the arc axis for arcs): the next segment's start cap
  is that face itself, so segments share edges exactly; all faces are exact (planes,
  cylinders, cones/tori from revolved lines and arcs).
* **Refused by name:** a path not starting along the frame normal; non-tangent joints;
  non-planar paths (arcs with non-parallel axes or centres off the path plane); arcs of a
  full turn or more; an arc whose radius does not exceed the profile's extent towards its
  centre; paths turning a full turn in total; a later part of the path coming back within
  twice the profile's reach of an earlier part (a conservative self-intersection guard,
  ignoring the stretch of (π + 2) × reach of arc length right after a point).
* **Checks.** One closed shell; volume = Pappus (area × length per line, area × angle ×
  centroid distance from the axis per arc), exact through `measure` when all curved faces
  share an axis (e.g. a rectangle swept by line/arc/line) and, since the later run-5
  `BrepBoundaryIntegralRevolution` path, for tori about one axis (a circular profile along an
  arc); other cases fall back to the labelled tessellation estimate; FreeCAD confirms
  independently
  (`tools/check_sweep.sh`).

## Consequences

* Handle (8 × 4 along 10 + 90° R 15 + 10): 32 (20 + 7.5π) mm³, exact; Ø6 channel along the
  same path: FreeCAD 4.5e-7 mm³ off Pappus.
* Not supported: non-planar (helical) paths, splines, twisted frames, profiles with holes.
