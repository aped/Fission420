//! Wording of the error banner over the 3D view. The log keeps the full
//! technical text ("shell_face rejected: product limit: …"); the banner
//! leads with what did not happen in plain words ("Shell face was not
//! applied: not supported yet") and then the reason and next step.

use crate::app::Diagnostic;

/// Plain words for an error kind (`Diagnostic::kind`).
pub fn kind_words(kind: &str) -> &str {
    match kind {
        "Validation" | "Input" | "Expression" => "invalid input",
        "UnknownFeature" => "unknown feature",
        "UnknownParameter" => "unknown parameter",
        "BrokenReference" => "broken reference",
        "Cycle" => "circular dependency",
        "SketchSolve" => "the sketch does not solve",
        "ProductLimit" => "not supported yet",
        "Kernel" => "geometry kernel error",
        "GeometryCheck" => "the result failed its geometry check",
        "NothingTo" => "nothing to do",
        "File" => "file error",
        "NeedsPieces" => "the body would split into more pieces",
        "Cancelled" => "cancelled",
        "Internal" => "internal error",
        "Worker" => "the model worker stopped",
        "Display" => "display error",
        "Dialog" => "dialog error",
        "Sketch" => "sketch error",
        "Delete" => "cannot delete",
        "Demo" => "demo error",
        other => other,
    }
}

/// "shell_face" -> "Shell face" (command names as the user knows them).
fn command_words(name: &str) -> String {
    match name {
        "redefine_feature" => return "Edit Feature".into(),
        "set_parameter" | "set_user_parameter" => return "The parameter change".into(),
        _ => {}
    }
    let mut s = name.replace('_', " ");
    if let Some(c) = s.get(0..1) {
        let up = c.to_uppercase();
        s.replace_range(0..1, &up);
    }
    s
}

/// The banner's heading and body for an error diagnostic.
pub fn banner_text(d: &Diagnostic) -> (String, String) {
    let words = kind_words(&d.kind);
    match d.text.split_once(" rejected: ") {
        Some((cmd, rest)) if !cmd.is_empty() && !cmd.contains(' ') => {
            // The kind is already in the heading; drop its repetition.
            let body = rest
                .strip_prefix("product limit: ")
                .or_else(|| rest.strip_prefix("validation: "))
                .unwrap_or(rest);
            (
                format!("{} was not applied: {words}", command_words(cmd)),
                body.to_string(),
            )
        }
        _ => {
            let mut h = words.to_string();
            if let Some(c) = h.get(0..1) {
                let up = c.to_uppercase();
                h.replace_range(0..1, &up);
            }
            (h, d.text.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(kind: &str, text: &str) -> Diagnostic {
        Diagnostic {
            error: true,
            kind: kind.into(),
            text: text.into(),
        }
    }

    #[test]
    fn a_rejected_command_leads_with_what_did_not_happen() {
        let (h, b) = banner_text(&diag(
            "ProductLimit",
            "shell_face rejected: product limit: F3: thickness 50 mm does not fit — This is a \
             current product limit; the model is unchanged.",
        ));
        assert_eq!(h, "Shell face was not applied: not supported yet");
        assert!(b.starts_with("F3: thickness 50 mm"), "{b}");
        assert!(b.ends_with("the model is unchanged."), "{b}");
        let (h, _) = banner_text(&diag("Validation", "redefine_feature rejected: bad"));
        assert_eq!(h, "Edit Feature was not applied: invalid input");
    }

    #[test]
    fn other_errors_keep_their_text_under_a_plain_heading() {
        let (h, b) = banner_text(&diag("Input", "count must be a whole number from 1 to 100"));
        assert_eq!(h, "Invalid input");
        assert_eq!(b, "count must be a whole number from 1 to 100");
        let (h, _) = banner_text(&diag("SomethingNew", "x"));
        assert_eq!(h, "SomethingNew");
    }
}
