//! Entity index over the parsed DATA section (ruststep AST) and typed parameter access.
//!
//! Every accessor returns an error naming the entity type and `#id` instead of
//! panicking, so malformed input is reported precisely.
use super::{StepError, fail};
use std::collections::HashMap;
use truck_stepio::r#in::ruststep::ast::{EntityInstance, Name, Parameter};

/// One entity instance: a simple record (one part) or a complex instance (several
/// `NAME(params)` parts).
pub(super) struct Entity {
    pub parts: Vec<(String, Vec<Parameter>)>,
}

impl Entity {
    /// Display name: `CIRCLE` or `(LENGTH_UNIT NAMED_UNIT SI_UNIT)` for complex instances.
    pub fn type_name(&self) -> String {
        match self.parts.as_slice() {
            [(n, _)] => n.clone(),
            parts => format!(
                "({})",
                parts
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        }
    }
    pub fn is_simple(&self, name: &str) -> bool {
        self.parts.len() == 1 && self.parts[0].0 == name
    }
    /// Parameters of the part called `name` (simple or complex instance).
    pub fn part(&self, name: &str) -> Option<&[Parameter]> {
        self.parts
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p.as_slice())
    }
    pub fn has(&self, name: &str) -> bool {
        self.parts.iter().any(|(n, _)| n == name)
    }
}

pub(super) struct Index {
    pub map: HashMap<u64, Entity>,
    /// Ids in file order (deterministic iteration).
    pub order: Vec<u64>,
}

fn record_params(p: &Parameter) -> Vec<Parameter> {
    match p {
        Parameter::List(v) => v.clone(),
        other => vec![other.clone()],
    }
}

impl Index {
    pub fn build(entities: &[EntityInstance]) -> Result<Index, StepError> {
        let mut map = HashMap::with_capacity(entities.len());
        let mut order = Vec::with_capacity(entities.len());
        for e in entities {
            let (id, parts) = match e {
                EntityInstance::Simple { id, record } => (
                    *id,
                    vec![(record.name.clone(), record_params(&record.parameter))],
                ),
                EntityInstance::Complex { id, subsuper } => (
                    *id,
                    subsuper
                        .0
                        .iter()
                        .map(|r| (r.name.clone(), record_params(&r.parameter)))
                        .collect(),
                ),
            };
            if map.insert(id, Entity { parts }).is_some() {
                return Err(fail(format!("duplicate entity id #{id}")));
            }
            order.push(id);
        }
        Ok(Index { map, order })
    }

    pub fn get(&self, id: u64) -> Result<&Entity, StepError> {
        self.map
            .get(&id)
            .ok_or_else(|| fail(format!("reference to missing entity #{id}")))
    }

    pub fn type_name(&self, id: u64) -> String {
        self.map
            .get(&id)
            .map(|e| e.type_name())
            .unwrap_or_else(|| "<missing>".into())
    }

    /// Parameters of a simple entity whose type must be one of `names`.
    pub fn simple<'a>(&'a self, id: u64, names: &[&str]) -> Result<(&'a str, P<'a>), StepError> {
        let e = self.get(id)?;
        match e.parts.as_slice() {
            [(n, p)] if names.contains(&n.as_str()) => Ok((n.as_str(), P { id, ty: n, p })),
            _ => Err(fail(format!(
                "#{id} is {}, expected {}",
                e.type_name(),
                names.join(" or ")
            ))),
        }
    }

    /// Ids of all simple entities of type `name`, in file order.
    pub fn all_simple(&self, name: &str) -> Vec<u64> {
        self.order
            .iter()
            .copied()
            .filter(|id| self.map[id].is_simple(name))
            .collect()
    }

    /// Ids of all entities (simple or complex) having a part called `name`.
    pub fn all_with(&self, name: &str) -> Vec<u64> {
        self.order
            .iter()
            .copied()
            .filter(|id| self.map[id].has(name))
            .collect()
    }
}

/// Parameter list of one entity part with typed, error-reporting accessors.
#[derive(Clone, Copy)]
pub(super) struct P<'a> {
    pub id: u64,
    pub ty: &'a str,
    pub p: &'a [Parameter],
}

