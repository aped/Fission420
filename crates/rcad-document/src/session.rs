//! Transactions and undo/redo over the authoritative document.
use crate::regen::{RegenCache, regenerate_cached, regenerate_cached_opts};
use crate::{DocError, Document, Evaluation, FeatureId, Result};
use rcad_kernel::Measurements;
use serde::{Deserialize, Serialize};

/// Record of a committed parameter change (for UIs / logs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamChange {
    pub feature: FeatureId,
    pub name: String,
    pub old: f64,
    pub new: f64,
}

/// A document plus its current evaluation and history. Every edit goes through
/// [`Model::transact`]: the edit is applied to a copy, regenerated, and committed only
/// if regeneration succeeds. On error the previous document and evaluation are kept.
pub struct Model {
    doc: Document,
    eval: Evaluation,
    undo: Vec<(String, Document)>,
    redo: Vec<(String, Document)>,
    /// Bumped on every committed change, undo, or redo (lets UIs refresh caches).
    pub revision: u64,
    /// Changes since the last save/load.
    pub dirty: bool,
    /// Memo of regenerated history prefixes (geometry cache only; never authoritative).
    cache: RegenCache,
    /// Skip B-rep measurement during edits (an interactive client measures out of
    /// band and hands the result back with [`Model::adopt_measurements`]).
    defer_measurements: bool,
    /// Revision whose deferred measurement failed (no longer pending; the
    /// measurements stay unavailable for that revision).
    measure_failed: Option<u64>,
}

/// Cap on undo depth; documents are small, so this is generous.
const MAX_UNDO: usize = 256;

impl Model {
    pub fn new() -> Self {
        let doc = Document::new();
        let eval = Evaluation::empty();
        Self {
            doc,
            eval,
            undo: vec![],
            redo: vec![],
            revision: 0,
            dirty: false,
            cache: RegenCache::default(),
            defer_measurements: false,
            measure_failed: None,
        }
    }

    /// Build from a loaded document; fails (without side effects) if it does not regenerate.
    pub fn from_document(mut doc: Document) -> Result<Self> {
        crate::params::apply_expressions(&mut doc)?;
        let mut cache = RegenCache::default();
        let eval = regenerate_cached(&doc, Some(&mut cache))?;
        Ok(Self {
            doc,
            eval,
            undo: vec![],
            redo: vec![],
            revision: 0,
            dirty: false,
            cache,
            defer_measurements: false,
            measure_failed: None,
        })
    }

    /// See the `defer_measurements` field. Headless clients keep the default (off).
    pub fn set_defer_measurements(&mut self, on: bool) {
        self.defer_measurements = on;
    }

    /// Some B-rep body has not been measured yet (deferred measurement).
    pub fn measurements_pending(&self) -> bool {
        !self.eval.unmeasured().is_empty() && self.measure_failed != Some(self.revision)
    }

    /// Record that the deferred measurement of `revision` failed, so it is no
    /// longer pending (clients stop waiting; measurements stay unavailable).
    pub fn fail_measurements(&mut self, revision: u64) -> bool {
        if revision != self.revision || !self.measurements_pending() {
            return false;
        }
        self.measure_failed = Some(revision);
        true
    }

    /// Bodies whose out-of-band measurement of `revision` failed: they get a
    /// stable error state (shown, not retried until their geometry changes)
    /// and stop being pending; other bodies are unaffected.
    pub fn fail_body_measurements(
        &mut self,
        revision: u64,
        failures: Vec<(crate::BodyId, String)>,
    ) -> bool {
        if revision != self.revision {
            return false;
        }
        let mut any = false;
        for (id, why) in failures {
            let pending = self
                .eval
                .body_by_id(id)
                .is_some_and(|b| b.measurements.is_none() && b.measure_error.is_none());
            if pending {
                self.cache
                    .store_failure(&self.doc, &self.eval, id, why.clone());
                self.eval.set_body_failure(id, why);
                any = true;
            }
        }
        any
    }

