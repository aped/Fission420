//! The command registry drives the command bars, the keymap and the help
//! sheet: every command bar action is a registry entry, keys run the same
//! action as the buttons in both key layers, and the help sheet is generated
//! from the table.
use std::time::{Duration, Instant};

use eframe::egui::Key;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::shell::{KeymapLayer, registry};
use rcad::{ExecMode, RcadApp};

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1600.0, 900.0])
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
        assert!(start.elapsed() < Duration::from_secs(120), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

/// Every action the command bar, its menus and the face/body inspector
/// offered before the registry existed has a registry entry (by label, or
/// by the ribbon command that replaces the inspector-only action).
#[test]
fn every_command_bar_action_is_in_the_registry() {
    let labels: Vec<&str> = registry::COMMANDS.iter().map(|d| d.label).collect();
    for l in [
        "New",
        "Open…",
        "Save",
        "Save As…",
        "Import…",
        "Close",
        "Export STL…",
        "Export STEP…",
        "Export DXF (sketch)…",
        "Export SVG (sketch)…",
        "Export 3MF…",
        "Undo",
        "Redo",
        "Fit view",
        "Section view…",
        "Help",
        "Create Sketch",
        "Sketch on XY",
        "Sketch on XZ",
        "Sketch on YZ",
        "Sketch on offset plane…",
        "Extrude…",
        "Cut through all…",
        "Revolve…",
        "Thread…",
        "Loft…",
        "Sweep…",
        "Rectangle on XY…",
        "Rectangle on XZ…",
        "Circle on face…",
        "Place rectangle",
        "Place hole",
        // Inspector-only actions, now ribbon commands.
        "Hole",
        "Fillet",
        "Chamfer",
        "Shell",
        "Align",
        "Direct Edit",
        "Move/Copy",
        "Scale",
        "Split Body",
        "Combine…",
        "Physical Material",
        "Pattern",
        "Mirror",
        // Commands that had no UI.
        "Export STEP assembly…",
        "Rigid Group",
        "Drive Joints",
        "Drawing Views",
        "Open (safe mode)…",
    ] {
        assert!(labels.contains(&l), "{l} is not a registry command");
    }
}

/// Buttons drawn from the registry carry its label as accessible name and
/// run its action; a disabled one explains why.
#[test]
fn registry_buttons_and_keys_run_the_same_action() {
    let mut h = harness();
    // Without a document Save is disabled (the registry's reason).
    assert_eq!(
        h.state().command_enabled(rcad::shell::CmdId::Save),
        Err("Open or create a document first".into())
    );
    click(&mut h, "New");
    assert!(h.state().summary().open);
    // Fusion layer (default): E opens Extrude once a sketch exists.
    assert_eq!(h.state().keymap(), KeymapLayer::Fusion);
    h.get_by_label("Templates").click();
    let _ = h.try_run();
    click(&mut h, "Rectangle on XY…");
    click(&mut h, "OK");
    assert_eq!(h.state().summary().features.len(), 1);
    h.key_press(Key::E);
    settle(&mut h);
    h.get_by_label("Extrude distance (mm)");
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.query_by_label("Extrude distance (mm)").is_none());
    // Classic layer: E does nothing in model mode.
    h.state_mut().set_keymap(KeymapLayer::Classic);
    h.key_press(Key::E);
    settle(&mut h);
    assert!(h.query_by_label("Extrude distance (mm)").is_none());
    // Fusion F (Fillet) without a face: refused with the reason, nothing opens.
    h.state_mut().set_keymap(KeymapLayer::Fusion);
    h.key_press(Key::F);
    settle(&mut h);
    let last = h.state().diagnostics().last().cloned().expect("message");
    assert!(
        last.text.starts_with("Fillet: Select a planar face"),
        "{}",
        last.text
    );
}

/// The help sheet is generated from the registry for the active layer.
#[test]
fn help_sheet_follows_the_key_layer() {
    let mut h = harness();
    click(&mut h, "Help");
    h.get_by_label_contains("E extrude");
    h.get_by_label_contains("Ctrl+Z undo");
    h.get_by_label_contains("R 2-point rect");
    click(&mut h, "Keymap R-CAD classic");
    assert_eq!(h.state().keymap(), KeymapLayer::Classic);
    assert!(h.query_by_label_contains("E extrude").is_none());
    h.get_by_label_contains("F fit view");
    h.get_by_label_contains("R center rect");
}

