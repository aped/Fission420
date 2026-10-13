//! Mass properties of bodies and of a set of bodies (`mass_properties`, run 6).
//!
//! Frame and units: world coordinates; lengths mm, volume mm³, density g/cm³,
//! mass g, inertia g·mm² (about the centre of mass, world axes). Geometry comes from
//! `rcad_kernel::volume_properties` (exact boundary integrals where the volume is
//! exact, else a labelled tessellation estimate) or, for mesh bodies,
//! `rcad_kernel::mesh_volume_properties` (exact for the mesh).
use rcad_document::assembly::{ComponentId, OccurrenceId};
use rcad_document::{BodyGeometry, BodyId, DocError, Evaluation, Material, Model};
use rcad_kernel::{self as k, VolumeProperties};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type M3 = [[f64; 3]; 3];

/// Units of every [`MassReport`] value (fixed; stated for clients and files).
pub const MASS_UNITS: &str = "lengths mm, volume mm³, density g/cm³, mass g, inertia g·mm²";
/// Coordinate frame of every position and tensor in a [`MassReport`].
pub const MASS_FRAME: &str = "world";

/// One body's properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyMass {
    pub body: BodyId,
    pub name: String,
    pub visible: bool,
    pub material: Option<Material>,
    /// Volume, centroid and central second moments (mm, mm³, mm⁵) with the
    /// method and its error estimates; None when they could not be computed
    /// (`error`).
    pub properties: Option<VolumeProperties>,
    /// True when `properties` come from an exact boundary integral.
    pub exact: bool,
    /// g (needs a material and properties).
    pub mass: Option<f64>,
    /// g, from the volume error estimate.
    pub mass_error: Option<f64>,
    /// Inertia tensor about the centre of mass, world axes, g·mm².
    pub inertia: Option<M3>,
    /// Largest component error, g·mm².
    pub inertia_error: Option<f64>,
    pub error: Option<String>,
}

/// Totals over the listed bodies (present only when every listed body has a
/// material and properties).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MassTotal {
    pub bodies: Vec<BodyId>,
    /// mm³.
    pub volume: f64,
    /// g.
    pub mass: f64,
    /// g (sum of the bodies' mass errors).
    pub mass_error: f64,
    /// Combined centre of mass, world mm.
    pub center_of_mass: [f64; 3],
    /// About `center_of_mass`, world axes (parallel-axis theorem), g·mm².
    pub inertia: M3,
    /// Every body's properties are exact.
    pub exact: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MassReport {
    /// [`MASS_FRAME`].
    pub frame: String,
    /// [`MASS_UNITS`].
    pub units: String,
    pub bodies: Vec<BodyMass>,
    pub total: Option<MassTotal>,
    /// Why there is no total (bodies without material or properties).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub total_missing: Vec<String>,
    /// Hidden bodies left out because no list was given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_hidden: Vec<BodyId>,
    /// Component occurrences (docs/ASSEMBLIES.md): each counts its component's
    /// visible evaluated definition bodies, placed. Component bodies are not
    /// also counted as loose bodies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub occurrences: Vec<OccurrenceMass>,
}

/// One occurrence's mass properties (world frame).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceMass {
    pub occurrence: OccurrenceId,
    /// Run 7: instance path ("O3/O7"); `name` is the path of names.
    #[serde(default)]
    pub path: String,
    pub name: String,
    pub component: ComponentId,
    /// mm³ (None when a body has no properties).
    pub volume: Option<f64>,
    pub mass: Option<f64>,
    pub mass_error: Option<f64>,
    pub center_of_mass: Option<[f64; 3]>,
    /// About `center_of_mass`, world axes, g·mm².
    pub inertia: Option<M3>,
    pub exact: bool,
    pub error: Option<String>,
}

impl BodyMass {
    /// Centre of mass (= centroid at uniform density), world mm.
    pub fn center_of_mass(&self) -> Option<[f64; 3]> {
        self.properties.as_ref().map(|p| p.centroid)
    }
}

/// One mass-carrying part for [`combine`].
#[derive(Clone, Copy)]
struct Part {
    mass: f64,
    mass_error: f64,
    volume: f64,
    com: [f64; 3],
    inertia: M3,
    exact: bool,
}

