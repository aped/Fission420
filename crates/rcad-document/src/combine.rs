//! Evaluation of [`crate::FeatureKind::Combine`]: validation of the
//! body lists, the kernel combine with its independent check, and the mapping of
//! kernel failures to structured document errors.
use crate::{BodyId, DocError, FeatureId, Result};
use rcad_kernel::{self as k, Body, CombineOp};

/// Most tool bodies in one combine.
pub const MAX_TOOLS: usize = 32;

/// The combined body and notes describing the independent check.
pub(crate) fn evaluate(
    id: FeatureId,
    target: BodyId,
    tools: &[BodyId],
    a: &Body,
    bs: &[Body],
    op: CombineOp,
) -> Result<(Body, Vec<String>)> {
    validate(id, target, tools)?;
    let (out, c) = k::combine(a, bs, op).map_err(|e| match e {
        k::KernelError::InvalidInput(m) => DocError::ProductLimit(format!("{id}: {m}")),
        k::KernelError::InvalidResult(m) if m.contains("independent check failed") => {
            DocError::GeometryCheck {
                feature: id,
                detail: m,
            }
        }
        k::KernelError::InvalidResult(m) => DocError::ProductLimit(format!("{id}: {m}")),
        e @ k::KernelError::BooleanFailed(_) => DocError::Kernel {
            feature: id,
            detail: format!(
                "{e} (Truck 0.6 booleans usually fail where faces of the bodies coincide, e.g. \
                 bodies that only touch or are flush: make them overlap)"
            ),
        },
        other => DocError::Kernel {
            feature: id,
            detail: other.to_string(),
        },
    })?;
    let mut notes = vec![format!(
        "independent check: {} of {} grid samples compared, {} mismatches ({} within {:.3} mm of a surface skipped)",
        c.compared, c.samples, c.mismatches, c.skipped, c.skip_distance
    )];
    if let Some(v) = c.result_volume {
        notes.push(format!(
            "volume {v:.3} mm³ vs grid estimate {:.3} ± {:.3} mm³",
            c.grid_volume, c.volume_tolerance
        ));
    }
    Ok((out, notes))
}

/// Non-empty, distinct tools, none of them the target.
pub fn validate(id: FeatureId, target: BodyId, tools: &[BodyId]) -> Result<()> {
    if tools.is_empty() {
        return Err(DocError::Validation(format!(
            "{id}: combine needs at least one tool body"
        )));
    }
    if tools.len() > MAX_TOOLS {
        return Err(DocError::Validation(format!(
            "{id}: at most {MAX_TOOLS} tool bodies per combine"
        )));
    }
    for (i, t) in tools.iter().enumerate() {
        if *t == target {
            return Err(DocError::Validation(format!(
                "{id}: the target body {target} cannot also be a tool"
            )));
        }
        if tools[..i].contains(t) {
            return Err(DocError::Validation(format!(
                "{id}: tool body {t} is listed twice"
            )));
        }
    }
    Ok(())
}
