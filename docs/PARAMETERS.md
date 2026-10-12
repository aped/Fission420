# Named parameters and expressions

Run 4 (SESSION_4 §3). Code: `crates/f420-document/src/params.rs` (language, evaluation,
binding), commands in `crates/f420-commands/src/lib.rs`, UI in
`crates/f420-app/src/params_ui.rs` and the inspector (`panels.rs`).

## Model

* A document holds an ordered list of **user parameters**: `name`, `expression`, declared
  `unit` (`mm` default, `unitless`, `deg`) and an optional `comment`.
* Any editable feature parameter (the names listed in `FeatureSummary.parameters`:
  sketch dimensions such as `width`, `height`, `diameter`, `d1`, angle dimensions,
  `center_x`/`center_y` of template sketches, extrusion `distance`, pocket `depth`,
  chamfer `distance`, pattern `count`/`spacing`/`angle_deg`, revolve `angle_deg`) can be
  **bound** to an expression (`Feature.expressions`: slot name → expression text).
* Expressions are authoritative. The numeric values stored in the feature definitions
  are derived from them by `apply_expressions`, which **every transaction** runs on the
  candidate document before regeneration (`Model::transact`). A parameter change and the
  whole regenerated model therefore commit, or fail, as **one undoable step**; on any
  error the previous document, evaluation and undo history are kept.
* Loading a file also re-derives bound values (`Model::from_document`).

## Language

```
expr   := term (('+' | '-') term)*
term   := unary (('*' | '/') unary)*
unary  := '-' unary | '+' unary | atom
atom   := number [unit] | name | name '(' expr (',' expr)* ')' | '(' expr ')'
number := digits ['.' digits] [('e' | 'E') ['+' | '-'] digits]
unit   := 'mm' | 'cm' | 'm' | 'in'
name   := [A-Za-z_][A-Za-z0-9_]*   (not a unit or function name; at most 64 characters)
functions: min(a, b), max(a, b), abs(a)
```

Limits: 1000 characters, nesting depth 64. Trigonometry, counts from expressions of
lengths, and scripting are not supported.

### Units

Values carry a length exponent (mm¹ for lengths, mm⁰ for dimensionless numbers).

* A number with a unit is a length in mm (`1 in` = 25.4).
* A **bare number is untyped**: it is a plain factor in `*` and `/`, and takes the other
  operand's unit in `+` and `-` (`outer_height - 2` means 2 mm when `outer_height` is a
  length). An untyped result is taken in the slot's unit (mm for lengths).
* `+`, `-`, `min`, `max` need equal units: adding a length to a `unitless` parameter is an
  error; `outer_length * floor` is mm² and is refused for a length slot.
* `deg` parameters are dimensionless numbers in degrees; angle slots (`angle_deg`, sketch
  angle dimensions) take degrees.
* `count` slots need a dimensionless whole number.

## Errors (document unchanged)

All are `DocError::Expression { name, detail }`, where `name` is the parameter or the
bound feature slot (`"<feature name>.<slot>"`):

| case | example detail |
|---|---|
| syntax | `'(L': expected ')', found the end` |
| unknown name | `'wal + 1': unknown parameter 'wal'` |
| cycle | `circular reference: outer_length -> cavity_length -> outer_length` |
| division by zero | `'outer_length / (floor - floor)': division by zero` |
| unit mismatch | `this value needs a length (mm) but the expression is mm^2` |
| non-finite | `the result is not a finite number` |
| value the geometry cannot take | `Cavity outline.height`: `'cavity_width' = -40: dimension 'height' has invalid value -40: must be finite and > 0 mm (caused by setting wall = 45)` |

A value the feature accepts but the geometry cannot build (e.g. a cut that no longer
removes anything) fails regeneration with that feature's usual error and is refused the
same way.

## Commands

| cmd | fields | behaviour |
|---|---|---|
| `set_user_parameter` | `name`, `expression`, `unit?`, `comment?` | create or redefine; one undo step |
| `rename_user_parameter` | `name`, `new_name` | rewrites every expression that uses it (whole-word, not inside `2mm`), one undo step |
| `delete_user_parameter` | `name` | refused while any parameter or feature slot uses it; the users are named |
| `set_expression` | `feature`, `name`, `expression` | bind a feature slot; a plain number sets the value and removes the binding; an empty expression removes the binding and keeps the value |
| `set_parameter` | `feature`, `name`, `value` | refused on a bound slot ("bound to the expression ..."); unbound behaviour unchanged |
| `edit_sketch` | | a dimension removed by the edit (e.g. a split line's length after Trim) takes its binding with it; the response names it |

`DocSummary.parameters` lists each parameter with its evaluated `value`, `depends_on` and
`used_by` (other parameters, and feature slots as `"F2 Base.distance"`);
`FeatureSummary.expressions` lists a feature's bindings.

References are by parameter name, never by display label or position; rename keeps
dependents correct by rewriting them in the same transaction.

## Native format v4

v4 adds `document.parameters` and `features[].expressions` (both omitted when empty). A v3
document is a v4 document without them, so v3 files load unchanged
(`crates/f420-commands/tests/fixtures/native_v3/`, frozen copies of the run-3 documents,
checked by `native_formats.rs::v3_run3_examples`). Saves always write v4, so an older build
refuses the file ("newer than supported") instead of silently dropping expressions. A file
that claims an older version but contains parameters or expressions is refused.

## UI

* **Browser → Parameters (N)**: name, expression, `= value unit`; hover shows comment,
  uses and used-by. Click a name to edit name / expression / unit, **Apply parameter**,
  **Cancel**, **Delete** (disabled with the reason while used). Bottom row: name,
  value or expression, unit, **Add parameter**. A refusal is shown under the list
  ("Refused, the model is unchanged: …") and in Messages.
* **Inspector → Parameters**: each editable value is a text field that takes a number or an
  expression; a bound value shows its expression with `= value` beside the label.
  **Apply** sends `set_parameter` for unbound numbers (keeps the engine's coalescing of
  rapid edits) and `set_expression` otherwise.
* Tests: `crates/f420-app/tests/parameters_ui.rs` (real widgets: add, bind, edit, undo,
  refused edit, rename, delete disabled, unbind; the enclosure's fit clearance; two-face
  measurement).

## Examples

`examples/enclosure_workflow.json`, `clearance_coupon_workflow.json`,
`mounting_bracket_workflow.json` (generated from `crates/f420-commands/src/printables.rs`)
build their documents entirely from parameters and bindings; the saved
`examples/*.f420.json` are edited by changing parameters only.
