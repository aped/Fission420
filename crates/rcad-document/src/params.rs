//! Named user parameters and expressions (docs/PARAMETERS.md).
//!
//! A document holds named scalar [`Parameter`]s whose definitions are
//! expressions (`outer_length - 2 * wall`), and features may bind any of their
//! editable parameters (`Feature::parameters`) to an expression. Expressions
//! are the authoritative definition; the numeric values stored in feature
//! definitions are derived from them by [`apply_expressions`], which every
//! transaction runs before regeneration, so a parameter change and the whole
//! regenerated model commit (or fail) as one undoable step.
//!
//! Grammar: numbers (optionally with a length unit `mm`, `cm`, `m`, `in`),
//! parameter names (`[A-Za-z_][A-Za-z0-9_]*`), `+ - * /`, unary minus,
//! parentheses, and the functions `min(a, b)`, `max(a, b)`, `abs(a)`.
//!
//! Units: values carry a length exponent (mm¹ for lengths, mm⁰ for
//! dimensionless numbers). A bare literal is *untyped*: it acts as a plain
//! factor in `*` and `/` and adopts the other operand's unit in `+`/`-`
//! (`height - 2` means 2 mm when `height` is a length). Adding a length to a
//! dimensionless parameter, or binding an area to a length, is an error. An
//! untyped result is taken in the slot's own unit (mm for lengths).
use crate::{DocError, Document, FeatureId, FeatureKind, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Declared unit of a user parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParamUnit {
    /// Length in millimetres.
    #[default]
    Mm,
    /// Dimensionless number (counts, ratios).
    Unitless,
    /// Angle in degrees (dimensionless for arithmetic).
    Deg,
}

impl ParamUnit {
    pub fn dim(self) -> i32 {
        match self {
            ParamUnit::Mm => 1,
            ParamUnit::Unitless | ParamUnit::Deg => 0,
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            ParamUnit::Mm => "mm",
            ParamUnit::Unitless => "",
            ParamUnit::Deg => "deg",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "mm" => Some(ParamUnit::Mm),
            "" | "unitless" | "none" => Some(ParamUnit::Unitless),
            "deg" | "°" => Some(ParamUnit::Deg),
            _ => None,
        }
    }
}

/// A named document parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub expression: String,
    #[serde(default)]
    pub unit: ParamUnit,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

/// Kind of value a feature parameter slot takes (its expression's unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotKind {
    /// mm.
    Length,
    /// Degrees (dimensionless arithmetic).
    Angle,
    /// Integer count.
    Count,
}

impl SlotKind {
    fn dim(self) -> i32 {
        match self {
            SlotKind::Length => 1,
            SlotKind::Angle | SlotKind::Count => 0,
        }
    }
    pub fn unit(self) -> &'static str {
        match self {
            SlotKind::Length => "mm",
            SlotKind::Angle => "deg",
            SlotKind::Count => "",
        }
    }
}

/// Value of an evaluated expression: magnitude (mm^dim) and unit exponent
/// (`None`: an untyped literal-only value).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub dim: Option<i32>,
}

const UNITS: &[(&str, f64)] = &[("mm", 1.0), ("cm", 10.0), ("m", 1000.0), ("in", 25.4)];
const FUNCTIONS: &[&str] = &["min", "max", "abs"];
const MAX_EXPR_LEN: usize = 1000;
const MAX_DEPTH: usize = 64;

