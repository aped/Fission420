//! The 3D view's bottom toolbar (Orbit / Pan / Zoom, Fit view, Look At,
//! Display style) at 1280 × 800 with the side columns wide (Data panel open
//! next to the browser and the inspector): every button stays whole, inside
//! the view's horizontal extent, and clear of the other buttons and of the
//! timeline strip.
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::Rect;
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::Command;

const NAV: [&str; 6] = [
    "Orbit",
    "Pan",
    "Zoom",
    "Fit view",
    "Look At",
    "Display style",
];

fn harness(size: [f32; 2]) -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size(size)
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut RcadApp| app.ui(ui),
            RcadApp::new(ExecMode::Sync, None),
        )
}

fn settle(h: &mut Harness<'_, RcadApp>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Rects of the navigation buttons; panics on overlap or clipping.
fn check_nav(h: &Harness<'_, RcadApp>, what: &str) {
    let view = h.state().view_rect().expect("viewport");
    let screen = h.ctx.content_rect();
    let rects: Vec<Rect> = NAV.iter().map(|l| h.get_by_label(l).rect()).collect();
    for (l, r) in NAV.iter().zip(&rects) {
        assert!(
            r.width() >= 8.0 && r.height() >= 8.0,
            "{what}: {l} collapsed: {r:?}"
        );
        assert!(
            r.min.x >= view.min.x - 0.5 && r.max.x <= view.max.x + 0.5,
            "{what}: {l} {r:?} leaves the 3D view's width {view:?}"
        );
        assert!(
            r.min.y >= view.max.y - 0.5 && r.max.y <= screen.max.y,
            "{what}: {l} {r:?} is not in the strip below the view {view:?}"
        );
    }
    for i in 0..rects.len() {
        for j in i + 1..rects.len() {
            let overlap = rects[i].intersect(rects[j]);
            assert!(
                !(overlap.width() > 0.5 && overlap.height() > 0.5),
                "{what}: {} {:?} overlaps {} {:?}",
                NAV[i],
                rects[i],
                NAV[j],
                rects[j]
            );
        }
    }
}

fn open_example(h: &mut Harness<'_, RcadApp>) {
    h.state_mut().request(Command::Open {
        path: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bracket.rcad.json"),
        safe_mode: false,
    });
    settle(h);
    settle(h);
}

#[test]
fn nav_bar_fits_at_1280_with_the_data_panel_open() {
    let mut h = harness([1280.0, 800.0]);
    open_example(&mut h);
    check_nav(&h, "default columns");
    #[cfg(feature = "cloud")]
    {
        let _service = signed_in_with_a_project(&mut h);
        assert!(h.state().cloud_panel_open());
        check_nav(&h, "Data panel open, signed in");
    }
    #[cfg(not(feature = "cloud"))]
    check_nav(&h, "without the cloud feature");
}

/// Sign in through a detached cloud service, open the Data panel and show a
/// project with a document (the widest Data panel content). Returns the
/// service's channels so they stay open.
#[cfg(feature = "cloud")]
fn signed_in_with_a_project(
    h: &mut Harness<'_, RcadApp>,
) -> (
    std::sync::mpsc::Receiver<rcad_cloud::service::Request>,
    std::sync::mpsc::Sender<rcad_cloud::service::Event>,
) {
    use rcad_cloud::proto;
    use rcad_cloud::service::{CloudService, Event};
    let (svc, req, ev) = CloudService::detached();
    h.state_mut().attach_cloud_service(svc);
    let project = proto::Project {
        id: "p1".into(),
        org: "o1".into(),
        name: "Parts".into(),
        description: String::new(),
        my_role: proto::ProjectRole::Editor,
        created_at: 1,
        revision: 1,
    };
    ev.send(Event::SignedIn {
        server_url: "http://127.0.0.1:18640".into(),
        server_name: "Home lab".into(),
        me: proto::Me {
            user: proto::User {
                id: "u1".into(),
                username: "alice".into(),
                display_name: "Alice".into(),
                server_admin: false,
            },
            orgs: vec![proto::OrgSummary {
                id: "o1".into(),
                name: "Home".into(),
                my_role: proto::OrgRole::Member,
            }],
        },
    })
    .unwrap();
    settle(h);
    h.get_by_label("Data panel").click();
    settle(h);
    ev.send(Event::Projects {
        org: "o1".into(),
        projects: vec![project.clone()],
    })
    .unwrap();
    settle(h);
    h.get_by_label("Project Parts").click();
    settle(h);
    ev.send(Event::Tree(proto::ProjectTree {
        project,
        folders: vec![],
        documents: vec![proto::DocumentSummary {
            id: "d1".into(),
            project: "p1".into(),
            folder: None,
            name: "Bracket with a long descriptive name".into(),
            head_version: 3,
            updated_at: 1,
            updated_by: "alice".into(),
            lock: None,
            has_thumbnail: false,
        }],
        trash: vec![],
    }))
    .unwrap();
    settle(h);
    settle(h);
    (req, ev)
}

#[test]
fn nav_bar_fits_in_a_narrow_window() {
    // Narrower than any default layout: the bar compacts and wraps instead of
    // overlapping or running out of the view.
    let mut h = harness([1000.0, 700.0]);
    open_example(&mut h);
    #[cfg(feature = "cloud")]
    {
        h.get_by_label("Data panel").click();
        settle(&mut h);
    }
    check_nav(&h, "1000 px");
}
