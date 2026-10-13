//! `cargo xtask check`: repository layout, formatting, lints, tests (release),
//! headless acceptance. Exits non-zero on the first failing stage; never masks failures.
use std::path::Path;
use std::process::{Command, ExitCode};

/// Run records belong under `runs/` (local, git-ignored; see AGENTS.md), not in the root.
/// Returns the offending root entries.
fn stray_run_files(root: &Path) -> std::io::Result<Vec<String>> {
    const EXACT: &[&str] = &["GOAL.md", "PLAN.md", "KICKOFF.md", "STATUS.md", "plan"];
    const PREFIX: &[&str] = &["SESSION_", "NIGHT_", "MORNING_REPORT"];
    let mut stray = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if EXACT.contains(&name.as_str()) || PREFIX.iter().any(|p| name.starts_with(p)) {
            stray.push(name);
        }
    }
    stray.sort();
    Ok(stray)
}

fn check_layout() -> bool {
    eprintln!("==> layout: run records under runs/");
    // xtask lives one level below the workspace root.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    match stray_run_files(&root) {
        Ok(stray) if stray.is_empty() => {
            eprintln!("<== layout: ok");
            true
        }
        Ok(stray) => {
            eprintln!(
                "<== layout: FAILED: run files in the repository root: {}\n    \
                 move them to runs/run-N/ (status: runs/STATUS.md); see AGENTS.md",
                stray.join(", ")
            );
            false
        }
        Err(e) => {
            eprintln!("<== layout: FAILED: cannot list the repository root: {e}");
            false
        }
    }
}

fn run(stage: &str, args: &[&str]) -> bool {
    eprintln!("==> {stage}: cargo {}", args.join(" "));
    let t = std::time::Instant::now();
    let ok = Command::new(env!("CARGO"))
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    eprintln!(
        "<== {stage}: {} ({:.1}s)",
        if ok { "ok" } else { "FAILED" },
        t.elapsed().as_secs_f64()
    );
    ok
}

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    if task != "check" {
        eprintln!("usage: cargo xtask check");
        return ExitCode::from(64);
    }
    if !check_layout() {
        return ExitCode::FAILURE;
    }
    let stages: &[(&str, &[&str])] = &[
        ("fmt", &["fmt", "--all", "--check"]),
        (
            "clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        ("test", &["test", "--workspace", "--release"]),
        (
            "acceptance",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "acceptance",
                "--out",
                "artifacts/acceptance",
            ],
        ),
        (
            "plate example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/plate_workflow.json",
            ],
        ),
        (
            "bracket example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/bracket_workflow.json",
            ],
        ),
        (
            "scenario A example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/scenario_a_workflow.json",
            ],
        ),
        (
            "scenario B example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/scenario_b_workflow.json",
            ],
        ),
        (
            "scenario R example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/scenario_r_workflow.json",
            ],
        ),
        (
            "imported STEP + hole example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/imported_step_hole_workflow.json",
            ],
        ),
        (
            "DXF + STL import example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/dxf_stl_import_workflow.json",
            ],
        ),
        (
            "enclosure example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/enclosure_workflow.json",
            ],
        ),
        (
            "clearance coupon example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/clearance_coupon_workflow.json",
            ],
        ),
        (
            "mounting bracket example",
            &[
                "run",
                "--release",
                "-p",
                "f420-cli",
                "--",
                "run",
                "examples/mounting_bracket_workflow.json",
            ],
        ),
    ];
    for (name, args) in stages {
        if !run(name, args) {
            return ExitCode::FAILURE;
        }
    }
    eprintln!("all checks passed");
    ExitCode::SUCCESS
}
