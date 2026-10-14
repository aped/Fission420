# Solid fillets and loop chamfers

Two exact solid fillet operations and a loop chamfer exist (the single-edge chamfer is
`chamfer`, docs/COMMANDS.md). Both are features in the document history, both
are reachable through the typed command API, and both are offered in the face inspector.

| Operation | Command | What it rounds | Faces created |
|---|---|---|---|
| Edge fillet | `fillet` (faces `a`, `b`, `radius`) | one straight edge between two planar faces, on a body prismatic along that edge | one cylinder |
| Cap-loop fillet | `fillet_loop` (face `face`, `radius`) | every edge of the outer loop of a planar cap face, on a body that is a straight prism under that face | cylinders (straight edges), tori (arcs), spheres (arcs of radius = r) |
| Cap-loop chamfer | `chamfer_loop` (face `face`, `distance`) | the same loop, 45° with equal legs | planes (straight edges), cones (arcs) |

## Cap-loop fillet (`rcad_kernel::fillet_cap_loop`, ADR 0014)

The usual print workflow is a box or rounded rectangle with its top edge rounded:

1. Extrude a rectangle, then round the vertical edges. You can use sketch fillets in the
   profile, or four edge fillets of radius R.
2. Apply the cap-loop fillet to the top face with radius r <= R.

With r < R, the corners are tori of major radius R - r. With r = R, they are sphere patches
(the three-equal-radii box corner). On a box with sharp vertical edges, adjacent
top-edge cylinders meet in an exact miter (a quarter ellipse in the bisector plane), and
the vertical edge stays sharp.

Supported:
- loops of lines and circular arcs whose junctions are tangent, or are sharp corners
  between two straight edges (convex or concave);
- convex arcs with R >= r, and concave arcs of any radius;
- holes in the cap that are farther than r from the loop (they keep full-height walls).

Refused with the reason:
- a sharp corner touching an arc;
- a convex arc with R < r, or with 0 < R - r < 0.001 mm;
- a straight edge too short for its corner setbacks;
- walls closer than r to the inset loop (the fillet bands would overlap);
- a hole within r of the loop;
- r at least as large as the body depth;
- a body that is not a straight prism under the face. This includes filleting the same
  loop twice, or a cap face whose body has an edge-filleted top edge.

### Volume oracle
The removed cross-section is the spandrel between two perpendicular walls and the
quarter circle. Its area is A = r^2 (1 - pi/4), and its first moment about either wall is
M = r^3 (5/6 - pi/4). Its centroid is d = M / A = r (10 - 3 pi) / (12 - 3 pi) from each wall.

Along a loop of length P, with signed arc sweeps phi_i and sharp-corner turning angles
alpha_j (left positive), the removed volume is:

    A * P  -  M * (sum phi_i + 2 * sum tan(alpha_j / 2))

This combines Pappus for the arcs with the miter setback d * tan(alpha/2) at depth d on
both edges of a corner. The kernel compares the result with this value. The tests and
`tools/check_cap_fillet.sh` (FreeCAD) use hand-derived values per case.

### Loop chamfer
`chamfer_loop` / `rcad_kernel::chamfer_cap_loop` is the same construction with a straight
section: 45° planes along lines, cones about the arcs' axes, and straight miter lines at
every junction. A convex arc needs R > d, because R = d would end the cone in an apex and
is refused. The section has A = d^2/2 and M = d^3/6. The tests check it against the
prismatoid band of convex polygons, d/6 (A0 + 4 Am + A1).

Chamfering or filleting edges one at a time where they meet at a corner is refused with a
reason that names the loop commands. Single-edge blends do not build the mitered or
blended corner.

### Exact volume
The result's faces include tori about several axes, spheres and cones, so the volume and
mass properties are computed by the parametric boundary integral
(`BrepBoundaryIntegralParametric`, docs/TOLERANCES.md). It matches the closed forms
to 1e-9 relative.