/// Whether `name` can name a parameter.
pub fn valid_name(name: &str) -> std::result::Result<(), String> {
    let mut c = name.chars();
    let ok_start = c
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !ok_start || !c.all(|c| c.is_ascii_alphanumeric() || c == '_') || name.len() > 64 {
        return Err(format!(
            "'{name}' is not a valid parameter name (letters, digits and _, starting with a \
             letter or _, at most 64 characters)"
        ));
    }
    if UNITS.iter().any(|(u, _)| *u == name) || FUNCTIONS.contains(&name) {
        return Err(format!("'{name}' is reserved (unit or function name)"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64, Option<f64>),
    Ident(String),
    Op(char),
}

fn tokenize(src: &str) -> std::result::Result<Vec<Tok>, String> {
    if src.len() > MAX_EXPR_LEN {
        return Err(format!("expression longer than {MAX_EXPR_LEN} characters"));
    }
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let s = i;
            while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let save = i;
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                if i < b.len() && (b[i] as char).is_ascii_digit() {
                    while i < b.len() && (b[i] as char).is_ascii_digit() {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            let v: f64 = src[s..i]
                .parse()
                .map_err(|_| format!("bad number '{}'", &src[s..i]))?;
            // Optional unit suffix (with or without a space before it).
            let mut j = i;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            let k0 = j;
            while j < b.len() && (b[j] as char).is_ascii_alphabetic() {
                j += 1;
            }
            let word = &src[k0..j];
            let unit = UNITS.iter().find(|(u, _)| *u == word).map(|(_, s)| *s);
            if unit.is_some() && (j == b.len() || !(b[j] as char).is_ascii_alphanumeric()) {
                i = j;
            }
            out.push(Tok::Num(v, unit));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let s = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(src[s..i].to_string()));
        } else if "+-*/(),".contains(c) {
            out.push(Tok::Op(c));
            i += 1;
        } else {
            return Err(format!("unexpected character '{c}'"));
        }
    }
    Ok(out)
}

/// Parsed expression tree.
#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Num(f64, Option<f64>),
    Var(String),
    Neg(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

struct Parser {
    toks: Vec<Tok>,
    i: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn expect(&mut self, c: char) -> std::result::Result<(), String> {
        match self.next() {
            Some(Tok::Op(x)) if x == c => Ok(()),
            other => Err(format!(
                "expected '{c}', found {}",
                describe(other.as_ref())
            )),
        }
    }
    fn expr(&mut self) -> std::result::Result<Expr, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("expression nested too deeply".into());
        }
        let mut l = self.term()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek().cloned() {
            self.i += 1;
            let r = self.term()?;
            l = Expr::Bin(c, Box::new(l), Box::new(r));
        }
        self.depth -= 1;
        Ok(l)
    }
    fn term(&mut self) -> std::result::Result<Expr, String> {
        let mut l = self.unary()?;
        while let Some(Tok::Op(c @ ('*' | '/'))) = self.peek().cloned() {
            self.i += 1;
            let r = self.unary()?;
            l = Expr::Bin(c, Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn unary(&mut self) -> std::result::Result<Expr, String> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.i += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Some(Tok::Op('+')) => {
                self.i += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }
    fn atom(&mut self) -> std::result::Result<Expr, String> {
        match self.next() {
            Some(Tok::Num(v, u)) => Ok(Expr::Num(v, u)),
            Some(Tok::Ident(n)) => {
                if let Some(Tok::Op('(')) = self.peek() {
                    if !FUNCTIONS.contains(&n.as_str()) {
                        return Err(format!("unknown function '{n}'"));
                    }
                    self.i += 1;
                    let mut args = vec![self.expr()?];
                    while let Some(Tok::Op(',')) = self.peek() {
                        self.i += 1;
                        args.push(self.expr()?);
                    }
                    self.expect(')')?;
                    Ok(Expr::Call(n, args))
                } else {
                    Ok(Expr::Var(n))
                }
            }
            Some(Tok::Op('(')) => {
                let e = self.expr()?;
                self.expect(')')?;
                Ok(e)
            }
            other => Err(format!(
                "expected a number, name or '(', found {}",
                describe(other.as_ref())
            )),
        }
    }
}

fn describe(t: Option<&Tok>) -> String {
    match t {
        None => "the end".into(),
        Some(Tok::Num(v, _)) => format!("number {v}"),
        Some(Tok::Ident(n)) => format!("'{n}'"),
        Some(Tok::Op(c)) => format!("'{c}'"),
    }
}

fn parse(src: &str) -> std::result::Result<Expr, String> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err("empty expression".into());
    }
    let mut p = Parser {
        toks,
        i: 0,
        depth: 0,
    };
    let e = p.expr()?;
    if p.i < p.toks.len() {
        return Err(format!(
            "unexpected {} after the expression",
            describe(p.toks.get(p.i))
        ));
    }
    Ok(e)
}

fn names_in(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Num(..) => {}
        Expr::Var(n) => {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
        Expr::Neg(a) => names_in(a, out),
        Expr::Bin(_, a, b) => {
            names_in(a, out);
            names_in(b, out);
        }
        Expr::Call(_, args) => args.iter().for_each(|a| names_in(a, out)),
    }
}

/// Parameter names an expression refers to (parse errors give none).
pub fn references(src: &str) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(e) = parse(src) {
        names_in(&e, &mut v);
    }
    v
}

