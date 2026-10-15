//! Byte-for-byte determinism of exports and saves (2026-10-12). Truck keys its
//! topology by pointer-derived ids, so face/edge order inside a regenerated B-rep
//! varies between runs (and between sessions in one process). Exports must
//! nevertheless be identical bytes for the same document and settings, and so
//! must saved documents. Each example is built twice in fresh sessions here.
use rcad_commands::printables;
use rcad_commands::{Command, Session};
use std::path::{Path, PathBuf};

fn build(steps: Vec<rcad_commands::scenarios::Step>) -> Session {
    let mut s = Session::new();
    for st in steps {
        if matches!(st.cmd, Command::Save { .. } | Command::Inspect) {
            continue;
        }
        let r = s.execute(&st.cmd);
        assert_eq!(r.ok, st.expect_ok, "{:?}: {:?}", st.cmd, r.error);
    }
    s
}

fn outputs(s: &mut Session, dir: &Path, tag: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (name, cmd) in [
        (
            "stl",
            Command::ExportStl {
                path: dir.join(format!("{tag}.stl")),
                chord: None,
                bodies: None,
                print: Default::default(),
            },
        ),
        (
            "3mf",
            Command::Export3mf {
                path: dir.join(format!("{tag}.3mf")),
                chord: None,
                bodies: None,
                unit: None,
                print: Default::default(),
            },
        ),
        (
            "step",
            Command::ExportStep {
                path: dir.join(format!("{tag}.step")),
                bodies: None,
            },
        ),
        (
            "rcad",
            Command::Save {
                path: dir.join(format!("{tag}.rcad.json")),
            },
        ),
    ] {
        let r = s.execute(&cmd);
        assert!(r.ok, "{name}: {:?}", r.error);
        let ext = match name {
            "rcad" => "rcad.json",
            e => e,
        };
        let p: PathBuf = dir.join(format!("{tag}.{ext}"));
        let mut bytes = std::fs::read(&p).unwrap();
        if name == "step" {
            // The header time stamp is the export time unless SOURCE_DATE_EPOCH
            // is set (rcad_kernel::export_step); everything else must match.
            let text = String::from_utf8(bytes).unwrap();
            bytes = text
                .lines()
                .filter(|l| !l.starts_with("FILE_NAME("))
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes();
        }
        out.push((name.to_string(), bytes));
    }
    out
}

fn check(name: &str, mk: fn(&Path) -> Vec<rcad_commands::scenarios::Step>) {
    let dir = tempfile::tempdir().unwrap();
    let a = outputs(&mut build(mk(Path::new("x"))), dir.path(), "a");
    let b = outputs(&mut build(mk(Path::new("x"))), dir.path(), "b");
    let mut diffs = Vec::new();
    for ((k, x), (_, y)) in a.iter().zip(&b) {
        // The native file names its own path nowhere, so bytes must match.
        if x != y {
            let first = x
                .iter()
                .zip(y)
                .position(|(p, q)| p != q)
                .unwrap_or(x.len().min(y.len()));
            diffs.push(format!(
                "{name}.{k}: {} vs {} bytes, first difference at byte {first}",
                x.len(),
                y.len()
            ));
        }
    }
    if !diffs.is_empty()
        && let Some(keep) = std::env::var_os("RCAD_KEEP_DIFF")
    {
        let keep = PathBuf::from(keep);
        let _ = std::fs::create_dir_all(&keep);
        for e in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let _ = std::fs::copy(
                e.path(),
                keep.join(format!("{name}-{}", e.file_name().to_string_lossy())),
            );
        }
    }
    assert!(diffs.is_empty(), "{diffs:#?}");
}

#[test]
fn enclosure_exports_and_saves_are_byte_identical() {
    check("enclosure", printables::enclosure_workflow);
}

#[test]
fn coupon_exports_and_saves_are_byte_identical() {
    check("coupon", printables::coupon_workflow);
}

#[test]
fn bracket_exports_and_saves_are_byte_identical() {
    check("bracket", printables::bracket_workflow);
}

#[test]
#[ignore = "diagnostic"]
fn zz_diag_coupon() {
    use std::collections::BTreeSet;
    let mut out = vec![];
    let mut raw: Vec<Vec<[f64; 3]>> = vec![];
    for _ in 0..2 {
        let s = build(printables::coupon_workflow(Path::new("x")));
        let m = s.model().unwrap();
        let b = m.evaluation().bodies[0].geometry.brep().unwrap();
        let mesh = rcad_kernel::tessellate(b, 0.01).unwrap();
        raw.push(mesh.positions.clone());
        let mm = rcad_kernel::measure(b).unwrap();
        let keys: BTreeSet<[i64; 3]> = mesh
            .positions
            .iter()
            .map(|p| p.map(|c| (c * 1e6).round() as i64))
            .collect();
        eprintln!(
            "faces {} edges {} verts {} tris {}",
            mm.face_count,
            mm.edge_count,
            mesh.positions.len(),
            mesh.triangles.len()
        );
        out.push(keys);
    }
    let only_a: Vec<_> = out[0].difference(&out[1]).take(6).collect();
    let only_b: Vec<_> = out[1].difference(&out[0]).take(6).collect();
    let mut worst = 0.0f64;
    let mut hist = [0usize; 8];
    for p in &raw[0] {
        let d = raw[1]
            .iter()
            .map(|q| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt())
            .fold(f64::INFINITY, f64::min);
        worst = worst.max(d);
        let b = if d == 0.0 {
            0
        } else {
            ((-d.log10()) as usize).min(7)
        };
        hist[b] += 1;
    }
    eprintln!(
        "nearest-vertex distance: worst {worst:e}; exact={} buckets(-log10) {:?}",
        hist[0], hist
    );
    eprintln!(
        "only a {} only b {}\n{only_a:?}\n{only_b:?}",
        out[0].difference(&out[1]).count(),
        out[1].difference(&out[0]).count()
    );
}

#[test]
#[ignore = "diagnostic"]
fn zz_diag_bracket() {
    let mut runs = vec![];
    for _ in 0..2 {
        let s = build(printables::bracket_workflow(Path::new("x")));
        let m = s.model().unwrap();
        let b = m.evaluation().bodies[0].geometry.brep().unwrap();
        let mesh = rcad_kernel::tessellate(b, 0.01).unwrap();
        let mut v = mesh.positions.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        runs.push((v, rcad_kernel::measure(b).unwrap()));
    }
    let (a, b) = (&runs[0].0, &runs[1].0);
    eprintln!(
        "verts {} {} faces {} {}",
        a.len(),
        b.len(),
        runs[0].1.face_count,
        runs[1].1.face_count
    );
    let mut worst = 0.0f64;
    let mut n = 0;
    for p in a {
        let d = b
            .iter()
            .map(|q| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt())
            .fold(f64::INFINITY, f64::min);
        if d > 0.0 {
            n += 1;
            if d > worst {
                worst = d;
                eprintln!("  {p:?} d={d:e}");
            }
        }
    }
    eprintln!("differing {n} worst {worst:e}");
}