fn body_props(b: &rcad_document::EvalBody) -> Result<VolumeProperties, String> {
    match &b.geometry {
        BodyGeometry::Brep(g) => k::volume_properties(g).map_err(|e| e.to_string()),
        BodyGeometry::Mesh(m) if m.diagnostics.closed => {
            k::mesh_volume_properties(&m.mesh).map_err(|e| e.to_string())
        }
        BodyGeometry::Mesh(_) => Err("the mesh is not closed: no volume or centre of mass".into()),
    }
}

/// Rotate a tensor: R S Rᵀ.
fn rotate(r: &M3, s: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let mut x = 0.0;
            for a in 0..3 {
                for b in 0..3 {
                    x += r[i][a] * s[a][b] * r[j][b];
                }
            }
            out[i][j] = x;
        }
    }
    out
}

/// Properties of `bodies` (None: every visible body of the evaluation that is
/// not a component body) and of `occurrences` (None with `bodies` None: every
/// occurrence; None with `bodies` given: none), with their total.
pub fn report(
    model: &Model,
    bodies: Option<&[BodyId]>,
    occurrences: Option<&[OccurrenceId]>,
) -> Result<MassReport, DocError> {
    let ev: &Evaluation = model.evaluation();
    let doc = model.document();
    let asm = &doc.assembly;
    let mut excluded_hidden = Vec::new();
    let ids: Vec<BodyId> = match bodies {
        Some(list) => {
            if list.is_empty() && occurrences.is_none_or(<[OccurrenceId]>::is_empty) {
                return Err(DocError::Validation("no bodies listed".into()));
            }
            let mut out: Vec<BodyId> = Vec::new();
            for &id in list {
                if ev.body_by_id(id).is_none() {
                    return Err(DocError::Validation(match doc.body(id) {
                        Ok(b) => format!(
                            "body {id} ({}) is not in the evaluated model (suppressed, rolled back or consumed by a combine)",
                            b.name
                        ),
                        Err(_) => format!("unknown body {id}"),
                    }));
                }
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            out
        }
        None => ev
            .bodies
            .iter()
            .filter(|b| asm.component_of(b.id).is_none())
            .filter(|b| {
                if !b.visible {
                    excluded_hidden.push(b.id);
                }
                b.visible
            })
            .map(|b| b.id)
            .collect(),
    };
    // Run 7 (nesting): one entry per evaluated instance; a listed occurrence
    // selects every instance whose path passes through it.
    let occ_ids: Vec<usize> = match (occurrences, bodies) {
        (Some(list), _) => {
            for o in list {
                asm.occurrence(*o)?;
            }
            (0..ev.occurrences.len())
                .filter(|&i| ev.occurrences[i].path.iter().any(|p| list.contains(p)))
                .collect()
        }
        (None, None) => (0..ev.occurrences.len()).collect(),
        (None, Some(_)) => vec![],
    };
    // Geometry properties per body, computed once (occurrences share them).
    let mut cache: BTreeMap<BodyId, Result<VolumeProperties, String>> = BTreeMap::new();
    let mut props_of =
        |b: &rcad_document::EvalBody| cache.entry(b.id).or_insert_with(|| body_props(b)).clone();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(b) = ev.body_by_id(id) else { continue };
        let material = doc.body(id).ok().and_then(|i| i.material.clone());
        let (properties, error) = match props_of(b) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };
        let rho = material.as_ref().map(Material::g_per_mm3);
        let (mass, mass_error, inertia, inertia_error) = match (&properties, rho) {
            (Some(p), Some(r)) => (
                Some(r * p.volume),
                Some(r * p.volume_error),
                Some(p.inertia(r)),
                Some(2.0 * r * p.second_moments_error),
            ),
            _ => (None, None, None, None),
        };
        out.push(BodyMass {
            body: id,
            name: b.name.clone(),
            visible: b.visible,
            exact: properties.as_ref().is_some_and(VolumeProperties::is_exact),
            material,
            properties,
            mass,
            mass_error,
            inertia,
            inertia_error,
            error,
        });
    }
    let mut total_missing = Vec::new();
    for b in &out {
        if let Some(e) = &b.error {
            total_missing.push(format!("{}: {e}", b.name));
        } else if b.material.is_none() {
            total_missing.push(format!("{}: no material assigned", b.name));
        }
    }
    let mut parts: Vec<Part> = out
        .iter()
        .filter_map(|b| {
            let p = b.properties.as_ref()?;
            Some(Part {
                mass: b.mass?,
                mass_error: b.mass_error.unwrap_or(0.0),
                volume: p.volume,
                com: p.centroid,
                inertia: b.inertia?,
                exact: b.exact,
            })
        })
        .collect();
    let mut occ_out = Vec::new();
    for oi in occ_ids {
        let Some(eo) = ev.occurrences.get(oi) else {
            continue;
        };
        let t = eo.transform;
        let mut sub: Vec<Part> = Vec::new();
        let mut volume = 0.0;
        let mut errors = Vec::new();
        let mut vol_only = Vec::new();
        for bid in &eo.bodies {
            let Some(b) = ev.body_by_id(*bid) else {
                continue;
            };
            if !b.visible {
                if !excluded_hidden.contains(bid) {
                    excluded_hidden.push(*bid);
                }
                continue;
            }
            let p = match props_of(b) {
                Ok(p) => p,
                Err(e) => {
                    errors.push(format!("{}: {e}", b.name));
                    continue;
                }
            };
            volume += p.volume;
            let com = t.point(p.centroid);
            vol_only.push((p.volume, com, p.is_exact()));
            match doc.body(*bid).ok().and_then(|i| i.material.as_ref()) {
                Some(mat) => {
                    let r = mat.g_per_mm3();
                    let ib = rotate(&t.r, &p.inertia(r));
                    sub.push(Part {
                        mass: r * p.volume,
                        mass_error: r * p.volume_error,
                        volume: p.volume,
                        com,
                        inertia: ib,
                        exact: p.is_exact(),
                    });
                }
                None => errors.push(format!("{}: no material assigned", b.name)),
            }
        }
        let exact = vol_only.iter().all(|x| x.2);
        let whole = (errors.is_empty() && !sub.is_empty())
            .then(|| combine(&sub))
            .flatten();
        let center = match &whole {
            Some(w) => Some(w.com),
            None if !vol_only.is_empty() => {
                let v: f64 = vol_only.iter().map(|x| x.0).sum();
                Some(std::array::from_fn(|i| {
                    vol_only.iter().map(|x| x.0 * x.1[i]).sum::<f64>() / v
                }))
            }
            None => None,
        };
        if !errors.is_empty() {
            total_missing.push(format!("{}: {}", eo.name, errors.join("; ")));
        }
        if let Some(w) = &whole {
            parts.push(*w);
        }
        occ_out.push(OccurrenceMass {
            occurrence: eo.id,
            path: eo.path_string(),
            name: eo.path_name.clone(),
            component: eo.component,
            volume: (!vol_only.is_empty()).then_some(volume),
            mass: whole.map(|w| w.mass),
            mass_error: whole.map(|w| w.mass_error),
            center_of_mass: center,
            inertia: whole.map(|w| w.inertia),
            exact,
            error: (!errors.is_empty()).then(|| errors.join("; ")),
        });
    }
    let total = (total_missing.is_empty() && !parts.is_empty())
        .then(|| combine(&parts))
        .flatten()
        .map(|w| MassTotal {
            bodies: out.iter().map(|b| b.body).collect(),
            volume: w.volume,
            mass: w.mass,
            mass_error: w.mass_error,
            center_of_mass: w.com,
            inertia: w.inertia,
            exact: w.exact,
        });
    Ok(MassReport {
        frame: MASS_FRAME.into(),
        units: MASS_UNITS.into(),
        bodies: out,
        total,
        total_missing,
        excluded_hidden,
        occurrences: occ_out,
    })
}

/// Totals: masses add, the centre of mass is the mass-weighted mean, inertia tensors
/// are moved to it with the parallel-axis theorem I += m (|d|² E − d dᵀ).
fn combine(parts: &[Part]) -> Option<Part> {
    let mass: f64 = parts.iter().map(|p| p.mass).sum();
    if mass.is_nan() || mass <= 0.0 {
        return None;
    }
    let com: [f64; 3] =
        std::array::from_fn(|i| parts.iter().map(|p| p.mass * p.com[i]).sum::<f64>() / mass);
    let mut inertia = [[0.0; 3]; 3];
    for p in parts {
        let d = [0, 1, 2].map(|i| p.com[i] - com[i]);
        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        for i in 0..3 {
            for j in 0..3 {
                let e = if i == j { d2 } else { 0.0 };
                inertia[i][j] += p.inertia[i][j] + p.mass * (e - d[i] * d[j]);
            }
        }
    }
    Some(Part {
        mass,
        mass_error: parts.iter().map(|p| p.mass_error).sum(),
        volume: parts.iter().map(|p| p.volume).sum(),
        com,
        inertia,
        exact: parts.iter().all(|p| p.exact),
    })
}