/// Check that an expression parses (syntax only).
pub fn check_syntax(src: &str) -> std::result::Result<(), String> {
    parse(src).map(|_| ())
}

/// Whether `src` is a plain number (no names, no units): such an "expression"
/// for a feature slot simply sets the value.
pub fn plain_number(src: &str) -> Option<f64> {
    match parse(src).ok()? {
        Expr::Num(v, None) => Some(v),
        Expr::Neg(b) => match *b {
            Expr::Num(v, None) => Some(-v),
            _ => None,
        },
        _ => None,
    }
}

fn eval(e: &Expr, env: &HashMap<String, Quantity>) -> std::result::Result<Quantity, String> {
    let unit_name = |d: Option<i32>| match d {
        None => "a plain number".to_string(),
        Some(0) => "a dimensionless value".to_string(),
        Some(1) => "a length".to_string(),
        Some(d) => format!("mm^{d}"),
    };
    let q = match e {
        Expr::Num(v, None) => Quantity {
            value: *v,
            dim: None,
        },
        Expr::Num(v, Some(scale)) => Quantity {
            value: v * scale,
            dim: Some(1),
        },
        Expr::Var(n) => *env
            .get(n)
            .ok_or_else(|| format!("unknown parameter '{n}'"))?,
        Expr::Neg(a) => {
            let q = eval(a, env)?;
            Quantity {
                value: -q.value,
                dim: q.dim,
            }
        }
        Expr::Bin(op, a, b) => {
            let (x, y) = (eval(a, env)?, eval(b, env)?);
            match op {
                '+' | '-' => {
                    let dim = match (x.dim, y.dim) {
                        (Some(p), Some(q)) if p != q => {
                            return Err(format!(
                                "cannot {} {} and {}",
                                if *op == '+' { "add" } else { "subtract" },
                                unit_name(x.dim),
                                unit_name(y.dim)
                            ));
                        }
                        (Some(p), _) | (None, Some(p)) => Some(p),
                        (None, None) => None,
                    };
                    let value = if *op == '+' {
                        x.value + y.value
                    } else {
                        x.value - y.value
                    };
                    Quantity { value, dim }
                }
                '*' => Quantity {
                    value: x.value * y.value,
                    dim: match (x.dim, y.dim) {
                        (None, None) => None,
                        (p, q) => Some(p.unwrap_or(0) + q.unwrap_or(0)),
                    },
                },
                '/' => {
                    if y.value == 0.0 {
                        return Err("division by zero".into());
                    }
                    Quantity {
                        value: x.value / y.value,
                        dim: match (x.dim, y.dim) {
                            (None, None) => None,
                            (p, q) => Some(p.unwrap_or(0) - q.unwrap_or(0)),
                        },
                    }
                }
                _ => return Err(format!("unknown operator '{op}'")),
            }
        }
        Expr::Call(f, args) => {
            let vals = args
                .iter()
                .map(|a| eval(a, env))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            match (f.as_str(), vals.as_slice()) {
                ("abs", [a]) => Quantity {
                    value: a.value.abs(),
                    dim: a.dim,
                },
                ("min" | "max", [a, b]) => {
                    if let (Some(p), Some(q)) = (a.dim, b.dim)
                        && p != q
                    {
                        return Err(format!(
                            "{f}() of {} and {}",
                            unit_name(a.dim),
                            unit_name(b.dim)
                        ));
                    }
                    let value = if f == "min" {
                        a.value.min(b.value)
                    } else {
                        a.value.max(b.value)
                    };
                    Quantity {
                        value,
                        dim: a.dim.or(b.dim),
                    }
                }
                _ => {
                    return Err(format!(
                        "{f}() takes {} argument(s), got {}",
                        if f == "abs" { 1 } else { 2 },
                        vals.len()
                    ));
                }
            }
        }
    };
    if !q.value.is_finite() {
        return Err("the result is not a finite number".into());
    }
    Ok(q)
}

