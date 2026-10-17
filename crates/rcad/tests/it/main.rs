//! The integration tests of `rcad` in one test binary (one link instead of one per
//! file). Each file under tests/it/ is a module; run one with
//! `cargo test -p rcad --test it <module>::`.
//!
//! Tests that set process-wide state (the cancellation flag, environment variables,
//! the file-dialog backend) stay separate binaries in tests/, so plain `cargo test`,
//! which runs the tests of one binary as threads of one process, cannot disturb them.

mod assembly_tree_ui;
mod assembly_ui;
mod browser_tree_ui;
#[cfg(feature = "cloud")]
mod cloud_ui;
mod command_registry_ui;
mod config_table_ui;
mod dialog_click;
mod direct_edit_ui;
mod drawings_ui;
mod edit_feature_ui;
mod exchange_ui;
mod extrude_history_ui;
mod face_measure_ui;
mod fillet_loop_ui;
mod first_run_ui;
mod frame_pacing;
mod glyphs;
mod health_ui;
mod holes_ui;
mod isolate_ui;
mod layout_1280;
mod materials_ui;
mod measure_imports;
mod mesh_edit_ui;
mod mesh_repair_ui;
mod multibody;
mod offset_plane_ui;
mod parameters_ui;
mod pattern_grid_ui;
mod pick_semantic;
mod placement;
mod polish_ui;
mod preview_regen;
mod print_export_ui;
mod projection_pick_ui;
mod properties_ui;
mod reattach;
mod recovery_env;
mod region_pick_ui;
mod ribbon_ui;
mod scenes;
mod section_mirror_hole_ui;
mod sheet_surface_ui;
mod shell_ui;
mod sketch_constraints_ui;
mod sketch_curves_ui;
mod sketch_dimension_entry_ui;
mod sketch_fillet_ui;
mod sketch_offset_ui;
mod sketch_pattern_ui;
mod sketch_planes_ui;
mod sketch_project_ui;
mod sketch_shapes_ui;
mod sketch_text_ui;
mod sketch_trim_ui;
mod sketch_workflow;
mod timeline_ui;
mod ui_workflow;
mod value_labels;
mod view_chrome_ui;
