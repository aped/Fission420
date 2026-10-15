//! Entity-id rule for trimmed circles: a circle may become
//! an arc with the SAME id when the arc keeps the circle's centre point (the
//! Trim result); any other kind change of an existing id stays refused.
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{DocError, SketchPlane};
use rcad_sketch::{Entity, EntityId, PointId, Sketch};

fn circle_sketch(s: &mut Session) -> (rcad_document::FeatureId, Sketch) {
    assert!(s.execute(&Command::NewDocument).ok);
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredCircle {
            diameter: 20.0,
            center: None,
        },
    });
    assert!(r.ok, "{r:?}");
    let f = r.feature.unwrap();
    let sk = s.summary().features[0].sketch.clone().unwrap();
    (f, sk)
}

/// Replace the circle by a right half arc (same id) plus a closing line.
fn half_disc(sk: &Sketch, center_override: Option<[f64; 2]>) -> Sketch {
    let mut new = sk.clone();
    let (cid, center) = new
        .entities
        .iter()
        .find_map(|e| match e {
            Entity::Circle { id, center, .. } => Some((*id, *center)),
            _ => None,
        })
        .unwrap();
    let c = match center_override {
        Some(xy) => new.add_point(xy[0], xy[1]),
        None => center,
    };
    let cxy = new.point_xy(c).unwrap();
    let a = new.add_point(cxy[0], cxy[1] - 10.0);
    let b = new.add_point(cxy[0], cxy[1] + 10.0);
    new.constraints.retain(|k| !k.entities().contains(&cid));
    let i = new.entities.iter().position(|e| e.id() == cid).unwrap();
    new.entities[i] = Entity::Arc {
        id: cid,
        center: c,
        start: a,
        end: b,
    };
    new.add_line(b, a);
    new
}

#[test]
fn trimmed_circle_keeps_its_id_as_an_arc_about_the_same_centre() {
    let mut s = Session::new();
    let (f, sk) = circle_sketch(&mut s);
    let cid = sk.entities[0].id();
    let r = s.execute(&Command::EditSketch {
        feature: f,
        sketch: half_disc(&sk, None),
    });
    assert!(r.ok, "{:?}", r.error);
    let stored = s.summary().features[0].sketch.clone().unwrap();
    assert!(matches!(stored.entity(cid), Some(Entity::Arc { .. })));
    let prof = stored.profile().expect("half disc closes");
    assert!(matches!(prof, rcad_sketch::Profile::Loop(_)));
}

#[test]
fn other_kind_changes_of_an_id_are_still_refused() {
    // Arc about a different centre point with the circle's id.
    let mut s = Session::new();
    let (f, sk) = circle_sketch(&mut s);
    let rev = s.summary().revision;
    let r = s.execute(&Command::EditSketch {
        feature: f,
        sketch: half_disc(&sk, Some([30.0, 0.0])),
    });
    assert!(
        matches!(&r.error, Some(DocError::Validation(m)) if m.contains("reused")),
        "{r:?}"
    );
    assert_eq!(s.summary().revision, rev);
    // A line given the circle's id.
    let mut line = sk.clone();
    let cid = line.entities[0].id();
    line.entities.clear();
    line.constraints.clear();
    let a = line.add_point(0.0, 0.0);
    let b = line.add_point(10.0, 0.0);
    line.entities.push(Entity::Line { id: cid, a, b });
    let _ = (PointId(0), EntityId(0));
    let r = s.execute(&Command::EditSketch {
        feature: f,
        sketch: line,
    });
    assert!(
        matches!(&r.error, Some(DocError::Validation(m)) if m.contains("reused")),
        "{r:?}"
    );
    assert_eq!(s.summary().revision, rev);
}