impl<'a> P<'a> {
    pub fn new(id: u64, ty: &'a str, p: &'a [Parameter]) -> Self {
        P { id, ty, p }
    }
    fn bad(&self, i: usize, what: &str) -> StepError {
        fail(format!(
            "{} #{}: parameter {} is not {what}",
            self.ty,
            self.id,
            i + 1
        ))
    }
    pub fn at(&self, i: usize) -> Result<&'a Parameter, StepError> {
        self.p.get(i).ok_or_else(|| {
            fail(format!(
                "{} #{}: missing parameter {} (has {})",
                self.ty,
                self.id,
                i + 1,
                self.p.len()
            ))
        })
    }
    pub fn r(&self, i: usize) -> Result<u64, StepError> {
        as_ref(self.at(i)?).ok_or_else(|| self.bad(i, "an entity reference"))
    }
    pub fn f(&self, i: usize) -> Result<f64, StepError> {
        let v = as_real(self.at(i)?).ok_or_else(|| self.bad(i, "a number"))?;
        if !v.is_finite() {
            return Err(self.bad(i, "a finite number"));
        }
        Ok(v)
    }
    pub fn int(&self, i: usize) -> Result<i64, StepError> {
        match self.at(i)? {
            Parameter::Integer(k) => Ok(*k),
            _ => Err(self.bad(i, "an integer")),
        }
    }
    pub fn s(&self, i: usize) -> Result<&'a str, StepError> {
        match self.at(i)? {
            Parameter::String(s) => Ok(s.as_str()),
            _ => Err(self.bad(i, "a string")),
        }
    }
    /// Optional string (`$` / `*` give `None`).
    pub fn opt_s(&self, i: usize) -> Option<&'a str> {
        match self.p.get(i) {
            Some(Parameter::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }
    pub fn b(&self, i: usize) -> Result<bool, StepError> {
        match self.at(i)? {
            Parameter::Enumeration(e) => match e.trim_matches('.') {
                "T" | "TRUE" => Ok(true),
                "F" | "FALSE" => Ok(false),
                _ => Err(self.bad(i, "a boolean")),
            },
            _ => Err(self.bad(i, "a boolean")),
        }
    }
    pub fn e(&self, i: usize) -> Result<&'a str, StepError> {
        match self.at(i)? {
            Parameter::Enumeration(e) => Ok(e.trim_matches('.')),
            _ => Err(self.bad(i, "an enumeration")),
        }
    }
    pub fn is_unset(&self, i: usize) -> bool {
        matches!(
            self.p.get(i),
            None | Some(Parameter::NotProvided | Parameter::Omitted)
        )
    }
    pub fn list(&self, i: usize) -> Result<&'a [Parameter], StepError> {
        match self.at(i)? {
            Parameter::List(v) => Ok(v.as_slice()),
            _ => Err(self.bad(i, "a list")),
        }
    }
    pub fn refs(&self, i: usize) -> Result<Vec<u64>, StepError> {
        self.list(i)?
            .iter()
            .map(|x| as_ref(x).ok_or_else(|| self.bad(i, "a list of references")))
            .collect()
    }
    pub fn reals(&self, i: usize) -> Result<Vec<f64>, StepError> {
        self.list(i)?
            .iter()
            .map(|x| {
                as_real(x)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| self.bad(i, "a list of finite numbers"))
            })
            .collect()
    }
    pub fn ints(&self, i: usize) -> Result<Vec<i64>, StepError> {
        self.list(i)?
            .iter()
            .map(|x| match x {
                Parameter::Integer(k) => Ok(*k),
                _ => Err(self.bad(i, "a list of integers")),
            })
            .collect()
    }
    /// List of lists of references (B-spline surface control net).
    pub fn ref_grid(&self, i: usize) -> Result<Vec<Vec<u64>>, StepError> {
        self.list(i)?
            .iter()
            .map(|row| match row {
                Parameter::List(v) => v
                    .iter()
                    .map(|x| as_ref(x).ok_or_else(|| self.bad(i, "a reference grid")))
                    .collect(),
                _ => Err(self.bad(i, "a reference grid")),
            })
            .collect()
    }
    /// List of lists of numbers (rational B-spline surface weights).
    pub fn real_grid(&self, i: usize) -> Result<Vec<Vec<f64>>, StepError> {
        self.list(i)?
            .iter()
            .map(|row| match row {
                Parameter::List(v) => v
                    .iter()
                    .map(|x| {
                        as_real(x)
                            .filter(|v| v.is_finite())
                            .ok_or_else(|| self.bad(i, "a number grid"))
                    })
                    .collect(),
                _ => Err(self.bad(i, "a number grid")),
            })
            .collect()
    }
}

pub(super) fn as_ref(p: &Parameter) -> Option<u64> {
    match p {
        Parameter::Ref(Name::Entity(id)) => Some(*id),
        _ => None,
    }
}

/// Number, also accepting integers and typed measures such as `LENGTH_MEASURE(25.4)`.
pub(super) fn as_real(p: &Parameter) -> Option<f64> {
    match p {
        Parameter::Real(x) => Some(*x),
        Parameter::Integer(k) => Some(*k as f64),
        Parameter::Typed { parameter, .. } => as_real(parameter),
        _ => None,
    }
}
