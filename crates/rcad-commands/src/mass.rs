//! Mass properties of bodies and of a set of bodies (`mass_properties`, run 6).
//!
//! Frame and units: world coordinates; lengths mm, volume mm³, density g/cm³,
//! mass g, inertia g·mm² (about the centre of mass, world axes). Geometry comes from
//! `rcad_kernel::volume_properties` (exact boundary integrals where the volume is
//! exact, else a labelled tessellation estimate) or, for mesh bodies,
//! `rcad_kernel::mesh_volume_properties` (exact for the mesh).
use rcad_document::{BodyGeometry, BodyId, DocError, Evaluation, Material, Model};
use rcad_kernel::{self as k, VolumeProperties};
use serde::{Deserialize, Serialize};

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
}

impl BodyMass {
    /// Centre of mass (= centroid at uniform density), world mm.
    pub fn center_of_mass(&self) -> Option<[f64; 3]> {
        self.properties.as_ref().map(|p| p.centroid)
    }
}

/// Properties of `bodies` (None: every visible body of the evaluation).
pub fn report(model: &Model, bodies: Option<&[BodyId]>) -> Result<MassReport, DocError> {
    let ev: &Evaluation = model.evaluation();
    let doc = model.document();
    let mut excluded_hidden = Vec::new();
    let ids: Vec<BodyId> = match bodies {
        Some(list) => {
            if list.is_empty() {
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
            .filter(|b| {
                if !b.visible {
                    excluded_hidden.push(b.id);
                }
                b.visible
            })
            .map(|b| b.id)
            .collect(),
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(b) = ev.body_by_id(id) else { continue };
        let material = doc.body(id).ok().and_then(|i| i.material.clone());
        let props = match &b.geometry {
            BodyGeometry::Brep(g) => k::volume_properties(g).map_err(|e| e.to_string()),
            BodyGeometry::Mesh(m) if m.diagnostics.closed => {
                k::mesh_volume_properties(&m.mesh).map_err(|e| e.to_string())
            }
            BodyGeometry::Mesh(_) => {
                Err("the mesh is not closed: no volume or centre of mass".to_string())
            }
        };
        let (properties, error) = match props {
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
    let total = (total_missing.is_empty() && !out.is_empty()).then(|| combine(&out));
    Ok(MassReport {
        frame: MASS_FRAME.into(),
        units: MASS_UNITS.into(),
        bodies: out,
        total: total.flatten(),
        total_missing,
        excluded_hidden,
    })
}

/// Totals: masses add, the centre of mass is the mass-weighted mean, inertia tensors
/// are moved to it with the parallel-axis theorem I += m (|d|² E − d dᵀ).
fn combine(bodies: &[BodyMass]) -> Option<MassTotal> {
    let mut mass = 0.0;
    let mut mass_error = 0.0;
    let mut volume = 0.0;
    let mut mc = [0.0; 3];
    for b in bodies {
        let (m, p) = (b.mass?, b.properties.as_ref()?);
        mass += m;
        mass_error += b.mass_error.unwrap_or(0.0);
        volume += p.volume;
        for (acc, c) in mc.iter_mut().zip(p.centroid) {
            *acc += m * c;
        }
    }
    if mass.is_nan() || mass <= 0.0 {
        return None;
    }
    let com = mc.map(|x| x / mass);
    let mut inertia = [[0.0; 3]; 3];
    for b in bodies {
        let (m, p, ib) = (b.mass?, b.properties.as_ref()?, b.inertia?);
        let d = [0, 1, 2].map(|i| p.centroid[i] - com[i]);
        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        for i in 0..3 {
            for j in 0..3 {
                let e = if i == j { d2 } else { 0.0 };
                inertia[i][j] += ib[i][j] + m * (e - d[i] * d[j]);
            }
        }
    }
    Some(MassTotal {
        bodies: bodies.iter().map(|b| b.body).collect(),
        volume,
        mass,
        mass_error,
        center_of_mass: com,
        inertia,
        exact: bodies.iter().all(|b| b.exact),
    })
}
