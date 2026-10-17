//! Data panel administration: project members, the organization window
//! (members and storage usage), the project trash and the offline queue.
//!
//! The server decides every permission; the client only hides controls the
//! caller's role cannot use (`Project::my_role`, `OrgSummary::my_role`) and
//! shows the server's refusals inline.

use eframe::egui::{self, RichText};
use rcad_cloud::proto::{self, ErrorCode, OrgRole, ProjectRole};
use rcad_cloud::service::Request;

use super::{Action, CloudDialog, field, fmt_time, now_secs};
use crate::app::RcadApp;
use crate::probe::Probe as _;

pub(super) const PROJECT_ROLES: [ProjectRole; 4] = [
    ProjectRole::Viewer,
    ProjectRole::Commenter,
    ProjectRole::Editor,
    ProjectRole::Admin,
];
pub(super) const ORG_ROLES: [OrgRole; 4] = [
    OrgRole::Guest,
    OrgRole::Member,
    OrgRole::Admin,
    OrgRole::Owner,
];

pub fn project_role_name(r: ProjectRole) -> &'static str {
    match r {
        ProjectRole::Viewer => "Viewer",
        ProjectRole::Commenter => "Commenter",
        ProjectRole::Editor => "Editor",
        ProjectRole::Admin => "Admin",
    }
}

pub fn org_role_name(r: OrgRole) -> &'static str {
    match r {
        OrgRole::Guest => "Guest",
        OrgRole::Member => "Member",
        OrgRole::Admin => "Admin",
        OrgRole::Owner => "Owner",
    }
}

