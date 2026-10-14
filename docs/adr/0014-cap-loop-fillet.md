# ADR 0014: Cap-loop fillets as a direct B-rep construction

Status: accepted (2026-10-14)

## Context
Rounding the top edge loop of a box or rounded-rectangle extrusion is the most common
finishing step for printed parts. The single-edge fillet (`fillet_edge`) cuts one straight
edge with a prismatic slab tool and refuses edges that meet other fillets at a corner. Doing
the loop with booleans would need cylinder, torus and sphere tools cut against each other
and the walls. Truck's boolean (even with the fork in ADR 0013) is fragile on
torus/cylinder intersections, and the result would carry implicit intersection curves that
STEP export cannot write.

## Decision
`rcad_kernel::fillet_cap_loop(body, cap, r)` builds the filleted solid directly when the body
is a straight prism under the cap (checked on the B-rep: one cap, one opposite cap, every
other face a wall ruled along the cap normal, exact volume = cap area x depth):

- every wall is kept to depth r below the cap;
- each loop edge gets an exact fillet face (cylinder along a line, torus about an arc's axis,
  sphere when the arc radius equals r);
- consecutive fillet faces share an exact section edge: a quarter circle at a tangent
  junction, or the quarter ellipse where two equal-radius cylinders meet at a sharp
  line-line corner (the miter);
- the top face is the loop inset by r; holes farther than r from the loop keep their
  full-height walls.

All curves are lines or rational quadratics. Revolved fillet surfaces are parametrised so
that no face touches the surface seam or a pole. For a sphere patch, that means a
revolution about a horizontal axis that misses the patch.

Every case outside this scope is refused by name: a sharp corner touching an arc, a convex
arc radius below r, overlapping fillet bands, a hole within r of the loop, or a body that is
not a prism. The self-check compares the result's volume with a closed form (spandrel
area and first moment, Pappus for arcs, miter setbacks).

## Consequences
- No boolean is involved, so the result is deterministic and fast. Every edge is explicit,
  and STEP export writes it exactly. FreeCAD reads all fixtures as single valid solids
  whose volume matches the closed form to about 1e-9 relative (`tools/check_cap_fillet.sh`).
- The construction rebuilds the whole body, so all faces are new objects. Downstream
  semantic references resolve by plane and footprint as before.
- Only prisms are in scope. A cap loop on a body with other blends, drafts or steps under
  the cap is refused. A general rolling-ball blend remains future work.
- The existing exact boundary integrals do not cover tori about different axes or spheres.
  A general method was therefore added: `BrepBoundaryIntegralParametric`, Green's theorem in
  each face's own parameters (`measure/parametric.rs`). It gives these bodies an exact
  volume and exact mass properties, and the self-check compares it with the closed form
  to 1e-9.