/// Value of `q` in a slot expecting unit exponent `dim`.
fn coerce(q: Quantity, dim: i32, what: &str) -> std::result::Result<f64, String> {
    match q.dim {
        None => Ok(q.value),
        Some(d) if d == dim => Ok(q.value),
        Some(d) => Err(format!(
            "{what} needs {} but the expression is {}",
            match dim {
                0 => "a dimensionless number".to_string(),
                1 => "a length (mm)".to_string(),
                d => format!("mm^{d}"),
            },
            match d {
                0 => "dimensionless".to_string(),
                1 => "a length".to_string(),
                d => format!("mm^{d}"),
            }
        )),
    }
}

/// Evaluated user parameters (name -> value in its declared unit).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParamValues {
    pub values: BTreeMap<String, f64>,
}

fn expr_error(name: &str, detail: impl Into<String>) -> DocError {
    DocError::Expression {
        name: name.to_string(),
        detail: detail.into(),
    }
}

/// Evaluate all user parameters in dependency order. Errors name the
/// failing parameter: unknown names, cycles (with the path), division by
/// zero, unit mismatches, non-finite values.
pub fn evaluate_parameters(doc: &Document) -> Result<HashMap<String, Quantity>> {
    let defs: HashMap<&str, &crate::Parameter> = doc
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();
    if defs.len() != doc.parameters.len() {
        return Err(DocError::Validation("duplicate parameter names".into()));
    }
    let mut env: HashMap<String, Quantity> = HashMap::new();
    // 0 = unvisited, 1 = on the stack, 2 = done
    let mut state: HashMap<&str, u8> = HashMap::new();
    fn visit<'a>(
        n: &'a str,
        defs: &HashMap<&'a str, &'a crate::Parameter>,
        state: &mut HashMap<&'a str, u8>,
        env: &mut HashMap<String, Quantity>,
        stack: &mut Vec<&'a str>,
    ) -> Result<()> {
        match state.get(n) {
            Some(2) => return Ok(()),
            Some(1) => {
                let start = stack.iter().position(|s| *s == n).unwrap_or(0);
                let mut cycle: Vec<&str> = stack[start..].to_vec();
                cycle.push(n);
                return Err(expr_error(
                    n,
                    format!("circular reference: {}", cycle.join(" -> ")),
                ));
            }
            _ => {}
        }
        let p = defs[n];
        state.insert(n, 1);
        stack.push(n);
        let e =
            parse(&p.expression).map_err(|e| expr_error(n, format!("'{}': {e}", p.expression)))?;
        let mut refs = Vec::new();
        names_in(&e, &mut refs);
        for r in &refs {
            let Some((key, _)) = defs.get_key_value(r.as_str()) else {
                return Err(expr_error(
                    n,
                    format!("'{}': unknown parameter '{r}'", p.expression),
                ));
            };
            visit(key, defs, state, env, stack)?;
        }
        let q = eval(&e, env).map_err(|e| expr_error(n, format!("'{}': {e}", p.expression)))?;
        let dim = p.unit.dim();
        let v = coerce(q, dim, "this parameter").map_err(|e| {
            expr_error(
                n,
                format!(
                    "'{}': {e} (declared unit {})",
                    p.expression,
                    p.unit.symbol()
                ),
            )
        })?;
        env.insert(
            n.to_string(),
            Quantity {
                value: v,
                dim: Some(dim),
            },
        );
        stack.pop();
        state.insert(n, 2);
        Ok(())
    }
    let mut stack = Vec::new();
    for p in &doc.parameters {
        valid_name(&p.name).map_err(|e| expr_error(&p.name, e))?;
        visit(&p.name, &defs, &mut state, &mut env, &mut stack)?;
    }
    Ok(env)
}

