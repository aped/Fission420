# Materials and mass properties

Code: `crates/rcad-document/src/material.rs` (catalog, validation),
`crates/rcad-kernel/src/measure/mass.rs` + `measure/revolution.rs` (volume, centroid, second
moments), `crates/rcad-commands/src/mass.rs` (mass, inertia, totals). Commands:
`set_body_material`, `mass_properties` (docs/COMMANDS.md).

## Material is not appearance

A body's material (`BodyInfo.material`: name, density in g/cm³, optional catalog key) is used
only for mass properties. It does not change the display colour, and the display colour (when
there is one) does not imply a material. A body without a material has a volume and a centroid
but no mass; totals over bodies are only reported when every listed body has a material.

## Density catalog (typical nominal values)

| key | name | density g/cm³ | typical published range |
|---|---|---|---|
| `pla` | PLA | 1.24 | 1.21–1.26 |
| `petg` | PETG | 1.27 | 1.23–1.29 |
| `abs` | ABS | 1.04 | 1.01–1.08 |
| `asa` | ASA | 1.07 | 1.05–1.10 |
| `pa12` | Nylon PA12 | 1.01 | 1.00–1.03 |
| `tpu` | TPU 95A | 1.21 | 1.18–1.24 |
| `al6061` | Aluminium 6061 | 2.70 | 2.69–2.71 |
| `steel` | Carbon steel | 7.85 | 7.80–7.87 |
| `ss304` | Stainless steel 304 | 8.00 | 7.90–8.03 |
| `brass` | Brass C360 | 8.50 | 8.40–8.55 |

**Provenance and uncertainty.** These are typical nominal densities of the solid material as
commonly quoted in filament manufacturers' technical data sheets (polymers) and general
engineering handbooks (metals). They were compiled from general knowledge for this catalog and
were **not** checked against one specific cited source; treat them as typical
values, not a specification. Real material varies with grade, supplier, fillers and colourant
(the ranges above). A 3D-printed part with less than 100 % infill, or with voids, weighs less
than density × solid volume; this program models solid bodies only. For a known material use
a user-defined density.

A catalog assignment copies the density into the document (`{"name": "PLA", "density": 1.24,
"catalog": "pla"}`), so a future catalog revision never silently changes a saved document's
mass; `catalog` records the provenance. A user-defined material has no `catalog` key.
Accepted densities: 0.001–30 g/cm³ (anything else is a unit mistake; the densest element is
about 22.6 g/cm³); `set_body_material` accepts g/cm³ (default) or kg/m³ (`density_unit:
"kg_per_m3"`). Hand-edited files with an invalid density are refused on load. A body copy
(`move_body` with `copy`) inherits the source's material.

Format: a document is written as v6 when any body has a material (`io::v6_feature`, "body
materials"); without materials files keep their previous version byte for byte.

## Mass properties

Frame and units: **world** coordinates and axes; mm, mm³, g/cm³, g, g·mm².

* Volume, centre of mass (= centroid at uniform density) and the second moments
  S = ∫ (p − c)(p − c)ᵀ dV about the centroid come from `rcad_kernel::volume_properties`.
  Inertia about the centre of mass is ρ (tr(S) I − S) with ρ in g/mm³ (density × 1e-3).
* **Exact methods** — the same selection as the volume (`VolumeMethod`): the Z, X, Y axis
  boundary integrals (curved faces ruled along the axis or in planes perpendicular to it;
  planar faces of any orientation) and the surface-of-revolution integral (cones, cylinders,
  tori about one axis; circular cylinders about any axes). The moments use the divergence
  theorem with F = G a (∂G/∂z = monomial, z along the method's axis a): ruled faces carry no
  flux, planar regions are fanned into triangles over their exact boundary chords and
  integrated with a cubature rule exact for the cubic G, faces of revolution and cylinders
  reduce to line integrals over their exact edges (Stokes; Gauss–Legendre along the
  meridian / ruling). Three chord levels with Richardson extrapolation; the reported errors
  are the differences of the two extrapolants (near rounding for exact methods).
* **Parametric boundary integral** (`BrepBoundaryIntegralParametric`, e.g. a filleted box
  with torus or sphere corners): the same fields F = G a over each face's own (u, v)
  parametrisation. Green's theorem turns the flux into a line integral along the face's exact
  edges, located on the surface point by point. Gauss–Legendre is used in both directions,
  on 4 and 8 panels per edge; the error is their difference. Refused (estimate fallback) for
  seam edges, poles on a boundary or implicit intersection edges.
* **Tessellation estimate** (`TessellationEstimate`, e.g. a sphere): the same integrals over
  the welded watertight tessellation at two chord tolerances; the finer is reported and the
  difference is the error estimate (an estimate, not a bound; see `tessellation_volume` in
  measure.rs). Labelled "Estimate" in the UI and `exact: false` in reports.
* **Mesh bodies** (STL/3MF imports, `TriangleMesh`): exact for the triangle mesh, which is
  the body's only geometry; open meshes have no mass properties.
* **Totals** (`MassReport.total`): masses add; the combined centre of mass is the
  mass-weighted mean; inertia tensors move to it with the parallel-axis theorem
  I += m (|d|² E − d dᵀ). `mass_properties` without `bodies` covers the visible bodies and
  lists the hidden ones it left out (`excluded_hidden`); suppressed, rolled-back and consumed
  bodies are not in the evaluated model and are refused when listed.
* `BodySummary.mass` is density × the body's measured volume (cheap; no centroid).

Evidence: `crates/rcad-kernel/tests/it/mass_properties.rs` (box, cylinder, tube, off-centre hole,
hole along X, tilted wedge vs. polygon formulas, frustum with coaxial and off-axis holes vs.
Boole-rule integrals, torus, half ring vs. Pappus, sphere estimate, tetrahedron exactness of
the cubature, Gauss–Legendre exactness), `crates/rcad-commands/tests/it/mass_properties.rs`
(property oracle: 20 × 30 × 40 box at 1.2 g/cm³ = 24 cm³, 28.8 g; centre
of mass after a known translation and rotation; two bodies; hidden/suppressed; density unit
edit; catalog; undo; save/reopen; labelled estimate), `crates/rcad/tests/it/materials_ui.rs`
(inspector widgets).

Assemblies: `mass_properties` also reports component occurrences (`occurrences`, default all
when `bodies` is absent; `crates/rcad-commands/src/mass.rs`). Each occurrence (one entry per
nested instance path) counts its component's visible definition bodies placed by the
occurrence transform (centre of mass moved, inertia rotated), and enters the total; component
bodies are not also counted as loose bodies. Evidence: `crates/rcad-commands/tests/it/assembly.rs`
(`linked_occurrences_follow_the_definition_and_copies_do_not`: three occurrences plus a loose copy), `nested_assembly.rs` (`nested_transforms_bom_and_mass`).

Not done: per-face or layered materials, infill-aware print mass, principal axes / moments
(the tensor is reported; diagonalisation is left to the client).
