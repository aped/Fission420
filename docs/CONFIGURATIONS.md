# Configurations

Status: run 6 (named configurations, pull-forward of SESSION_7 §3), run 7 (table editor,
variant export UI, table semantics). Code: `rcad_document::Configuration`,
`Document::apply_configuration` (`crates/rcad-document/src/model.rs`), commands in
`crates/rcad-commands/src/lib.rs`, UI in `crates/rcad/src/panels.rs` (Configurations section)
and `crates/rcad/src/config_ui.rs` (table and export). Format v6.

## Model

A configuration is a named variant of one document:

* `values`: user-parameter expressions it sets (other parameters are shared: editing them
  changes every variant);
* `suppressed`: features it suppresses. Every feature listed by *any* configuration is a
  *configured feature*: activating a configuration suppresses exactly the configured
  features it lists and unsuppresses the other configured features. Features no
  configuration lists keep their own suppression state;
* `excluded_occurrences`: component occurrences it excludes (the same rule for
  occurrences: excluded ones leave display, BOM, mass, interference, drawings and export);
* `part_number`: shown in title blocks (when the drawing leaves it empty) and in variant
  export file names.

`active_configuration` names the applied one. Activation is one undo step and is refused
unchanged when the result does not regenerate. References are by feature id and occurrence
id, so they are stable across activations; deleting a feature removes it from every
configuration, and a file naming a missing feature is refused on load.

## Commands

* `set_configuration {name, values, suppressed, part_number, excluded_occurrences}`: create
  or replace one (re-applied when active).
* `set_configuration_table {configurations}` (run 7): replace the whole list in one undo
  step. Every row is checked (name characters, expression syntax, parameters, features and
  occurrences exist), names must be unique; the active configuration is re-applied, or
  cleared when it was removed. Features and occurrences that no configuration controls after
  the edit are **restored** (unsuppressed / included) rather than left in whatever state the
  last activation gave them. When the table removes (or renames) the active configuration, the
  document keeps the parameter values and suppression it had (nothing is reset) and no
  configuration is active until one is activated (review run 7 #17: documented, not changed).
* `activate_configuration {name}`, `delete_configuration {name}`.
* `export_configurations {dir, stem, configurations?, step, stl, 3mf, overwrite}`: for each
  configuration (all by default) activate, regenerate and write
  `<dir>/<stem>-<configuration>[-<part number>].<ext>`; the document, undo/redo stacks and
  dirty flag are restored afterwards. Names are sanitized; two configurations that would
  write the same file are refused before writing; an existing file is refused unless
  `overwrite`.

## UI

* Browser **Configurations**: list (● active), Activate, Delete, "Save current as
  configuration" (all user-parameter expressions and the features suppressed now).
* Browser **Configuration table** (run 7): one column per configuration, rows: name, part
  number, every user parameter (expression; empty = not set by that configuration), every
  non-sketch feature ("suppressed" check box), every occurrence ("included" check box).
  "+ Column" adds a configuration from the current state, "− Last column" removes one.
  Edits are a draft until **Apply table** (`set_configuration_table`); **Revert** discards
  them. An invalid table is refused with the reason and the document is unchanged.
* **Variant export** (run 7, under the table): folder, stem, STEP/STL/3MF, Overwrite,
  **Export variants** (`export_configurations`).

## Not supported

Spreadsheet-like logic (formulas across configurations), per-configuration materials,
appearance or sketch dimensions not bound to a user parameter, nested configurations of
sub-assemblies, configuration-specific drawings (a drawing shows the active configuration;
export it once per configuration).

Evidence: `crates/rcad-commands/tests/configurations.rs` (run 6: two variants, shared edit,
batch export, refusals, reopen), `crates/rcad-commands/tests/product_configurations.rs` (run 7
acceptance 3: 80/100 mm enclosure assembly with an optional fan occurrence and drain hole;
per variant assembly volume/mass, BOM and drawing dimensions; shared height edit; export
names; no silent overwrite; table edit + undo; reopen), `crates/rcad/tests/run6_ui.rs`,
`crates/rcad/tests/config_table_ui.rs` (table editing, one undo step, duplicate refused,
variant export through the widgets).
