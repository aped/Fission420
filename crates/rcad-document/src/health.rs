//! Per-feature health of an evaluation (docs/behavior/health.md).
//!
//! Regeneration records one [`FeatureHealth`] per feature. A feature that
//! fails does not stop regeneration: later features that do not depend on it
//! (explicitly or through a body it touches) are still evaluated; dependents
//! are marked [`HealthState::Error`] with `upstream` naming the failed feature
//! and are not evaluated, so no geometry is ever built on a state other than
//! the one the history describes.
use crate::FeatureId;
use serde::{Deserialize, Serialize};

/// Health of one feature after regeneration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HealthState {
    /// Evaluated without problems.
    #[default]
    Healthy,
    /// Evaluated, but something deserves attention (e.g. an under-defined
    /// sketch, an unused profile, a stale cached value).
    Warning,
    /// Not evaluated: it failed itself, or depends on a feature that failed.
    Error,
    /// Suppressed by the user: in history, not evaluated.
    Suppressed,
    /// After the timeline marker: in history, not evaluated.
    RolledBack,
}

/// Health of one feature with the reason and an optional repair hint.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FeatureHealth {
    pub state: HealthState,
    /// What is wrong (None when healthy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// How to repair it, when known ("edit F3 and pick the face again").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Error caused by a failed upstream feature (that feature's id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<FeatureId>,
    /// The structured error of a failed feature (None for dependents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::DocError>,
}

impl FeatureHealth {
    pub fn healthy() -> Self {
        Self::default()
    }

    pub fn is_error(&self) -> bool {
        self.state == HealthState::Error
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            state: HealthState::Warning,
            message: Some(message.into()),
            ..Self::default()
        }
    }

    pub fn failed(error: crate::DocError, hint: Option<String>) -> Self {
        Self {
            state: HealthState::Error,
            message: Some(error.to_string()),
            hint,
            upstream: None,
            error: Some(error),
        }
    }

    pub fn upstream(failed: FeatureId) -> Self {
        Self {
            state: HealthState::Error,
            message: Some(format!("not evaluated: depends on {failed}, which failed")),
            hint: Some(format!("repair {failed} first")),
            upstream: Some(failed),
            error: None,
        }
    }
}
