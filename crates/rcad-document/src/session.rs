//! Transactions and undo/redo over the authoritative document.
use crate::regen::{RegenCache, regenerate_partial};
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

/// Most separate bodies one cut may split its target into.
pub const MAX_PIECES: usize = 64;
/// Rounds of [`Model::transact`]'s piece assignment (one per splitting cut).
const MAX_PIECE_ROUNDS: usize = 32;

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

/// What [`Model::from_document_reporting`] found while opening a document.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// The parameter expressions do not evaluate: the stored values are shown.
    pub expression_error: Option<DocError>,
    /// Stale entries removed on load (one line each); the model is dirty
    /// when this is non-empty.
    pub cleaned: Vec<String>,
    /// Features that did not regenerate (failed, or depend on a failure):
    /// the document still opens with them flagged in their health.
    pub failures: Vec<(FeatureId, String)>,
    /// Safe-mode open: the timeline marker was moved to just before this
    /// failed feature (the document is then unsaved).
    pub rolled_back_to: Option<FeatureId>,
}

/// Cap on undo depth; documents are small, so this is generous.
const MAX_UNDO: usize = 256;

/// Document, evaluation, undo/redo history and dirty flag of a [`Model`],
/// taken before temporary changes (e.g. activating each configuration to
/// export it) and put back with [`Model::restore`].
#[derive(Debug, Clone)]
pub struct ModelSnapshot {
    doc: Document,
    eval: Evaluation,
    undo: Vec<(String, Document)>,
    redo: Vec<(String, Document)>,
    dirty: bool,
}

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
    pub fn from_document(doc: Document) -> Result<Self> {
        Self::from_document_reporting(doc).map(|(m, _)| m)
    }

    /// [`Model::from_document`], but a document whose expressions no longer
    /// evaluate (e.g. a hand-edited file) still opens with its stored values
    /// (the last committed definition) when those regenerate; the expression
    /// error is reported so the client can show it (otherwise such a file
    /// could not be opened to repair it). The next edit re-derives every
    /// bound value, so it is refused until the named parameter is fixed.
    ///
    /// Stale configuration entries an older build left behind (exclusions of
    /// deleted occurrences; values for parameters that no longer exist) are
    /// dropped and listed in
    /// [`LoadReport::cleaned`]; the model is then dirty, so saving keeps the
    /// cleaned state.
    pub fn from_document_reporting(doc: Document) -> Result<(Self, LoadReport)> {
        Self::open_document(doc, false)
    }

    /// [`Model::from_document_reporting`]; with `safe_mode` the timeline
    /// marker is moved to just before the first feature that fails, so the
    /// model opens fully healthy (rolled back) and can be repaired step by
    /// step. A document whose very first feature fails opens flagged as usual.
    pub fn open_document(mut doc: Document, safe_mode: bool) -> Result<(Self, LoadReport)> {
        let mut cleaned = Vec::new();
        let pruned = doc.prune_configuration_occurrences();
        if pruned > 0 {
            cleaned.push(format!(
                "removed {pruned} reference(s) to deleted occurrences from configurations"
            ));
        }
        cleaned.extend(doc.prune_configuration_values());
        let stored = doc.clone();
        let expression_error = match crate::params::apply_expressions(&mut doc) {
            Ok(()) => None,
            Err(e) => {
                doc = stored;
                Some(e)
            }
        };
        let mut m = Self {
            doc: Document::new(),
            eval: Evaluation::empty(),
            undo: vec![],
            redo: vec![],
            revision: 0,
            dirty: false,
            cache: RegenCache::default(),
            defer_measurements: false,
            measure_failed: None,
        };
        // A cut that now splits its target into more pieces than the file
        // assigned (e.g. a kernel change between builds) gets its bodies here,
        // as an edit would; the document is then marked unsaved.
        let before = doc.features.clone();
        m.eval = m.regen_assigning_pieces(&mut doc)?;
        if doc.features != before {
            cleaned.push(
                "a cut now splits its body into more pieces than the file recorded: new bodies \
                 were assigned (save to keep them)"
                    .into(),
            );
        }
        let mut rolled_back_to = None;
        let first_failure = m.eval.failures().next().map(|r| r.id);
        if safe_mode
            && let Some(first) = first_failure
            && let Some(p) = doc.features.iter().position(|f| f.id == first)
            && p > 0
        {
            let mut rolled = doc.clone();
            rolled.rollback = Some(rolled.features[p - 1].id);
            let eval = m.regen(&rolled)?;
            // Earlier features can depend on later ones only through the
            // evaluation order; keep the safe-mode result only when healthy.
            if eval.is_healthy() {
                doc = rolled;
                m.eval = eval;
                rolled_back_to = Some(first);
            }
        }
        let failures = m
            .eval
            .failures()
            .map(|r| {
                (
                    r.id,
                    r.health.message.clone().unwrap_or_else(|| "failed".into()),
                )
            })
            .collect();
        m.dirty = !cleaned.is_empty() || rolled_back_to.is_some();
        // Seeds only record the identity this evaluation already gave the
        // pieces; adopting them is not an unsaved change.
        adopt_piece_seeds(&mut doc, &m.eval);
        m.doc = doc;
        Ok((
            m,
            LoadReport {
                expression_error,
                cleaned,
                failures,
                rolled_back_to,
            },
        ))
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
            self.eval = regenerate_partial(&self.doc, Some(&mut self.cache), true)?;
        }
        Ok(())
    }

    fn regen(&mut self, doc: &Document) -> Result<Evaluation> {
        regenerate_partial(doc, Some(&mut self.cache), !self.defer_measurements)
    }

    /// Regenerate `doc`; when a cut splits its target into more separate
    /// pieces than it has body ids for ([`DocError::NeedsPieces`]), assign
    /// the missing ids (new bodies, named and placed like any new body) in this same transaction and regenerate again. Bounded: each
    /// round assigns at least one id and a document has finitely many cuts.
    fn regen_assigning_pieces(&mut self, doc: &mut Document) -> Result<Evaluation> {
        for _ in 0..MAX_PIECE_ROUNDS {
            match self.regen(doc) {
                Err(DocError::NeedsPieces {
                    feature,
                    pieces,
                    assigned,
                }) if pieces > assigned + 1 && pieces <= MAX_PIECES => {
                    // Named after the body that was split: "Bar (2)", "Bar (3)", ...
                    let source = doc
                        .feature(feature)?
                        .target_body()
                        .and_then(|b| doc.body(b).ok())
                        .map(|b| b.name.clone());
                    for _ in assigned + 1..pieces {
                        let name = match &source {
                            Some(src) => doc.piece_name(src),
                            None => doc.default_body_name(),
                        };
                        let b = doc.add_body(name);
                        match &mut doc.feature_mut(feature)?.kind {
                            crate::FeatureKind::Extrude { pieces, .. }
                            | crate::FeatureKind::SplitBody { pieces, .. } => pieces.push(b),
                            _ => {
                                return Err(DocError::Validation(format!(
                                    "{feature}: only cuts and splits make separate pieces"
                                )));
                            }
                        }
                    }
                }
                Err(DocError::NeedsPieces {
                    feature, pieces, ..
                }) if pieces > MAX_PIECES => {
                    return Err(DocError::ProductLimit(format!(
                        "{feature}: the cut would split its target into {pieces} separate bodies \
                         (at most {MAX_PIECES})"
                    )));
                }
                other => return other,
            }
        }
        Err(DocError::ProductLimit(
            "too many cuts split their targets in one edit".into(),
        ))
    }

    /// Drop the regeneration memo (after a caught panic may have left a
    /// half-built entry); the next regeneration rebuilds it.
    pub fn discard_cache(&mut self) {
        self.cache = RegenCache::default();
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

    /// Apply `edit` transactionally. `label` names the history entry. The
    /// edit is refused (nothing changes) when any feature that evaluated
    /// before fails after it: an edit never makes the document less healthy.
    pub fn transact<T>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut Document) -> Result<T>,
    ) -> Result<T> {
        self.transact_allowing(label, None, edit)
    }

    /// [`Model::transact`], except that features downstream of `edited`
    /// that newly fail are committed flagged in their health (Edit Feature,
    /// as in Fusion); `edited` itself must still evaluate.
    pub fn transact_allowing<T>(
        &mut self,
        label: &str,
        edited: Option<FeatureId>,
        edit: impl FnOnce(&mut Document) -> Result<T>,
    ) -> Result<T> {
        let mut candidate = self.doc.clone();
        let out = edit(&mut candidate)?;
        // Bound values follow their expressions in the same transaction.
        crate::params::apply_expressions(&mut candidate)?;
        let eval = self.regen_assigning_pieces(&mut candidate)?;
        let before: std::collections::HashSet<FeatureId> =
            self.eval.failures().map(|r| r.id).collect();
        let allowed: Vec<FeatureId> = match edited {
            Some(e) => candidate
                .with_dependents(e)
                .into_iter()
                .filter(|f| *f != e)
                .collect(),
            None => vec![],
        };
        if let Some(r) = eval
            .failures()
            .find(|r| !before.contains(&r.id) && !allowed.contains(&r.id))
        {
            return Err(new_failure(&eval, r));
        }
        adopt_piece_seeds(&mut candidate, &eval);
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

    /// Copy of the state that [`Model::restore`] puts back.
    pub fn snapshot(&self) -> ModelSnapshot {
        ModelSnapshot {
            doc: self.doc.clone(),
            eval: self.eval.clone(),
            undo: self.undo.clone(),
            redo: self.redo.clone(),
            dirty: self.dirty,
        }
    }

    /// Put back a [`ModelSnapshot`] exactly (no regeneration: the evaluation
    /// is the one taken with it). Undo/redo and the dirty flag are as they
    /// were, so temporary changes leave no history; `revision` still advances
    /// so clients refresh their caches.
    pub fn restore(&mut self, s: ModelSnapshot) {
        self.doc = s.doc;
        self.eval = s.eval;
        self.undo = s.undo;
        self.redo = s.redo;
        self.dirty = s.dirty;
        self.revision += 1;
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

/// The error to report for a feature that newly fails in a candidate
/// evaluation: its own error, or (failed upstream) the root feature's.
fn new_failure(eval: &Evaluation, r: &crate::FeatureResult) -> DocError {
    if let Some(e) = &r.health.error {
        return e.clone();
    }
    let root = r.health.upstream;
    eval.features
        .iter()
        .find(|x| Some(x.id) == root)
        .and_then(|x| x.health.error.clone())
        .unwrap_or_else(|| DocError::Validation(r.health.message.clone().unwrap_or_default()))
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

/// Store the piece seeds of every feature `eval` evaluated (ADR 0003, piece
/// identity): a cut or split that separates a body records where each of its
/// pieces is, so the next regeneration recognises them after an upstream
/// edit. Features that failed or were not evaluated keep their seeds.
fn adopt_piece_seeds(doc: &mut Document, eval: &Evaluation) {
    for r in &eval.features {
        if r.health.state != crate::HealthState::Healthy {
            continue;
        }
        if let Ok(f) = doc.feature_mut(r.id)
            && f.piece_seeds != r.piece_seeds
        {
            f.piece_seeds = r.piece_seeds.clone();
        }
    }
    adopt_face_prints(doc, eval);
}

/// Keep [`crate::Feature::face_prints`] of every STEP import equal to the
/// faces the document references: a stored print stays as it was (it is
/// what the reference meant, even if the face no longer matches), a newly
/// referenced planar face gets the print of the face as imported now, and
/// prints of faces nothing references any more are dropped.
fn adopt_face_prints(doc: &mut Document, eval: &Evaluation) {
    let imports: Vec<FeatureId> = doc
        .features
        .iter()
        .filter(|f| matches!(f.kind, crate::FeatureKind::ImportStep { .. }))
        .map(|f| f.id)
        .collect();
    for id in imports {
        let refs = crate::regen::imported_face_refs(doc, id);
        let Ok(f) = doc.feature(id) else { continue };
        let prints: Vec<crate::ImportFacePrint> = refs
            .iter()
            .filter_map(|&(body, face)| {
                f.face_prints
                    .iter()
                    .find(|p| p.body == body && p.face == face)
                    .cloned()
                    .or_else(|| {
                        eval.import_planes
                            .get(&(id, body))?
                            .iter()
                            .find(|p| p.index == face)
                            .map(|p| crate::ImportFacePrint::of(body, p))
                    })
            })
            .collect();
        if f.face_prints != prints
            && let Ok(f) = doc.feature_mut(id)
        {
            f.face_prints = prints;
        }
    }
}
