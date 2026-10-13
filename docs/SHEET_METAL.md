# Sheet metal (run 6, bounded subset)

`FeatureKind::SheetMetal` / command `sheet_metal`: a **contour flange** from an open chain of
sketch lines. Exact geometry: the section (lines and circular arcs) is extruded `width` along the
sketch normal.

* The chain is one surface of the sheet. Material lies to the **right** of the chain's direction
  (`flip`: left). The chain starts at the end of the lowest-numbered line.
* Every interior vertex becomes a cylindrical bend of **inner radius** `radius`. Where the material
  is outside the turn the chain is the inner surface (radius R); where it is inside, the chain is
  the outer surface (radius R + t). Straight flange lengths are measured **between tangent
  points**: a chain segment of length L with bends of setback s = r·tan(θ/2) at its ends keeps
  L − s₀ − s₁.
* Flat pattern: neutral axis at `k_factor`·t from the inner surface (default K = 0.44);
  L_flat = Σ straights + Σ θ (R + K t) (bend allowance; no deduction tables, no relief cuts).
  `export_flat_pattern` writes an R12 DXF in mm: the outline on layer `CUT`, bend centre lines on
  layer `BEND`; the response reports the flat length.
* Refused with a reason: arcs/circles/branches/closed chains in the sketch, a flange too short for
  its bends, 180° folds (hems), collinear vertices, self-overlapping sections, thickness/radius ≤ 0,
  K outside [0, 1].

Not implemented: base face from a closed profile, edge flanges on existing bodies, reliefs,
hems, unfold/refold of arbitrary solids, K-factor tables per material, flat-pattern drawings.
Oracle: `crates/rcad-commands/tests/sheet_metal.rs` (U-channel 60+20+20, t 1, R 2, K 0.4 →
107.539822 mm; thickness/radius edits; DXF read back by an independent parser in the test).
