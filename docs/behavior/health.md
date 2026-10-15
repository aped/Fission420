# Feature health, opening documents with errors, and Edit Feature

## Per-feature health

Every regeneration gives each feature a health state (`FeatureHealth` in
`rcad-document`, reported as `health` on each entry of `inspect`'s feature
list):

| state | meaning |
|---|---|
| `Healthy` | evaluated without notes that need attention |
| `Warning` | evaluated, with a note the user should read (reserved: no feature reports it yet) |
| `Error` | did not evaluate; `message` gives the error and `hint` a repair step |
| `Suppressed` | suppressed, not evaluated |
| `RolledBack` | after the timeline marker, not evaluated |

A feature that only fails because something it reads failed is an `Error`
whose `upstream` names the failing feature. It is never evaluated on a state
the history does not describe. "Reads" covers the features it references and
the bodies a failed feature should have made or changed.

A panic inside one feature's evaluation (a kernel defect) is caught. The
feature fails with a kernel error and the state it was building is discarded.

## Regeneration modes

- **Partial** (the interactive model, `Model`): a failing feature is recorded
  and regeneration continues past it. The model stays open and shows which
  features failed.
- **Strict** (`regenerate`, `regenerate_cached`): the first failure stops the
  regeneration with its error. Validators and batch tools use this mode.

Cancellation (`Cancelled`) and an unassigned split (`NeedsPieces`) are never
per-feature health. They always stop the regeneration.

## Edits

An edit (`set_parameter`, a new feature, ...) is refused, with nothing
changed, if it makes a previously healthy feature fail. Edit Feature
(`redefine_feature`) is the exception for the edited feature's dependents:
they may fail, the edit is committed in one undo step, and the response lists
the dependents that now fail. Edit Feature itself is refused if the edited
feature fails, or if the new definition changes the feature's type.

Undo and redo always succeed. They restore a document, whatever its health.

## Opening a document whose history fails

`open` loads a document even when features fail. The response notes say
`ERRORS: N feature(s) did not regenerate` and list them. `open` with
`safe_mode: true` also moves the timeline marker to just before the first
failing feature (if that leaves a healthy model), so the user can repair
the history step by step. The model is marked dirty.

## Exports

STL, 3MF, STEP, assembly, drawing, flat-pattern and configuration exports
are refused while any feature fails. The error names the failing features.
An export never writes a model the history does not fully describe.

## Errors outside regeneration

A panic anywhere in a command is caught at the command boundary and
returned as an `Internal` error. Transactions replace the model only on
commit, so the document stays at its last committed revision. The
regeneration cache is discarded.
