//! `cargo xtask check`: formatting, lints, tests (release), headless acceptance.
//! Exits non-zero on the first failing stage; never masks failures.
use std::process::{Command, ExitCode};

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
    ];
    for (name, args) in stages {
        if !run(name, args) {
            return ExitCode::FAILURE;
        }
    }
    eprintln!("all checks passed");
    ExitCode::SUCCESS
}
