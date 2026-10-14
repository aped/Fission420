//! Physical material of a body (format v6): a name and a uniform density
//! with its provenance. Separate from appearance (colour); used only for mass
//! properties. docs/MATERIALS.md documents the catalog and its uncertainty.
//!
//! Densities are stored in g/cm³ in the document. A catalog material copies the
//! catalog's density into the body at assignment, so a later catalog revision never
//! silently changes the mass of a saved document; `catalog` records where it came from.
use crate::{DocError, Result};
use serde::{Deserialize, Serialize};

/// Smallest and largest accepted density (g/cm³). The densest element (osmium) is
/// about 22.6 g/cm³; anything outside this range is a unit mistake.
pub const MIN_DENSITY: f64 = 0.001;
pub const MAX_DENSITY: f64 = 30.0;

/// A body's material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// Display name ("PLA", or the user's name for a user-defined density).
    pub name: String,
    /// Uniform density, g/cm³ (= 1e-3 g/mm³).
    pub density: f64,
    /// Catalog key when the material was taken from [`CATALOG`] (provenance);
    /// None: a user-defined density.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
}

/// One catalog entry. Values are typical nominal densities of the solid material,
/// not a supplier specification (see docs/MATERIALS.md).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CatalogEntry {
    pub key: &'static str,
    pub name: &'static str,
    /// g/cm³.
    pub density: f64,
    /// Typical spread of published values, g/cm³ (min, max).
    pub range: (f64, f64),
}

/// The built-in density catalog: typical nominal values ("typical nominal" in the UI).
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        key: "pla",
        name: "PLA",
        density: 1.24,
        range: (1.21, 1.26),
    },
    CatalogEntry {
        key: "petg",
        name: "PETG",
        density: 1.27,
        range: (1.23, 1.29),
    },
    CatalogEntry {
        key: "abs",
        name: "ABS",
        density: 1.04,
        range: (1.01, 1.08),
    },
    CatalogEntry {
        key: "asa",
        name: "ASA",
        density: 1.07,
        range: (1.05, 1.10),
    },
    CatalogEntry {
        key: "pa12",
        name: "Nylon PA12",
        density: 1.01,
        range: (1.00, 1.03),
    },
    CatalogEntry {
        key: "tpu",
        name: "TPU 95A",
        density: 1.21,
        range: (1.18, 1.24),
    },
    CatalogEntry {
        key: "al6061",
        name: "Aluminium 6061",
        density: 2.70,
        range: (2.69, 2.71),
    },
    CatalogEntry {
        key: "steel",
        name: "Carbon steel",
        density: 7.85,
        range: (7.80, 7.87),
    },
    CatalogEntry {
        key: "ss304",
        name: "Stainless steel 304",
        density: 8.00,
        range: (7.90, 8.03),
    },
    CatalogEntry {
        key: "brass",
        name: "Brass C360",
        density: 8.50,
        range: (8.40, 8.55),
    },
];

/// Catalog entry by key (case-insensitive).
pub fn catalog_entry(key: &str) -> Option<&'static CatalogEntry> {
    let k = key.trim();
    CATALOG.iter().find(|e| e.key.eq_ignore_ascii_case(k))
}

/// Unit of a density given in a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DensityUnit {
    /// g/cm³ (= kg/L = 1000 kg/m³).
    #[default]
    GPerCm3,
    /// kg/m³.
    KgPerM3,
}

impl DensityUnit {
    /// Value in g/cm³.
    pub fn to_g_per_cm3(self, v: f64) -> f64 {
        match self {
            DensityUnit::GPerCm3 => v,
            DensityUnit::KgPerM3 => v / 1000.0,
        }
    }
}

impl Material {
    /// The material of a catalog entry.
    pub fn from_catalog(key: &str) -> Result<Material> {
        let e = catalog_entry(key).ok_or_else(|| {
            DocError::Validation(format!(
                "unknown material '{key}' (catalog: {})",
                CATALOG.iter().map(|e| e.key).collect::<Vec<_>>().join(", ")
            ))
        })?;
        Ok(Material {
            name: e.name.into(),
            density: e.density,
            catalog: Some(e.key.into()),
        })
    }

    /// A user-defined density (`density` in `unit`), named `name` (default
    /// "User-defined").
    pub fn user(name: Option<&str>, density: f64, unit: DensityUnit) -> Result<Material> {
        let m = Material {
            name: name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or("User-defined")
                .to_string(),
            density: unit.to_g_per_cm3(density),
            catalog: None,
        };
        m.validate()?;
        Ok(m)
    }

    /// Finite density within [`MIN_DENSITY`, `MAX_DENSITY`] g/cm³, a sane name, and
    /// a known catalog key when one is recorded.
    pub fn validate(&self) -> Result<()> {
        let d = self.density;
        if !(d.is_finite() && (MIN_DENSITY..=MAX_DENSITY).contains(&d)) {
            return Err(DocError::Validation(format!(
                "density must be between {MIN_DENSITY} and {MAX_DENSITY} g/cm³, got {d} g/cm³ \
                 (kg/m³ values need density_unit \"kg_per_m3\")"
            )));
        }
        let n = self.name.trim();
        if n.is_empty() || n.chars().count() > 64 || n.chars().any(char::is_control) {
            return Err(DocError::Validation(
                "material name must be 1-64 printable characters".into(),
            ));
        }
        if let Some(k) = &self.catalog
            && catalog_entry(k).is_none()
        {
            return Err(DocError::Validation(format!(
                "unknown catalog material '{k}'"
            )));
        }
        Ok(())
    }

    /// Density in g/mm³ (mass in g from a volume in mm³).
    pub fn g_per_mm3(&self) -> f64 {
        self.density * 1e-3
    }

    /// "PLA, 1.24 g/cm³ (catalog, typical nominal)" / "..., user-defined".
    pub fn describe(&self) -> String {
        format!(
            "{}, {} g/cm³ ({})",
            self.name,
            self.density,
            if self.catalog.is_some() {
                "catalog, typical nominal"
            } else {
                "user-defined"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        for e in CATALOG {
            assert!(
                e.range.0 <= e.density && e.density <= e.range.1,
                "{}",
                e.key
            );
            assert_eq!(e.key, e.key.to_ascii_lowercase());
            Material::from_catalog(e.key).unwrap().validate().unwrap();
        }
        assert_eq!(Material::from_catalog("PLA").unwrap().density, 1.24);
        assert!(Material::from_catalog("unobtainium").is_err());
    }

    #[test]
    fn user_density_units_and_limits() {
        let m = Material::user(None, 1200.0, DensityUnit::KgPerM3).unwrap();
        assert_eq!(m.density, 1.2);
        assert_eq!(m.name, "User-defined");
        assert!(Material::user(None, 1200.0, DensityUnit::GPerCm3).is_err());
        assert!(Material::user(None, 0.0, DensityUnit::GPerCm3).is_err());
        assert!(Material::user(None, f64::NAN, DensityUnit::GPerCm3).is_err());
        assert!(Material::user(Some("  "), 1.0, DensityUnit::GPerCm3).is_ok());
    }
}
