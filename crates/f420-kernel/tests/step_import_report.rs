//! Human-readable import report over every STEP fixture (timings, units, stats,
//! volumes, planar face frames, tessellation check). Not an assertion test:
//! `cargo test --release -p f420-kernel --test step_import_report -- --ignored --nocapture`
use f420_kernel::*;
#[test]
#[ignore]
fn report() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/step");
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map(|e| e == "step").unwrap_or(false))
        .collect();
    names.sort();
    for p in names {
        let text = std::fs::read_to_string(&p).unwrap();
        let t = std::time::Instant::now();
        let r = import_step(&text, &StepImportOptions::default());
        let dt = t.elapsed();
        println!("== {} ({:?})", p.file_name().unwrap().to_string_lossy(), dt);
        match r {
            Err(e) => println!("  ERR {e}"),
            Ok(imp) => {
                println!(
                    "  unit {:?} warnings {:?}\n  stats {:?}",
                    imp.length_unit, imp.warnings, imp.stats
                );
                for s in &imp.solids {
                    let m = measure(&s.body).unwrap();
                    println!(
                        "  solid {:?} faces {} vol {:?} bounds {:?}",
                        s.name, m.face_count, m.volume, m.bounds
                    );
                    for f in &s.planar_faces {
                        println!(
                            "    pf {} area {:.6} o {:?} z {:?}",
                            f.index, f.area, f.frame.origin, f.frame.z
                        );
                    }
                    let mesh = tessellate(&s.body, 0.05).map(|m| check_mesh(&m));
                    match mesh {
                        Ok(c) => {
                            println!("  mesh watertight {} vol {}", c.watertight, c.mesh_volume)
                        }
                        Err(e) => println!("  mesh ERR {e}"),
                    }
                }
            }
        }
    }
}
