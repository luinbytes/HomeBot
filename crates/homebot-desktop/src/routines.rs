//! Server-projected routine list, editor, and demonstration recording surfaces.

use crate::tokens::HomeBotTheme;
use egui::{Align, Frame, Layout, RichText, Stroke, Ui};
use homebot_protocol::{
    RoutineRecordingSummary, RoutineRunSummary, RoutineSummary, ServerEvent, ServerEventBody,
};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Read-only desktop cache hydrated from authenticated server responses/events.
#[derive(Clone, Debug, Default)]
pub struct RoutineProjection {
    routines: BTreeMap<Uuid, RoutineSummary>,
    recordings: BTreeMap<Uuid, RoutineRecordingSummary>,
    runs: BTreeMap<Uuid, Vec<RoutineRunSummary>>,
}

impl RoutineProjection {
    pub fn hydrate(&mut self, routines: Vec<RoutineSummary>) {
        self.routines = routines
            .into_iter()
            .map(|routine| (routine.id, routine))
            .collect();
    }

    pub fn apply(&mut self, event: &ServerEvent) {
        match &event.body {
            ServerEventBody::RoutineChanged { routine } => {
                self.routines.insert(routine.id, routine.clone());
            }
            ServerEventBody::RoutineRemoved { routine_id } => {
                self.routines.remove(routine_id);
                self.runs.remove(routine_id);
            }
            ServerEventBody::RoutineRecordingChanged { recording } => {
                self.recordings.insert(recording.id, recording.clone());
            }
            ServerEventBody::RoutineRunChanged { run } => {
                let runs = self.runs.entry(run.routine_id).or_default();
                if let Some(existing) = runs.iter_mut().find(|existing| existing.id == run.id) {
                    *existing = run.clone();
                } else {
                    runs.push(run.clone());
                }
                runs.sort_by_key(|item| std::cmp::Reverse(item.started_at_unix_ms));
            }
            _ => {}
        }
    }

    pub fn routines(&self) -> impl Iterator<Item = &RoutineSummary> {
        self.routines.values()
    }

    #[must_use]
    pub fn recording(&self, id: Uuid) -> Option<&RoutineRecordingSummary> {
        self.recordings.get(&id)
    }

    #[must_use]
    pub fn runs(&self, routine_id: Uuid) -> &[RoutineRunSummary] {
        self.runs.get(&routine_id).map_or(&[], Vec::as_slice)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutineSurface {
    List,
    Editor,
    Recording,
}

pub fn routine_surface(ui: &mut Ui, theme: HomeBotTheme, surface: RoutineSurface) {
    ui.vertical_centered(|ui| {
        ui.set_max_width(720.0);
        ui.add_space(theme.spacing.xxl);
        match surface {
            RoutineSurface::List => list(ui, theme),
            RoutineSurface::Editor => editor(ui, theme),
            RoutineSurface::Recording => recording(ui, theme),
        }
    });
}

fn heading(ui: &mut Ui, theme: HomeBotTheme, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new(title)
                    .font(theme.typography.font(theme.typography.title))
                    .strong(),
            );
            ui.label(RichText::new(subtitle).color(theme.palette.text_secondary));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let _ = ui.button("+");
        });
    });
    ui.add_space(theme.spacing.xl);
}

fn list(ui: &mut Ui, theme: HomeBotTheme) {
    heading(
        ui,
        theme,
        "Routines",
        "Repeat useful work without repeating yourself.",
    );
    routine_row(
        ui,
        theme,
        "Morning intelligence",
        "Nova · 3 structured steps",
        "Enabled",
        theme.palette.success,
    );
    ui.add_space(theme.spacing.sm);
    routine_row(
        ui,
        theme,
        "Publish weekly notes",
        "Nova · Draft",
        "Edit",
        theme.palette.warning,
    );
}

