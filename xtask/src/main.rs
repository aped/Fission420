//! `cargo xtask check`: repository layout, public text, formatting, lints, tests (release),
//! headless acceptance and the example replays. Exits non-zero on the first failing stage;
//! never masks failures.
//!
//! `cargo xtask check --stage <lint|test|doctest|replay>` (repeatable) runs only those
//! groups, in that fixed order, which is how CI splits the gate across jobs. With no
//! `--stage`, everything runs (the local gate).
//!
//! Environment:
//! - `CI`: tests use the nextest `ci` profile (`.config/nextest.toml`) unless
//!   `NEXTEST_PROFILE` names another one.
//! - `RCAD_CLI`: path of a prebuilt `rcad-cli` for acceptance and the replays, instead of
//!   `cargo run --release -p rcad-cli`.
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod public_text;

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

fn check_public_text() -> bool {
    eprintln!("==> public text: no development-process references in tracked files");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    match public_text::offenders(&root) {
        Ok(found) if found.is_empty() => {
            eprintln!("<== public text: ok");
            true
        }
        Ok(found) => {
            for f in &found {
                eprintln!("    {f}");
            }
            eprintln!(
                "<== public text: FAILED: {} reference(s) to development runs, session specs or \
                 local notes; describe the feature, ADR or doc instead",
                found.len()
            );
            false
        }
        Err(e) => {
            eprintln!("<== public text: FAILED: {e}");
            false
        }
    }
}

/// The committed example workflows the replay stage runs, with a display name each.
const EXAMPLES: &[(&str, &str)] = &[
    ("plate", "plate_workflow.json"),
    ("bracket", "bracket_workflow.json"),
    ("hinge assembly", "hinge_assembly_workflow.json"),
    ("drawing set", "drawing_set_workflow.json"),
    ("scenario A", "scenario_a_workflow.json"),
    ("scenario B", "scenario_b_workflow.json"),
    ("scenario R", "scenario_r_workflow.json"),
    ("imported STEP + hole", "imported_step_hole_workflow.json"),
    ("DXF + STL import", "dxf_stl_import_workflow.json"),
    ("enclosure", "enclosure_workflow.json"),
    ("clearance coupon", "clearance_coupon_workflow.json"),
    ("mounting bracket", "mounting_bracket_workflow.json"),
    ("organizer tray", "organizer_tray_workflow.json"),
    (
        "organizer tray (finished)",
        "organizer_tray_finished_workflow.json",
    ),
    ("everyday enclosure", "everyday_enclosure_workflow.json"),
    ("everyday tray", "everyday_tray_workflow.json"),
    ("everyday knob", "everyday_knob_workflow.json"),
    ("threaded pair", "threaded_pair_workflow.json"),
    ("split bar", "split_bar_workflow.json"),
];

/// Groups of stages, selectable with `--stage`; always run in this order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    /// Layout, public text, `fmt --check`, `clippy -D warnings`.
    Lint,
    /// Workspace tests in release (nextest when installed, plus doc tests).
    Test,
    /// Doc tests only (nextest does not run them; CI runs them next to the test build).
    Doctest,
    /// Headless acceptance and the example replays (release CLI).
    Replay,
}

impl Stage {
    const ALL: [Stage; 3] = [Stage::Lint, Stage::Test, Stage::Replay];

    fn parse(s: &str) -> Option<Stage> {
        Some(match s {
            "lint" => Stage::Lint,
            "test" => Stage::Test,
            "doctest" => Stage::Doctest,
            "replay" => Stage::Replay,
            _ => return None,
        })
    }
}