/// S opens the command search over the registry: Enter runs the first
/// enabled match, disabled matches keep their reason, Esc closes.
#[test]
fn command_search_runs_registry_commands() {
    let mut h = harness();
    h.key_press(Key::S);
    settle(&mut h);
    h.get_by_label("Search commands").type_text("new");
    let _ = h.try_run();
    h.get_by_label("Search result New");
    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(h.state().summary().open, "Enter ran New");
    assert!(h.query_by_label("Search commands").is_none());
    // A command that cannot run yet is listed disabled.
    click(&mut h, "Command search");
    h.get_by_label("Search commands").type_text("extr");
    let _ = h.try_run();
    assert!(
        h.get_by_label("Search result Extrude…")
            .accesskit_node()
            .is_disabled()
    );
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(h.query_by_label("Search commands").is_none());
}

/// Every former command-bar, Inspector and browser action is reachable from
/// the ribbon, the application bar or its menus, or a context menu, and the
/// selection-dependent ones are enabled exactly when their selection exists.
#[test]
fn every_former_action_is_reachable_with_its_enabled_rule() {
    use rcad::shell::CmdId as C;
    use rcad::shell::registry::Place;
    // Placement: every registry command sits on the ribbon, the app bar, the
    // File / Export / View menus (the former top bar and inspector actions
    // are all registry commands, see every_command_bar_action_is_in_the_registry).
    for d in registry::COMMANDS {
        assert!(
            d.places.iter().any(|p| matches!(
                p,
                Place::Ribbon { .. }
                    | Place::AppBar
                    | Place::File
                    | Place::FileExport
                    | Place::View
            )),
            "{} is not reachable",
            d.label
        );
    }
    let mut h = harness();
    // Without a document: modelling commands are disabled with a reason.
    for id in [
        C::Extrude,
        C::Hole,
        C::Fillet,
        C::Chamfer,
        C::Shell,
        C::MoveCopy,
        C::SplitBody,
        C::Combine,
        C::Material,
        C::Pattern,
        C::Mirror,
        C::Align,
        C::DirectEdit,
    ] {
        assert!(h.state().command_enabled(id).is_err(), "{id:?}");
    }
    click(&mut h, "New");
    h.get_by_label("Templates").click();
    let _ = h.try_run();
    click(&mut h, "Rectangle on XY…");
    click(&mut h, "OK");
    click(&mut h, "Extrude…");
    click(&mut h, "OK");
    // Face commands need a face (Shell is enabled by the selected New Body
    // extrusion: its scoped shell).
    assert_eq!(h.state().command_enabled(C::Shell), Ok(()));
    for id in [C::Hole, C::Fillet, C::Chamfer] {
        assert!(
            h.state().command_enabled(id).is_err(),
            "{id:?} without a face"
        );
    }
    let top = h
        .state()
        .faces()
        .iter()
        .find(|f| {
            f.plane
                .is_some_and(|p| p.normal[2] > 0.99 && p.point[2] > 0.0)
        })
        .map(|f| f.group)
        .expect("top face");
    h.state_mut().select_face(Some(top));
    let _ = h.try_run();
    for id in [
        C::Hole,
        C::Fillet,
        C::Chamfer,
        C::Shell,
        C::MoveCopy,
        C::Measure,
    ] {
        assert_eq!(h.state().command_enabled(id), Ok(()), "{id:?} with a face");
    }
    // Browser actions: History row context menu and the timeline marker menu.
    h.state_mut().select_face(None);
    settle(&mut h);
    h.get_by_label("Extrude1").click_secondary();
    settle(&mut h);
    h.get_by_label("Edit Feature Extrude1");
    h.get_by_label("Rename…");
    h.get_by_label("Delete feature");
    h.key_press(Key::Escape);
    settle(&mut h);
    h.get_by_label("Sketch1").click_secondary();
    settle(&mut h);
    h.get_by_label("Edit Sketch");
    h.key_press(Key::Escape);
    settle(&mut h);
    h.get_by_label("Timeline Extrude1").click_secondary();
    settle(&mut h);
    h.get_by_label("Roll History Marker Here");
}