/// "1.5 MB" (decimal units, like the server's quota settings).
pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u + 1 < UNITS.len() {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// The text shown for a refused member change, explaining the usual causes.
pub fn member_error_text(request: &str, code: Option<ErrorCode>, message: &str) -> String {
    match (request, code) {
        ("set_project_member", Some(ErrorCode::NotFound)) => format!(
            "Not found ({message}). The user must be a member of the organization first; \
             an organization administrator adds them under Organization…"
        ),
        ("set_org_member", Some(ErrorCode::NotFound)) => {
            format!(
                "Not found ({message}). Check the username: it must be an account on this server."
            )
        }
        (_, Some(ErrorCode::Forbidden)) => format!(
            "Not allowed ({message}). Only administrators change members; \
             only an owner manages other administrators and owners."
        ),
        (_, Some(ErrorCode::NotFound)) => format!("Not found ({message})."),
        _ => message.to_string(),
    }
}

fn member_request(request: &str) -> bool {
    matches!(
        request,
        "load_project_members" | "set_project_member" | "remove_project_member"
    )
}

fn org_request(request: &str) -> bool {
    matches!(
        request,
        "load_org_members" | "set_org_member" | "remove_org_member" | "load_usage"
    )
}

impl RcadApp {
    /// Role of the signed-in user in `project` (from the loaded tree or project list).
    pub(super) fn cloud_project_role(&self, project: &str) -> Option<ProjectRole> {
        let c = &self.cloud;
        c.tree
            .as_ref()
            .filter(|t| t.project.id == project)
            .map(|t| t.project.my_role)
            .or_else(|| {
                c.projects
                    .iter()
                    .find(|p| p.id == project)
                    .map(|p| p.my_role)
            })
    }

    /// Role of the signed-in user in `org`.
    pub(super) fn cloud_org_role(&self, org: &str) -> Option<OrgRole> {
        self.cloud
            .account
            .as_ref()?
            .me
            .orgs
            .iter()
            .find(|o| o.id == org)
            .map(|o| o.my_role)
    }

    pub(super) fn cloud_open_members(&mut self, project: String) {
        self.cloud.members = None;
        self.cloud.admin_error_code = None;
        self.cloud.dialog = Some(CloudDialog::Members {
            project: project.clone(),
            username: String::new(),
            role: ProjectRole::Editor,
            error: None,
        });
        self.cloud_send(Request::LoadProjectMembers { project });
    }

    pub(super) fn cloud_open_organization(&mut self, org: String) {
        self.cloud.org_members = None;
        self.cloud.admin_error_code = None;
        self.cloud.usage = None;
        self.cloud.org_activity = None;
        let admin = self.cloud_org_role(&org) >= Some(OrgRole::Admin);
        self.cloud.dialog = Some(CloudDialog::Organization {
            org: org.clone(),
            username: String::new(),
            role: OrgRole::Member,
            error: None,
        });
        self.cloud_send(Request::LoadOrgMembers { org: org.clone() });
        self.cloud_send(Request::LoadUsage { org: org.clone() });
        if admin {
            self.cloud_send(Request::LoadOrgActivity { org });
        }
    }

    /// Show a failed member or organization request in its open window.
    /// Returns whether it was shown there.
    pub(super) fn cloud_admin_error(
        &mut self,
        request: &str,
        code: Option<ErrorCode>,
        message: &str,
    ) -> bool {
        let code_name = code
            .and_then(|c| serde_json::to_value(c).ok())
            .and_then(|v| v.as_str().map(String::from));
        let shown = match &mut self.cloud.dialog {
            Some(CloudDialog::Members { error, .. }) if member_request(request) => {
                *error = Some(member_error_text(request, code, message));
                true
            }
            Some(CloudDialog::Organization { error, .. }) if org_request(request) => {
                *error = Some(member_error_text(request, code, message));
                true
            }
            _ => false,
        };
        if shown {
            self.cloud.admin_error_code = code_name;
        }
        shown
    }

    /// The Members window of a project.
    pub(super) fn cloud_members_window(&mut self, ctx: &egui::Context) {
        let Some(CloudDialog::Members {
            project,
            mut username,
            mut role,
            mut error,
        }) = self.cloud.dialog.clone()
        else {
            return;
        };
        let admin = self.cloud_project_role(&project) == Some(ProjectRole::Admin);
        let name = self
            .cloud
            .projects
            .iter()
            .find(|p| p.id == project)
            .map(|p| p.name.clone())
            .or_else(|| self.cloud.tree.as_ref().map(|t| t.project.name.clone()))
            .unwrap_or_else(|| project.clone());
        let members = self
            .cloud
            .members
            .as_ref()
            .filter(|(p, _)| *p == project)
            .map(|(_, m)| m.clone());
        let me = self
            .cloud
            .account
            .as_ref()
            .map(|a| a.me.user.username.clone())
            .unwrap_or_default();
        let mut sends: Vec<Request> = vec![];
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Window::new(format!("Members: {name}"))
            .id(egui::Id::new("cloud_members"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .show(ctx, |ui| {
                ui.small(
                    "Organization administrators manage every project without being listed here.",
                );
                match &members {
                    None => {
                        ui.weak("Loading…");
                    }
                    Some(list) if list.is_empty() => {
                        ui.weak("No members are listed explicitly.");
                    }
                    Some(list) => {
                        egui::Grid::new("cloud_members_grid")
                            .num_columns(3)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                for m in list {
                                    let u = &m.user.username;
                                    let you = if *u == me { " (you)" } else { "" };
                                    ui.label(format!("{}{you}", m.user.display_name))
                                        .on_hover_text(u);
                                    if admin {
                                        let mut r = m.role;
                                        egui::ComboBox::from_id_salt(("member_role", u))
                                            .selected_text(project_role_name(r))
                                            .show_ui(ui, |ui| {
                                                for x in PROJECT_ROLES {
                                                    ui.selectable_value(
                                                        &mut r,
                                                        x,
                                                        project_role_name(x),
                                                    );
                                                }
                                            })
                                            .response
                                            .named(&format!("Role of {u}"));
                                        if r != m.role {
                                            sends.push(Request::SetProjectMember {
                                                project: project.clone(),
                                                username: u.clone(),
                                                role: r,
                                            });
                                        }
                                        if ui
                                            .small_button("Remove")
                                            .named(&format!("Remove {u}"))
                                            .on_hover_text("Remove this member from the project")
                                            .clicked()
                                        {
                                            sends.push(Request::RemoveProjectMember {
                                                project: project.clone(),
                                                username: u.clone(),
                                            });
                                        }
                                    } else {
                                        ui.label(project_role_name(m.role));
                                        ui.label("");
                                    }
                                    ui.end_row();
                                }
                            });
                    }
                }
                if admin {
                    ui.separator();
                    crate::form::form(ui, "cloud_members_add", |ui| {
                        field(ui, "Member username", &mut username, false);
                        crate::form::combo(ui, "New member role", project_role_name(role), |ui| {
                            for x in PROJECT_ROLES {
                                ui.selectable_value(&mut role, x, project_role_name(x));
                            }
                        });
                    });
                    if ui
                        .add_enabled(!username.trim().is_empty(), egui::Button::new("Add member"))
                        .probe("Add member")
                        .clicked()
                    {
                        error = None;
                        sends.push(Request::SetProjectMember {
                            project: project.clone(),
                            username: username.trim().to_string(),
                            role,
                        });
                        username.clear();
                    }
                } else {
                    ui.weak("Only project administrators change members.");
                }
                if let Some(e) = &error {
                    ui.colored_label(crate::theme::of(ui).bad, e.as_str());
                }
                ui.horizontal(|ui| {
                    if ui.button("Reload").named("Reload members").clicked() {
                        sends.push(Request::LoadProjectMembers {
                            project: project.clone(),
                        });
                    }
                    close |= ui.button("Close").named("Close members").clicked();
                });
            });
        if !sends.is_empty() {
            error = None;
            self.cloud.admin_error_code = None;
        }
        if matches!(self.cloud.dialog, Some(CloudDialog::Members { .. })) {
            self.cloud.dialog = if close {
                None
            } else {
                Some(CloudDialog::Members {
                    project,
                    username,
                    role,
                    error,
                })
            };
        }
        for r in sends {
            self.cloud_send(r);
        }
    }

    /// The Organization window (administrators): members, roles and storage.
    pub(super) fn cloud_org_window(&mut self, ctx: &egui::Context) {
        let Some(CloudDialog::Organization {
            org,
            mut username,
            mut role,
            mut error,
        }) = self.cloud.dialog.clone()
        else {
            return;
        };
        let my_role = self.cloud_org_role(&org);
        let owner = my_role == Some(OrgRole::Owner);
        let admin = my_role >= Some(OrgRole::Admin);
        // Owners manage every role; administrators manage guests and members.
        let assignable: Vec<OrgRole> = ORG_ROLES
            .into_iter()
            .filter(|r| owner || *r <= OrgRole::Member)
            .collect();
        let name = self
            .cloud
            .account
            .as_ref()
            .and_then(|a| a.me.orgs.iter().find(|o| o.id == org))
            .map_or(org.clone(), |o| o.name.clone());
        let members = self
            .cloud
            .org_members
            .as_ref()
            .filter(|(o, _)| *o == org)
            .map(|(_, m)| m.clone());
        let usage = self
            .cloud
            .usage
            .as_ref()
            .filter(|(o, _)| *o == org)
            .map(|(_, u)| u.clone());
        let me = self
            .cloud
            .account
            .as_ref()
            .map(|a| a.me.user.username.clone())
            .unwrap_or_default();
        let activity = self
            .cloud
            .org_activity
            .as_ref()
            .filter(|(o, _)| *o == org)
            .map(|(_, a)| a.clone());
        let mut sends: Vec<Request> = vec![];
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Window::new(format!("Organization: {name}"))
            .id(egui::Id::new("cloud_org"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .show(ctx, |ui| {
                ui.strong("Storage");
                match &usage {
                    None => {
                        ui.weak("Loading…");
                    }
                    Some(u) => usage_ui(ui, u),
                }
                if admin {
                    ui.separator();
                    ui.strong("Recent activity");
                    match &activity {
                        None => {
                            ui.weak("Loading…");
                        }
                        Some(list) => {
                            egui::ScrollArea::vertical()
                                .id_salt("cloud_org_activity")
                                .max_height(160.0)
                                .show(ui, |ui| {
                                    if let Some(document) = super::activity::activity_list(
                                        ui,
                                        list,
                                        now_secs(),
                                        "Organization activity entry",
                                    ) {
                                        sends.push(Request::OpenDocument { document });
                                        close = true;
                                    }
                                });
                        }
                    }
                }
                ui.separator();
                ui.strong("Members");
                match &members {
                    None => {
                        ui.weak("Loading…");
                    }
                    Some(list) => {
                        egui::Grid::new("cloud_org_grid")
                            .num_columns(3)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                for m in list {
                                    let u = &m.user.username;
                                    let you = if *u == me { " (you)" } else { "" };
                                    ui.label(format!("{}{you}", m.user.display_name))
                                        .on_hover_text(u);
                                    let editable = admin && (owner || m.role <= OrgRole::Member);
                                    if editable {
                                        let mut r = m.role;
                                        egui::ComboBox::from_id_salt(("org_role", u))
                                            .selected_text(org_role_name(r))
                                            .show_ui(ui, |ui| {
                                                for x in &assignable {
                                                    ui.selectable_value(
                                                        &mut r,
                                                        *x,
                                                        org_role_name(*x),
                                                    );
                                                }
                                            })
                                            .response
                                            .named(&format!("Organization role of {u}"));
                                        if r != m.role {
                                            sends.push(Request::SetOrgMember {
                                                org: org.clone(),
                                                username: u.clone(),
                                                role: r,
                                            });
                                        }
                                        if ui
                                            .small_button("Remove")
                                            .named(&format!("Remove {u} from the organization"))
                                            .on_hover_text(
                                                "Remove from the organization and all its projects",
                                            )
                                            .clicked()
                                        {
                                            sends.push(Request::RemoveOrgMember {
                                                org: org.clone(),
                                                username: u.clone(),
                                            });
                                        }
                                    } else {
                                        ui.label(org_role_name(m.role));
                                        ui.label("");
                                    }
                                    ui.end_row();
                                }
                            });
                    }
                }
                if admin {
                    ui.separator();
                    crate::form::form(ui, "cloud_org_add", |ui| {
                        field(ui, "Organization username", &mut username, false);
                        crate::form::combo(ui, "Organization role", org_role_name(role), |ui| {
                            for x in &assignable {
                                ui.selectable_value(&mut role, *x, org_role_name(*x));
                            }
                        });
                    });
                    ui.small("The account must already exist on this server.");
                    if ui
                        .add_enabled(
                            !username.trim().is_empty(),
                            egui::Button::new("Add to organization"),
                        )
                        .probe("Add to organization")
                        .clicked()
                    {
                        sends.push(Request::SetOrgMember {
                            org: org.clone(),
                            username: username.trim().to_string(),
                            role,
                        });
                        username.clear();
                    }
                }
                if let Some(e) = &error {
                    ui.colored_label(crate::theme::of(ui).bad, e.as_str());
                }
                ui.horizontal(|ui| {
                    if ui.button("Reload").named("Reload organization").clicked() {
                        sends.push(Request::LoadOrgMembers { org: org.clone() });
                        sends.push(Request::LoadUsage { org: org.clone() });
                        if admin {
                            sends.push(Request::LoadOrgActivity { org: org.clone() });
                        }
                    }
                    close |= ui.button("Close").named("Close organization").clicked();
                });
            });
        if !sends.is_empty() {
            error = None;
            self.cloud.admin_error_code = None;
        }
        if matches!(self.cloud.dialog, Some(CloudDialog::Organization { .. })) {
            self.cloud.dialog = if close {
                None
            } else {
                Some(CloudDialog::Organization {
                    org,
                    username,
                    role,
                    error,
                })
            };
        }
        for r in sends {
            self.cloud_send(r);
        }
    }

    /// The project trash (documents restorable by editors).
    pub(super) fn cloud_trash_ui(
        &self,
        ui: &mut egui::Ui,
        tree: &proto::ProjectTree,
        actions: &mut Vec<Action>,
    ) {
        let editor = tree.project.my_role >= ProjectRole::Editor;
        ui.strong("Trash");
        if tree.trash.is_empty() {
            ui.weak("The trash is empty.");
        }
        let now = now_secs();
        for d in &tree.trash {
            ui.horizontal_wrapped(|ui| {
                ui.label(&d.name).on_hover_text(format!(
                    "v{} · updated {} by {}",
                    d.head_version,
                    fmt_time(d.updated_at, now),
                    d.updated_by
                ));
                if ui
                    .add_enabled(editor, egui::Button::new("Restore").small())
                    .named(&format!("Restore {}", d.name))
                    .on_disabled_hover_text("Editors restore documents")
                    .clicked()
                {
                    actions.push(Action::Send(Request::RestoreDocument {
                        document: d.id.clone(),
                    }));
                }
            });
        }
    }

    /// Versions waiting in the offline queue, each with Discard.
    pub(super) fn cloud_queue_ui(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let c = &self.cloud;
        if c.queue.is_empty() {
            return;
        }
        let now = now_secs();
        egui::CollapsingHeader::new(format!("Queued versions ({})", c.queue.len()))
            .id_salt("cloud_queue")
            .default_open(true)
            .show(ui, |ui| {
                for (i, p) in c.queue.iter().enumerate() {
                    let doc = c.doc_name(&p.document);
                    ui.horizontal_wrapped(|ui| {
                        let what = if p.description.is_empty() {
                            doc.clone()
                        } else {
                            format!("{doc}: {}", p.description)
                        };
                        ui.label(RichText::new(&what)).on_hover_text(format!(
                            "{}\nQueued {}",
                            p.local_path.display(),
                            fmt_time(p.queued_at, now)
                        ));
                        ui.weak(fmt_time(p.queued_at, now));
                        if ui
                            .small_button("Discard")
                            .named(&format!("Discard queued version {}", i + 1))
                            .on_hover_text(
                                "Remove this version from the queue (the local file is not changed)",
                            )
                            .clicked()
                        {
                            actions.push(Action::Dialog(CloudDialog::DiscardPending {
                                index: i,
                                document: p.document.clone(),
                                blob: p.blob.clone(),
                                what,
                            }));
                        }
                    });
                }
            });
    }
}

fn usage_ui(ui: &mut egui::Ui, u: &proto::OrgUsage) {
    let counts = format!("{} documents, {} versions", u.documents, u.versions);
    match u.quota_bytes {
        Some(q) if q > 0 => {
            let frac = (u.used_bytes as f64 / q as f64).clamp(0.0, 1.0) as f32;
            ui.add(
                egui::ProgressBar::new(frac)
                    .desired_width(260.0)
                    .text(format!("{} of {}", fmt_bytes(u.used_bytes), fmt_bytes(q))),
            )
            .named("Storage usage");
        }
        _ => {
            ui.label(format!("{} used (no quota)", fmt_bytes(u.used_bytes)))
                .named("Storage usage");
        }
    }
    ui.weak(counts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_error_texts() {
        assert_eq!(fmt_bytes(999), "999 B");
        assert_eq!(fmt_bytes(1_500), "1.5 kB");
        assert_eq!(fmt_bytes(2_000_000_000), "2.0 GB");
        let t = member_error_text("set_project_member", Some(ErrorCode::NotFound), "no user");
        assert!(t.contains("member of the organization first"), "{t}");
        let t = member_error_text("remove_org_member", Some(ErrorCode::Forbidden), "no");
        assert!(t.contains("Not allowed"), "{t}");
        assert_eq!(member_error_text("x", None, "boom"), "boom");
    }
}