/// Evaluate one expression against the evaluated parameters for a slot of `kind`.
pub fn evaluate_for_slot(
    src: &str,
    env: &HashMap<String, Quantity>,
    kind: SlotKind,
) -> std::result::Result<f64, String> {
    let e = parse(src)?;
    let q = eval(&e, env)?;
    coerce(q, kind.dim(), "this value")
}

/// Kind of feature parameter `name` of `kind` (None: not an editable slot).
pub fn slot_kind(kind: &FeatureKind, name: &str) -> Option<SlotKind> {
    match kind {
        FeatureKind::Sketch {
            plane: crate::SketchPlane::Offset { .. },
            ..
        }
        | FeatureKind::Mirror {
            plane: crate::SketchPlane::Offset { .. },
            ..
        }
        | FeatureKind::Hole {
            plane: crate::SketchPlane::Offset { .. },
            ..
        } if name == "plane_offset" => Some(SlotKind::Length),
        FeatureKind::Sketch {
            plane: crate::SketchPlane::Offset { .. },
            ..
        }
        | FeatureKind::Mirror {
            plane: crate::SketchPlane::Offset { .. },
            ..
        }
        | FeatureKind::Hole {
            plane: crate::SketchPlane::Offset { .. },
            ..
        } if name == "plane_angle_deg" => Some(SlotKind::Angle),
        FeatureKind::Sketch { sketch, .. } => {
            if name == "center_x" || name == "center_y" {
                return Some(SlotKind::Length);
            }
            if name.starts_with("text") && name.ends_with("_height") && kind_has(kind, name) {
                return Some(SlotKind::Length);
            }
            let c = sketch.constraints.iter().find_map(|c| {
                c.dimension()
                    .filter(|d| d.name == name)
                    .map(|_| matches!(c, rcad_sketch::Constraint::Angle { .. }))
            })?;
            Some(if c { SlotKind::Angle } else { SlotKind::Length })
        }
        FeatureKind::Revolve { .. } | FeatureKind::CircularPattern { .. }
            if name == "angle_deg" =>
        {
            Some(SlotKind::Angle)
        }
        FeatureKind::LinearPattern { .. } | FeatureKind::CircularPattern { .. }
            if name == "count" =>
        {
            Some(SlotKind::Count)
        }
        FeatureKind::LinearPattern { .. } if name == "count2" => Some(SlotKind::Count),
        FeatureKind::LinearPattern { .. } if name == "spacing" || name == "spacing2" => {
            Some(SlotKind::Length)
        }
        FeatureKind::Fillet { .. } | FeatureKind::FilletLoop { .. } if name == "radius" => {
            Some(SlotKind::Length)
        }
        FeatureKind::Extrude { .. }
        | FeatureKind::Chamfer { .. }
        | FeatureKind::ChamferLoop { .. }
            if name == "distance" =>
        {
            Some(SlotKind::Length)
        }
        FeatureKind::Extrude { .. } if name == "taper_deg" => {
            kind_has(kind, name).then_some(SlotKind::Angle)
        }
        FeatureKind::Extrude { .. } => kind_has(kind, name).then_some(SlotKind::Length),
        FeatureKind::Pocket { .. } if name == "depth" => Some(SlotKind::Length),
        FeatureKind::Shell { .. } | FeatureKind::ShellFace { .. } if name == "thickness" => {
            Some(SlotKind::Length)
        }
        FeatureKind::MoveBody { .. } if name == "angle_deg" => Some(SlotKind::Angle),
        FeatureKind::ScaleBody { .. } if name == "factor" => Some(SlotKind::Angle),
        FeatureKind::AlignBody { .. } if name == "offset" => Some(SlotKind::Length),
        // The K-factor is a ratio: dimensionless like an angle.
        FeatureKind::SheetMetal { .. } if name == "k_factor" => Some(SlotKind::Angle),
        FeatureKind::SheetMetal { .. } => kind_has(kind, name).then_some(SlotKind::Length),
        FeatureKind::SplitBody { .. } if name == "plane_angle_deg" => {
            kind_has(kind, name).then_some(SlotKind::Angle)
        }
        FeatureKind::SplitBody { .. } if name == "plane_offset" => {
            kind_has(kind, name).then_some(SlotKind::Length)
        }
        FeatureKind::MoveBody { .. } if matches!(name, "tx" | "ty" | "tz") => {
            Some(SlotKind::Length)
        }
        FeatureKind::DirectEdit { .. } if name == "angle_deg" => {
            kind_has(kind, name).then_some(SlotKind::Angle)
        }
        FeatureKind::DirectEdit { .. } => kind_has(kind, name).then_some(SlotKind::Length),
        FeatureKind::Countersink { .. } if name == "diameter" => Some(SlotKind::Length),
        FeatureKind::Thread { .. } => kind_has(kind, name).then_some(SlotKind::Length),
        FeatureKind::Countersink { .. } if name == "angle_deg" => Some(SlotKind::Angle),
        FeatureKind::Hole { .. } => kind_has(kind, name).then_some(SlotKind::Length),
        _ => None,
    }
}