/// Whether `cargo nextest` is available (optional speed-up; without it the tests run
/// under plain `cargo test`, which also runs the doc tests).
fn nextest() -> bool {
    Command::new(env!("CARGO"))
        .args(["nextest", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The nextest profile: `NEXTEST_PROFILE` (read by nextest itself) wins; otherwise `ci`
/// under CI and nextest's default locally.
fn nextest_profile() -> Option<&'static str> {
    let set = |v: &str| std::env::var_os(v).is_some_and(|x| !x.is_empty());
    (!set("NEXTEST_PROFILE") && set("CI")).then_some("ci")
}

/// Runs `program args`, timing it. A spawn failure is a failure.
fn run_cmd(stage: &str, program: &Path, args: &[String]) -> bool {
    eprintln!("==> {stage}: {} {}", program.display(), args.join(" "));
    let t = std::time::Instant::now();
    let ok = match Command::new(program).args(args).status() {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!("    cannot run {}: {e}", program.display());
            false
        }
    };
    eprintln!(
        "<== {stage}: {} ({:.1}s)",
        if ok { "ok" } else { "FAILED" },
        t.elapsed().as_secs_f64()
    );
    ok
}

fn cargo(stage: &str, args: &[&str]) -> bool {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    run_cmd(stage, Path::new(env!("CARGO")), &args)
}

fn lint() -> bool {
    check_layout()
        && check_public_text()
        && cargo("fmt", &["fmt", "--all", "--check"])
        && cargo(
            "clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        )
}

fn doctest() -> bool {
    cargo("doctest", &["test", "--workspace", "--release", "--doc"])
}

fn test() -> bool {
    if !nextest() {
        // cargo test also runs the doc tests.
        return cargo(
            "test",
            &["test", "--workspace", "--release", "--no-fail-fast"],
        );
    }
    let mut args = vec!["nextest", "run", "--workspace", "--release"];
    if let Some(p) = nextest_profile() {
        args.extend(["--profile", p]);
    } else {
        args.push("--no-fail-fast");
    }
    cargo("test", &args) && doctest()
}

/// Acceptance and the example replays, with `RCAD_CLI` or `cargo run`.
fn replay() -> bool {
    // The workflows export into artifacts/, which the CLI does not create.
    if let Err(e) = std::fs::create_dir_all("artifacts") {
        eprintln!("<== replay: FAILED: cannot create artifacts/: {e}");
        return false;
    }
    // Absolute, so a relative RCAD_CLI does not depend on how the OS resolves programs.
    let prebuilt = match std::env::var_os("RCAD_CLI").filter(|v| !v.is_empty()) {
        None => None,
        Some(p) => match std::path::absolute(&p) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("<== replay: FAILED: RCAD_CLI {p:?}: {e}");
                return false;
            }
        },
    };
    let cli = |stage: &str, cli_args: &[&str]| -> bool {
        let mut args: Vec<String> = Vec::new();
        let program = match &prebuilt {
            Some(p) => p.clone(),
            None => {
                args.extend(["run", "--release", "-p", "rcad-cli", "--"].map(String::from));
                PathBuf::from(env!("CARGO"))
            }
        };
        args.extend(cli_args.iter().map(|a| a.to_string()));
        run_cmd(stage, &program, &args)
    };
    if !cli(
        "acceptance",
        &["acceptance", "--out", "artifacts/acceptance"],
    ) {
        return false;
    }
    EXAMPLES.iter().all(|(name, file)| {
        cli(
            &format!("{name} example"),
            &["run", &format!("examples/{file}")],
        )
    })
}

const USAGE: &str =
    "usage: cargo xtask check [--stage lint|test|doctest|replay]... | cargo xtask public-text";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let task = args.next().unwrap_or_default();
    if task == "public-text" {
        return if check_public_text() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if task != "check" {
        eprintln!("{USAGE}");
        return ExitCode::from(64);
    }
    let mut chosen = Vec::new();
    while let Some(a) = args.next() {
        let value = match a.strip_prefix("--stage=") {
            Some(v) => Some(v.to_string()),
            None if a == "--stage" => args.next(),
            None => None,
        };
        match value.as_deref().and_then(Stage::parse) {
            Some(s) => chosen.push(s),
            None => {
                eprintln!("unknown argument {a:?} {value:?}\n{USAGE}");
                return ExitCode::from(64);
            }
        }
    }
    let all = chosen.is_empty();
    let want = |s: Stage| all && Stage::ALL.contains(&s) || chosen.contains(&s);
    // Fixed order: cheap checks first. `test` already includes the doc tests.
    let plan: [(Stage, fn() -> bool); 4] = [
        (Stage::Lint, lint),
        (Stage::Test, test),
        (Stage::Doctest, doctest),
        (Stage::Replay, replay),
    ];
    for (stage, f) in plan {
        if !want(stage) || (stage == Stage::Doctest && want(Stage::Test)) {
            continue;
        }
        if !f() {
            return ExitCode::FAILURE;
        }
    }
    eprintln!("all checks passed");
    ExitCode::SUCCESS
}
