//! Length and plane-angle units from GLOBAL_UNIT_ASSIGNED_CONTEXT.
//!
//! Supported: SI_UNIT(prefix, .METRE.) with any SI prefix, SI_UNIT(.., .RADIAN.), and
//! CONVERSION_BASED_UNIT('name', #measure_with_unit) chains (e.g. INCH = 25.4 mm,
//! DEGREE = pi/180 rad). Every representation context in the file must agree.
use super::ast::{Index, P};
use super::{LengthUnit, StepError, StepImportOptions, fail};

const MAX_UNIT_DEPTH: usize = 8;

fn si_prefix(p: &str) -> Option<f64> {
    Some(match p {
        "EXA" => 1e18,
        "PETA" => 1e15,
        "TERA" => 1e12,
        "GIGA" => 1e9,
        "MEGA" => 1e6,
        "KILO" => 1e3,
        "HECTO" => 1e2,
        "DECA" => 1e1,
        "DECI" => 1e-1,
        "CENTI" => 1e-2,
        "MILLI" => 1e-3,
        "MICRO" => 1e-6,
        "NANO" => 1e-9,
        "PICO" => 1e-12,
        "FEMTO" => 1e-15,
        "ATTO" => 1e-18,
        _ => return None,
    })
}

fn prefix_name(p: &str) -> String {
    p.to_ascii_lowercase()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Length,
    Angle,
}

/// (name, factor) with factor = mm per unit (length) or rad per unit (angle).
fn unit_value(idx: &Index, id: u64, kind: Kind, depth: usize) -> Result<(String, f64), StepError> {
    if depth > MAX_UNIT_DEPTH {
        return Err(fail(format!("unit #{id}: definition nested too deep")));
    }
    let e = idx.get(id)?;
    let (base, base_name) = match kind {
        Kind::Length => (1000.0, "METRE"),
        Kind::Angle => (1.0, "RADIAN"),
    };
    if let Some(si) = e.part("SI_UNIT") {
        // SI_UNIT(prefix, name) in a complex instance; the simple form has a leading
        // derived dimensions parameter in some writers, so take the last two.
        let n = si.len();
        if n < 2 {
            return Err(fail(format!("SI_UNIT #{id}: malformed")));
        }
        let p = P::new(id, "SI_UNIT", &si[n - 2..]);
        let name = p.e(1)?;
        if name != base_name {
            return Err(fail(format!(
                "SI_UNIT #{id}: expected {base_name}, found {name}"
            )));
        }
        let (f, pname) = if p.is_unset(0) {
            (1.0, String::new())
        } else {
            let pre = p.e(0)?;
            (
                si_prefix(pre)
                    .ok_or_else(|| fail(format!("SI_UNIT #{id}: unknown prefix {pre}")))?,
                prefix_name(pre),
            )
        };
        return Ok((format!("{pname}{}", name.to_ascii_lowercase()), f * base));
    }
    if let Some(cb) = e.part("CONVERSION_BASED_UNIT") {
        let p = P::new(id, "CONVERSION_BASED_UNIT", cb);
        let name = p.s(0)?.trim().to_ascii_lowercase();
        let mwu = p.r(1)?;
        let m = idx.get(mwu)?;
        let mp = m
            .part("LENGTH_MEASURE_WITH_UNIT")
            .or(m.part("PLANE_ANGLE_MEASURE_WITH_UNIT"))
            .or(m.part("MEASURE_WITH_UNIT"))
            .ok_or_else(|| {
                fail(format!(
                    "CONVERSION_BASED_UNIT #{id}: #{mwu} is {}, expected a measure with unit",
                    m.type_name()
                ))
            })?;
        let mp = P::new(mwu, "MEASURE_WITH_UNIT", mp);
        let value = mp.f(0)?;
        let (_, f) = unit_value(idx, mp.r(1)?, kind, depth + 1)?;
        let v = value * f;
        if !(v.is_finite() && v > 0.0) {
            return Err(fail(format!("CONVERSION_BASED_UNIT #{id}: invalid factor")));
        }
        return Ok((name, v));
    }
    Err(fail(format!(
        "unsupported unit entity {} #{id}",
        e.type_name()
    )))
}

/// Length unit and plane-angle factor (rad per file unit) of the file.
pub(super) fn units(
    idx: &Index,
    opts: &StepImportOptions,
    warnings: &mut Vec<String>,
) -> Result<(LengthUnit, f64), StepError> {
    let mut length: Option<(String, f64)> = None;
    let mut angle: Option<f64> = None;
    for ctx in idx.all_with("GLOBAL_UNIT_ASSIGNED_CONTEXT") {
        let e = idx.get(ctx)?;
        let p = P::new(
            ctx,
            "GLOBAL_UNIT_ASSIGNED_CONTEXT",
            e.part("GLOBAL_UNIT_ASSIGNED_CONTEXT").unwrap_or(&[]),
        );
        for u in p.refs(0)? {
            let ue = idx.get(u)?;
            if ue.has("LENGTH_UNIT") {
                let v = unit_value(idx, u, Kind::Length, 0)?;
                match &length {
                    Some(l) if (l.1 - v.1).abs() > 1e-12 * l.1 => {
                        return Err(fail(format!(
                            "conflicting length units in the file ({} and {})",
                            l.0, v.0
                        )));
                    }
                    Some(_) => {}
                    None => length = Some(v),
                }
            } else if ue.has("PLANE_ANGLE_UNIT") {
                let (_, v) = unit_value(idx, u, Kind::Angle, 0)?;
                match angle {
                    Some(a) if (a - v).abs() > 1e-12 * a => {
                        return Err(fail("conflicting plane angle units in the file".into()));
                    }
                    _ => angle = Some(v),
                }
            }
        }
    }
    let length = match length {
        Some((name, to_mm)) => LengthUnit {
            name,
            to_mm,
            declared: true,
        },
        None => match opts.unit_override_to_mm {
            Some(k) if k.is_finite() && k > 0.0 => {
                warnings.push(format!(
                    "file declares no length unit; using the chosen {k} mm per unit"
                ));
                LengthUnit {
                    name: format!("{k} mm (chosen)"),
                    to_mm: k,
                    declared: false,
                }
            }
            Some(k) => return Err(fail(format!("invalid unit override {k}"))),
            None => {
                return Err(fail(
                    "units required: the file declares no length unit; choose one".into(),
                ));
            }
        },
    };
    let angle = match angle {
        Some(a) => a,
        None => {
            warnings.push("file declares no plane angle unit; assuming radians".into());
            1.0
        }
    };
    Ok((length, angle))
}