fn kind_has(kind: &FeatureKind, name: &str) -> bool {
    crate::Feature {
        id: FeatureId(0),
        name: String::new(),
        kind: kind.clone(),
        notes: vec![],
        expressions: Default::default(),
        suppressed: false,
        piece_seeds: Default::default(),
    }
    .parameters()
    .iter()
    .any(|(n, _)| n == name)
}

/// Re-derive every bound feature value from its expression (and the user
/// parameters) and write it into the definition. Run by every transaction
/// before regeneration. A value that is unchanged is not rewritten (so a
/// sketch is not re-solved needlessly). Errors name the parameter or the
/// feature slot.
pub fn apply_expressions(doc: &mut Document) -> Result<()> {
    let env = evaluate_parameters(doc)?;
    let ids: Vec<FeatureId> = doc
        .features
        .iter()
        .filter(|f| !f.expressions.is_empty())
        .map(|f| f.id)
        .collect();
    for id in ids {
        let f = doc.feature(id)?;
        let fname = f.name.clone();
        let bound: Vec<(String, String)> = f
            .expressions
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Evaluate every bound slot of this feature first, then apply the
        // changed ones together: coupled values (a hole's depth and its
        // counterbore depth) are validated in their final combination, not
        // one at a time against stale partners.
        let mut changed: Vec<(String, String, f64)> = Vec::new();
        for (slot, src) in bound {
            let label = format!("{fname}.{slot}");
            let f = doc.feature(id)?;
            let kind = slot_kind(&f.kind, &slot).ok_or_else(|| {
                expr_error(
                    &label,
                    format!("{id} has no parameter '{slot}' to bind (was it removed?)"),
                )
            })?;
            let v = evaluate_for_slot(&src, &env, kind)
                .map_err(|e| expr_error(&label, format!("'{src}': {e}")))?;
            if kind == SlotKind::Count && v.fract() != 0.0 {
                return Err(expr_error(
                    &label,
                    format!("'{src}' = {v}: a count must be a whole number"),
                ));
            }
            let current = f
                .parameters()
                .into_iter()
                .find(|(n, _)| *n == slot)
                .map(|(_, v)| v);
            if current.is_some_and(|c| c == v) {
                continue;
            }
            changed.push((slot, src, v));
        }
        if changed.is_empty() {
            continue;
        }
        let values: Vec<(String, f64)> = changed.iter().map(|(n, _, v)| (n.clone(), *v)).collect();
        doc.feature_mut(id)?
            .set_parameters(&values)
            .map_err(|(slot, e)| {
                // User-facing: the slot is already named by the label.
                let m = match e {
                    DocError::Validation(m) => m,
                    other => other.to_string(),
                };
                let m = m.replace(&format!("{id}.{slot}: "), "");
                let (src, v) = changed
                    .iter()
                    .find(|(n, _, _)| *n == slot)
                    .map(|(_, s, v)| (s.clone(), *v))
                    .unwrap_or_default();
                expr_error(&format!("{fname}.{slot}"), format!("'{src}' = {v}: {m}"))
            })?;
    }
    Ok(())
}