    /// Install per-body measurements computed out of band for `revision`.
    /// Ignored (false) if the model has moved on or nothing was pending.
    pub fn adopt_measurements(
        &mut self,
        revision: u64,
        measured: Vec<(crate::BodyId, Measurements)>,
    ) -> bool {
        if revision != self.revision || !self.measurements_pending() {
            return false;
        }
        let mut any = false;
        for (id, m) in measured {
            let pending = self
                .eval
                .body_by_id(id)
                .is_some_and(|b| b.measurements.is_none() && b.geometry.brep().is_some());
            if pending {
                self.cache
                    .store_measurements(&self.doc, &self.eval, id, m.clone());
                self.eval.set_body_measurements(id, m);
                any = true;
            }
        }
        any
    }

    /// Measure synchronously if measurement was deferred (Save, Measure).
    pub fn complete_measurements(&mut self) -> Result<()> {
        if self.measurements_pending() {
            self.eval = regenerate_cached(&self.doc, Some(&mut self.cache))?;
        }
        Ok(())
    }

    fn regen(&mut self, doc: &Document) -> Result<Evaluation> {
        regenerate_cached_opts(doc, Some(&mut self.cache), !self.defer_measurements)
    }

    /// (hits, misses) of the regeneration cache, for diagnostics.
    pub fn cache_stats(&self) -> (u64, u64) {
        (self.cache.hits, self.cache.misses)
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn evaluation(&self) -> &Evaluation {
        &self.eval
    }

    /// Apply `edit` transactionally. `label` names the history entry.
    pub fn transact<T>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut Document) -> Result<T>,
    ) -> Result<T> {
        let mut candidate = self.doc.clone();
        let out = edit(&mut candidate)?;
        // Bound values follow their expressions in the same transaction.
        crate::params::apply_expressions(&mut candidate)?;
        let eval = self.regen(&candidate)?;
        let old = std::mem::replace(&mut self.doc, candidate);
        self.eval = eval;
        self.undo.push((label.to_string(), old));
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.revision += 1;
        self.dirty = true;
        Ok(out)
    }

    pub fn set_parameter(
        &mut self,
        feature: FeatureId,
        name: &str,
        value: f64,
    ) -> Result<ParamChange> {
        let label = format!("Set {feature}.{name} = {value}");
        self.transact(&label, |d| {
            let f = d.feature_mut(feature)?;
            if let Some(e) = f.expressions.get(name) {
                return Err(DocError::Validation(format!(
                    "{feature}.{name} is bound to the expression '{e}'; edit the expression \
                     (or replace it with a number to unbind it)"
                )));
            }
            let old = f
                .parameters()
                .into_iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v)
                .ok_or(DocError::UnknownParameter {
                    feature,
                    name: name.into(),
                })?;
            f.set_parameter(name, value)?;
            Ok(ParamChange {
                feature,
                name: name.into(),
                old,
                new: value,
            })
        })
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(l, _)| l.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|(l, _)| l.as_str())
    }

    /// Undo the last committed change. Returns its label.
    pub fn undo(&mut self) -> Result<String> {
        let (label, prev) = self.undo.pop().ok_or(DocError::NothingTo("undo".into()))?;
        match self.regen(&prev) {
            Ok(eval) => {
                let cur = std::mem::replace(&mut self.doc, prev);
                self.eval = eval;
                self.redo.push((label.clone(), cur));
                self.revision += 1;
                self.dirty = true;
                Ok(label)
            }
            Err(e) => {
                self.undo.push((label, prev));
                Err(e)
            }
        }
    }

    /// Redo the last undone change. Returns its label.
    pub fn redo(&mut self) -> Result<String> {
        let (label, next) = self.redo.pop().ok_or(DocError::NothingTo("redo".into()))?;
        match self.regen(&next) {
            Ok(eval) => {
                let cur = std::mem::replace(&mut self.doc, next);
                self.eval = eval;
                self.undo.push((label.clone(), cur));
                self.revision += 1;
                self.dirty = true;
                Ok(label)
            }
            Err(e) => {
                self.redo.push((label, next));
                Err(e)
            }
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}
