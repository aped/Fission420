# ADR 0012: Sheet bodies and surface repair by exact shell merging

Status: accepted

## Context

The product needs folded sheet metal with edge flanges and surface repair (stitch, patch,
thicken). Truck 0.6 booleans are unreliable on coincident and tangent geometry (ADR 0001,
0005): uniting a plate with a flange prism that shares a whole face is exactly the coincident
case they fail on, and Truck has no sewing, no open-shell solids and no offset surfaces.

## Decision

1. **Shell merging instead of booleans** (`rcad_kernel::stitch`). On Truck's compressed
   representation: concatenate shells, merge vertices within a tolerance (union–find over an
   x-sorted sweep, earliest vertex wins), merge edges with the same end vertices and midpoints
   within the tolerance (orientation flags of face wires fixed up), optionally remove pairs of
   planar faces bounded by the same edges with opposite normals (**glue**), compact, extract.
   Geometry is never re-fitted: a straight edge whose vertices merged is rebuilt as the exact
   line between them; surfaces are unchanged.
2. **Sheet bodies from their definition.** The folded body is rebuilt at every regeneration from
   the base profile and the flange list: the plate (exact extrusion of the outline with relief
   notches) and one prism per flange (exact section of a θ annulus R..R+t plus the leg, extruded
   along the edge), **glued** at tolerance `sheet::GLUE_TOL` = 1e-7 mm (the pieces share exact
   corner points up to rounding). The vertices along the attachment edge are inserted into the
   plate outline so the shared rectangle is the same four edges on both sides. The flat pattern
   is computed from the same definition (bend allowance θ(R + Kt)), not by unfolding faces.
   Validity checks (footprint overlap, flange into plate, concave corners, relief fits, simple
   outlines) run on the definition before any B-rep is built.
3. **Open shells are Bodies** whose solid is built with `Solid::new_unchecked` (no closedness
   check). `Body::is_open_shell` distinguishes them; `measure` reports no volume, display meshes
   them per shell, STEP export refuses them, and the document's single gate for exact
   modifications (`regen::brep_target`) refuses them.
4. **Stitch/fill/thicken are new-body features** keeping their input. Fill adds exact planar
   faces on planar boundary loops (Newell plane, planarity within the tolerance). Thicken is the
   exact prism of one planar face, or the exact annular sector of one cylindrical patch, with
   offset collapse (inward d ≥ r) refused.

## Consequences

* Exact results without booleans; independent checks (closed-form volumes, point membership,
  FreeCAD validity/volume/bbox) pass at 1e-6 mm³.
* Coplanar adjacent faces remain separate faces after gluing (valid B-rep, not maximal).
* Healing beyond float noise leaves faces up to the tolerance off their boundaries; this is
  stated with the tolerance in the feature notes and documented (docs/SURFACES.md).
* Flange-flange collision checks are conservative 2-D footprint tests for flanges leaning past
  90°; exact 3-D clearance is not computed.
* No general offsetting: multi-face thicken, shell of sheet bodies and non-planar patches remain
  unimplemented.