fn routine_row(
    ui: &mut Ui,
    theme: HomeBotTheme,
    name: &str,
    detail: &str,
    status: &str,
    color: egui::Color32,
) {
    Frame::NONE
        .fill(theme.palette.surface)
        .stroke(Stroke::new(1.0_f32, theme.palette.border))
        .corner_radius(theme.radii.md)
        .inner_margin(egui::Margin::same(theme.insets.lg))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.strong(name);
                    ui.label(RichText::new(detail).color(theme.palette.text_secondary));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let _ = ui.button("•••");
                    ui.colored_label(color, status);
                });
            });
        });
}

fn editor(ui: &mut Ui, theme: HomeBotTheme) {
    heading(
        ui,
        theme,
        "Edit routine",
        "Version 2 · Changes create a new recorded version",
    );
    ui.strong("Name");
    let mut name = "Morning intelligence".to_owned();
    let _ = ui.text_edit_singleline(&mut name);
    ui.add_space(theme.spacing.lg);
    ui.strong("Steps");
    for (number, title, detail) in [
        ("1", "Ask Nova", "Summarise overnight updates"),
        ("2", "Repository tools · repo_status", "Approval required"),
        ("3", "Record output", "morning_brief"),
    ] {
        routine_row(
            ui,
            theme,
            &format!("{number}  {title}"),
            detail,
            "Edit",
            theme.palette.text_secondary,
        );
        ui.add_space(theme.spacing.sm);
    }
    ui.horizontal(|ui| {
        let _ = ui.button("Dry run");
        let _ = ui.button("Run now");
        let _ = ui.button("Save draft");
        let _ = ui.button("Publish");
    });
}

fn recording(ui: &mut Ui, theme: HomeBotTheme) {
    heading(
        ui,
        theme,
        "Teach a routine",
        "Show HomeBot once. Review every step before saving.",
    );
    ui.colored_label(theme.palette.danger, "● Recording  00:42");
    ui.add_space(theme.spacing.lg);
    routine_row(
        ui,
        theme,
        "1  You asked Nova",
        "Summarise overnight updates",
        "Captured",
        theme.palette.success,
    );
    ui.add_space(theme.spacing.sm);
    routine_row(
        ui,
        theme,
        "2  Nova used Repository tools",
        "repo_status · approval preserved",
        "Captured",
        theme.palette.success,
    );
    ui.add_space(theme.spacing.xl);
    ui.label(RichText::new("Only structured actions are recorded. Mouse coordinates, secret values and raw credentials are never captured.").color(theme.palette.text_secondary));
    ui.add_space(theme.spacing.lg);
    ui.horizontal(|ui| {
        let _ = ui.button("Cancel");
        let _ = ui.button("Stop and review");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use homebot_protocol::{PROTOCOL_VERSION, RoutineDefinition};

    fn routine(id: Uuid, name: &str) -> RoutineSummary {
        RoutineSummary {
            id,
            bot_id: Uuid::nil(),
            name: name.to_owned(),
            description: String::new(),
            enabled: false,
            draft: true,
            active_version_id: Uuid::now_v7(),
            version: 1,
            definition: RoutineDefinition {
                inputs: Vec::new(),
                steps: Vec::new(),
                expected_outputs: Vec::new(),
            },
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn event(body: ServerEventBody) -> ServerEvent {
        ServerEvent {
            protocol_version: PROTOCOL_VERSION,
            sequence: 1,
            event_id: Uuid::now_v7(),
            body,
        }
    }

    #[test]
    fn projection_is_hydrated_and_changed_only_by_server_contracts() {
        let id = Uuid::now_v7();
        let mut projection = RoutineProjection::default();
        projection.hydrate(vec![routine(id, "Morning brief")]);
        assert_eq!(
            projection.routines().next().map(|item| item.name.as_str()),
            Some("Morning brief")
        );
        projection.apply(&event(ServerEventBody::RoutineChanged {
            routine: routine(id, "Morning intelligence"),
        }));
        assert_eq!(
            projection.routines().next().map(|item| item.name.as_str()),
            Some("Morning intelligence")
        );
        projection.apply(&event(ServerEventBody::RoutineRemoved { routine_id: id }));
        assert_eq!(projection.routines().count(), 0);
    }
}
