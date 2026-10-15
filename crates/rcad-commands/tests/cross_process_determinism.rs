//! Same document, same result in every process. The seeded arc corpus (the
//! round and rounded cuts of `arc_fuzz.rs`) is regenerated in several child
//! processes of this test binary. Every child must accept and refuse the same
//! cuts, give bit-identical volumes, identical refusal messages and identical
//! STEP text (apart from the export time in its header) for every accepted
//! result.
//!
//! Address-space layout and the standard library's hash seeds differ between
//! processes, so any ordering derived from pointers or from randomly seeded
//! hash maps shows up here as a differing line.
mod arc_corpus;

use arc_corpus::*;
use rcad_document::BodyId;
use std::process::Command as Proc;

const CHILD_ENV: &str = "RCAD_DETERMINISM_CHILD";
const PROCESSES: usize = 5;
const TEST_NAME: &str = "arc_corpus_is_identical_across_processes";

/// FNV-1a, enough to compare texts between processes.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// One line per corpus feature: outcome, exact volume bits and STEP hash, or the
/// refusal message.
fn child_lines() -> Vec<String> {
    let all = features(None);
    let mut lines = Vec::new();
    // Diagnosis: RCAD_DETERMINISM_CASE=k runs one case of the whole-corpus stream.
    let only: Option<usize> = std::env::var("RCAD_DETERMINISM_CASE")
        .ok()
        .and_then(|v| v.parse().ok());
    for case in 0..CASES {
        if only.is_some_and(|k| k != case) || !all.iter().any(|f| f.case == case) {
            continue;
        }
        let (mut s, top) = plate();
        for f in all.iter().filter(|f| f.case == case) {
            let r = cut(&mut s, top, f);
            let line = if r.ok {
                let model = s.model().unwrap();
                let body = model
                    .evaluation()
                    .body_by_id(BodyId(1))
                    .unwrap()
                    .geometry
                    .brep()
                    .unwrap();
                let text = rcad_kernel::export_step(body, "det");
                // Diagnosis: RCAD_DETERMINISM_DUMP=<dir> keeps each child's STEP files.
                if let (Some(dir), Ok(t)) = (std::env::var_os("RCAD_DETERMINISM_DUMP"), &text) {
                    let dir = std::path::Path::new(&dir).join(std::process::id().to_string());
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(dir.join(format!("case{case:02}_{}.step", f.n)), t).unwrap();
                }
                // The header's FILE_NAME carries the export time.
                let step = text.map(|t| {
                    fnv(&t
                        .lines()
                        .filter(|l| !l.starts_with("FILE_NAME("))
                        .collect::<Vec<_>>()
                        .join("\n"))
                });
                format!(
                    "case {case} #{} ok volume={:016x} step={step:x?}",
                    f.n,
                    vol(&s).to_bits()
                )
            } else {
                format!("case {case} #{} refused {:?}", f.n, r.error)
            };
            lines.push(line);
        }
    }
    lines
}

#[test]
fn arc_corpus_is_identical_across_processes() {
    if std::env::var_os(CHILD_ENV).is_some() {
        for l in child_lines() {
            println!("DET {l}");
        }
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..PROCESSES)
        .map(|_| {
            Proc::new(&exe)
                .args([TEST_NAME, "--exact", "--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, "1")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn child")
        })
        .collect();
    let runs: Vec<Vec<String>> = children
        .into_iter()
        .map(|c| {
            let out = c.wait_with_output().unwrap();
            assert!(out.status.success(), "child failed: {:?}", out.status);
            String::from_utf8(out.stdout)
                .unwrap()
                .lines()
                .filter_map(|l| l.strip_prefix("DET ").map(str::to_string))
                .collect()
        })
        .collect();
    let n = runs[0].len();
    assert!(n > 100, "corpus too small: {n} features");
    let mut differing = Vec::new();
    for i in 0..n {
        let first = &runs[0][i];
        if runs.iter().any(|r| r.get(i) != Some(first)) {
            differing.push(
                runs.iter()
                    .enumerate()
                    .map(|(p, r)| format!("  process {p}: {}", r.get(i).map_or("<missing>", |s| s)))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
    let accepted = runs[0].iter().filter(|l| l.contains(" ok ")).count();
    eprintln!(
        "DETERMINISM arcs: processes={PROCESSES} features={n} accepted={accepted} differing={}",
        differing.len()
    );
    assert!(
        runs.iter().all(|r| r.len() == n),
        "processes ran different numbers of features"
    );
    assert!(
        differing.is_empty(),
        "{} of {n} corpus features differ between processes:\n{}",
        differing.len(),
        differing.join("\n")
    );
}
