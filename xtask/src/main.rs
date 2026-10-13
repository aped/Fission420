//! `cargo xtask check`: repository layout, formatting, lints, tests (release),
//! headless acceptance. Exits non-zero on the first failing stage; never masks failures.
use std::path::Path;
use std::process::{Command, ExitCode};

/// Run records belong under `.meta/runs/` (local, git-ignored), not in the root.
/// Returns the offending root entries.
fn stray_run_files(root: &Path) -> std::io::Result<Vec<String>> {
    const EXACT: &[&str] = &[
        "GOAL.md",
        "PLAN.md",
        "KICKOFF.md",
        "STATUS.md",
        "plan",
        "runs",
        "AGENTS.md",
    ];
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
    eprintln!("==> layout: run records under .meta/runs/");
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
                 move them to .meta/runs/run-N/ (status: .meta/runs/STATUS.md; agent instructions: .meta/agents/)",
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

const NEXTEST: &[&str] = &[
    "nextest",
    "run",
    "--workspace",
    "--release",
    "--no-fail-fast",
];
const CARGO_TEST: &[&str] = &["test", "--workspace", "--release", "--no-fail-fast"];
const DOCTEST: &[&str] = &["test", "--workspace", "--release", "--doc"];

/// Whether `cargo nextest` is available (optional speed-up; CI without it
/// runs plain `cargo test`).
fn nextest() -> bool {
    Command::new(env!("CARGO"))
        .args(["nextest", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn run(stage: &str, args: &[&str]) -> bool {
    if args.is_empty() {
        return true; // stage not applicable (e.g. doctest without nextest)
    }
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
        // Tests: cargo-nextest when installed (one process per test, all
        // binaries in parallel; doc tests separately), else cargo test.
        ("test", if nextest() { NEXTEST } else { CARGO_TEST }),
        ("doctest", if nextest() { DOCTEST } else { &[] }),
        (
            "acceptance",
            &[
                "run",
                "--release",
                "-p",
                "rcad-cli",
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
                "rcad-cli",
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
                "rcad-cli",
                "--",
                "run",
                "examples/bracket_workflow.json",
            ],
        ),
        (
            "hinge assembly example",
            &[
                "run",
                "--release",
                "-p",
                "rcad-cli",
                "--",
                "run",
                "examples/hinge_assembly_workflow.json",
            ],
        ),
        (
            "scenario A example",
            &[
                "run",
                "--release",
                "-p",
                "rcad-cli",
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
                "rcad-cli",
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
                "rcad-cli",
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
                "rcad-cli",
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
                "rcad-cli",
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
                "rcad-cli",
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
                "rcad-cli",
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
                "rcad-cli",
                "--",
                "run",
                "examples/mounting_bracket_workflow.json",
            ],
        ),
        (
            "organizer tray example",
            &[
                "run",
                "--release",
                "-p",
                "rcad-cli",
                "--",
                "run",
                "examples/organizer_tray_workflow.json",
            ],
        ),
        (
            "organizer tray (finished) example",
            &[
                "run",
                "--release",
                "-p",
                "rcad-cli",
                "--",
                "run",
                "examples/organizer_tray_finished_workflow.json",
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