/// Replace whole-word references to `old` by `new` in `src` (rename).
pub fn rename_in(src: &str, old: &str, new: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() || c == '_' {
            let s = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            // A unit right after a number is not a name (e.g. "2mm").
            let after_digit = s > 0 && ((b[s - 1] as char).is_ascii_digit() || b[s - 1] == b'.');
            let word = &src[s..i];
            if word == old && !after_digit {
                out.push_str(new);
            } else {
                out.push_str(word);
            }
        } else if c.is_ascii_digit() || c == '.' {
            let s = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            out.push_str(&src[s..i]);
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Who uses parameter `name`: other parameters and feature slots (labels).
pub fn users_of(doc: &Document, name: &str) -> Vec<String> {
    let mut v: Vec<String> = doc
        .parameters
        .iter()
        .filter(|p| references(&p.expression).iter().any(|r| r == name))
        .map(|p| format!("parameter {}", p.name))
        .collect();
    for f in &doc.features {
        for (slot, src) in &f.expressions {
            if references(src).iter().any(|r| r == name) {
                v.push(format!("{} {}.{slot}", f.id, f.name));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, f64, i32)]) -> HashMap<String, Quantity> {
        pairs
            .iter()
            .map(|(n, v, d)| {
                (
                    n.to_string(),
                    Quantity {
                        value: *v,
                        dim: Some(*d),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn arithmetic_units_and_errors() {
        let e = env(&[("L", 80.0, 1), ("w", 2.0, 1), ("n", 4.0, 0)]);
        let ev = |s: &str, k| evaluate_for_slot(s, &e, k);
        assert_eq!(ev("L - 2 * w", SlotKind::Length), Ok(76.0));
        assert_eq!(ev("(L - 2*w) / 2", SlotKind::Length), Ok(38.0));
        assert_eq!(ev("L - 2", SlotKind::Length), Ok(78.0));
        assert_eq!(ev("1 in + 1mm", SlotKind::Length), Ok(26.4));
        assert_eq!(ev("-w", SlotKind::Length), Ok(-2.0));
        assert_eq!(ev("max(w, 1.5)", SlotKind::Length), Ok(2.0));
        assert_eq!(ev("n + 1", SlotKind::Count), Ok(5.0));
        assert_eq!(ev("L / w", SlotKind::Count), Ok(40.0));
        assert_eq!(ev("2.5e1", SlotKind::Length), Ok(25.0));
        assert!(ev("L * w", SlotKind::Length).unwrap_err().contains("mm^2"));
        assert!(
            ev("L + n", SlotKind::Length)
                .unwrap_err()
                .contains("cannot add")
        );
        assert!(
            ev("L / (w - 2)", SlotKind::Length)
                .unwrap_err()
                .contains("division by zero")
        );
        assert!(
            ev("L", SlotKind::Count)
                .unwrap_err()
                .contains("dimensionless")
        );
        assert!(
            ev("x + 1", SlotKind::Length)
                .unwrap_err()
                .contains("unknown parameter 'x'")
        );
        assert!(
            ev("(L", SlotKind::Length)
                .unwrap_err()
                .contains("expected ')'")
        );
        assert!(
            ev("L $ 2", SlotKind::Length)
                .unwrap_err()
                .contains("unexpected character")
        );
        assert!(
            ev("1e308 * 10", SlotKind::Length)
                .unwrap_err()
                .contains("finite")
        );
        assert!(
            ev("sin(1)", SlotKind::Length)
                .unwrap_err()
                .contains("unknown function")
        );
        assert_eq!(plain_number("12.5"), Some(12.5));
        assert_eq!(plain_number("-3"), Some(-3.0));
        assert_eq!(plain_number("3mm"), None);
        assert_eq!(plain_number("w"), None);
    }

    #[test]
    fn rename_is_whole_word() {
        assert_eq!(rename_in("w + wall*2 - w", "w", "t"), "t + wall*2 - t");
        assert_eq!(rename_in("2mm + m", "m", "mass"), "2mm + mass");
        assert_eq!(references("min(a, b) - a"), vec!["a", "b"]);
    }
}
