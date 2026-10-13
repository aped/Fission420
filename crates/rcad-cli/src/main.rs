//! Headless command-line front end over the shared command API.
//!
//! ```text
//! rcad-cli acceptance [--out DIR]     run the GOAL.md workflow with analytic checks
//! rcad-cli run SCRIPT.json            execute a JSON array of commands, print JSON lines
//! ```
use rcad_commands::{Command, Session, acceptance};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Deserialize)]
struct Step {
    #[serde(flatten)]
    cmd: Command,
    /// Set to false for steps that are expected to be rejected.
    #[serde(default = "yes")]
    expect_ok: bool,
}

fn yes() -> bool {
    true
}

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  rcad-cli acceptance [--out DIR]\n  rcad-cli run SCRIPT.json\n  rcad-cli --version"
    );
    ExitCode::from(64)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("acceptance") => {
            let out = match args.get(1).map(String::as_str) {
                Some("--out") => match args.get(2) {
                    Some(d) => PathBuf::from(d),
                    None => return usage(),
                },
                None => PathBuf::from("artifacts/acceptance"),
                Some(_) => return usage(),
            };
            run_acceptance(out)
        }
        Some("-V" | "--version") => {
            println!("{}", rcad_commands::version_string());
            ExitCode::SUCCESS
        }
        Some("run") => match args.get(1) {
            Some(p) => run_script(PathBuf::from(p)),
            None => usage(),
        },
        _ => usage(),
    }
}

fn run_acceptance(out: PathBuf) -> ExitCode {
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("cannot create {}: {e}", out.display());
        return ExitCode::from(2);
    }
    let t = std::time::Instant::now();
    let report = acceptance::run(&out);
    let mut step = String::new();
    for c in &report.checks {
        if c.step != step {
            step = c.step.clone();
            println!("== {step}");
        }
        println!(
            "  [{}] {}: expected {} | actual {}",
            if c.pass { "PASS" } else { "FAIL" },
            c.name,
            c.expected,
            c.actual
        );
    }
    let path = out.join("acceptance_report.json");
    match serde_json::to_vec_pretty(&report) {
        Ok(j) => {
            if let Err(e) = std::fs::write(&path, j) {
                eprintln!("cannot write {}: {e}", path.display());
            }
        }
        Err(e) => eprintln!("cannot serialize report: {e}"),
    }
    println!(
        "acceptance: {} passed, {} failed in {:.1}s; report {}",
        report.passed,
        report.failed,
        t.elapsed().as_secs_f64(),
        path.display()
    );
    if report.ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_script(path: PathBuf) -> ExitCode {
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let steps: Vec<Step> = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("invalid script {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let mut session = Session::new();
    let mut unexpected = 0;
    for (i, s) in steps.iter().enumerate() {
        let t0 = std::time::Instant::now();
        let mut r = session.execute(&s.cmd);
        r.elapsed_ms = Some(t0.elapsed().as_secs_f64() * 1e3);
        r.summary = None; // keep lines short; use {"cmd":"inspect"} to get the summary
        if matches!(s.cmd, Command::Inspect) {
            r.summary = Some(session.summary());
        }
        let as_expected = r.ok == s.expect_ok;
        if !as_expected {
            unexpected += 1;
        }
        let line = serde_json::json!({ "step": i, "as_expected": as_expected, "response": r });
        println!("{line}");
    }
    if unexpected == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
