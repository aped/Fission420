//! Transactions and undo/redo over the authoritative document.
use crate::{DocError, Document, Evaluation, FeatureId, Result, regenerate};
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
}

/// Cap on undo depth; documents are small, so this is generous.
const MAX_UNDO: usize = 256;

impl Model {
    pub fn new() -> Self {
        let doc = Document::new();
        let eval = Evaluation {
            features: vec![],
            body: None,
            measurements: None,
        };
        Self {
            doc,
            eval,
            undo: vec![],
            redo: vec![],
            revision: 0,
            dirty: false,
        }
    }

    /// Build from a loaded document; fails (without side effects) if it does not regenerate.
    pub fn from_document(doc: Document) -> Result<Self> {
        let eval = regenerate(&doc)?;
        Ok(Self {
            doc,
            eval,
            undo: vec![],
            redo: vec![],
            revision: 0,
            dirty: false,
        })
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
        let eval = regenerate(&candidate)?;
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
        match regenerate(&prev) {
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
        match regenerate(&next) {
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
