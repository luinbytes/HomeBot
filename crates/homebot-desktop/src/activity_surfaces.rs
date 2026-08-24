//! Remote-safe activity cards driven only by the server protocol.

use egui::{Color32, CornerRadius, Frame, RichText, Sense, Stroke, Ui, Vec2};
use homebot_protocol::{ActivityDetail, ActivitySummary, RiskLevel};
use uuid::Uuid;

use crate::tokens::HomeBotTheme;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivityAction {
    Copy(String),
    OpenArtifact(Uuid),
    ReviewApproval,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityCardModel {
    pub activity: ActivitySummary,
    pub expanded: bool,
}

impl ActivityCardModel {
    #[must_use]
    pub const fn new(activity: ActivitySummary) -> Self {
        Self {
            activity,
            expanded: false,
        }
    }
}

/// Renders a normalized activity without resolving a client-local path.
pub fn activity_surface(
    ui: &mut Ui,
    theme: HomeBotTheme,
    model: &mut ActivityCardModel,
) -> Vec<ActivityAction> {
    let mut actions = Vec::new();
    let risk_color = risk_color(theme, model.activity.presentation.risk);
    Frame::NONE
        .fill(theme.palette.surface)
        .stroke(Stroke::new(theme.layout.hairline, theme.palette.border))
        .corner_radius(CornerRadius::same(theme.radii.sm))
        .inner_margin(egui::Margin::symmetric(theme.insets.sm, theme.insets.sm))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::splat(theme.layout.activity_icon_size),
                    Sense::hover(),
                );
                ui.painter().circle_filled(
                    rect.center(),
                    theme.layout.activity_icon_size / 4.0,
                    risk_color,
                );
                ui.label(
                    RichText::new(&model.activity.title)
                        .font(theme.typography.font(theme.typography.caption))
                        .color(theme.palette.text_primary)
                        .strong(),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(&model.activity.detail)
                            .font(theme.typography.font(theme.typography.micro))
                            .color(theme.palette.text_secondary),
                    )
                    .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(if model.expanded { "⌃" } else { "⌄" })
                        .on_hover_text(if model.expanded {
                            "Hide details"
                        } else {
                            "Show details"
                        })
                        .clicked()
                    {
                        model.expanded = !model.expanded;
                    }
                });
            });
            if model.expanded {
                ui.add_space(theme.spacing.sm);
                ui.separator();
                ui.add_space(theme.spacing.sm);
                egui::ScrollArea::vertical()
                    .max_height(theme.layout.activity_detail_max_height)
                    .show(ui, |ui| {
                        render_detail(ui, theme, &model.activity.presentation.detail);
                    });
                ui.add_space(theme.spacing.sm);
                ui.horizontal(|ui| {
                    if let Some(copy_text) = &model.activity.presentation.copy_text
                        && ui.button("Copy").clicked()
                    {
                        actions.push(ActivityAction::Copy(copy_text.clone()));
                    }
                    if let Some(artifact_id) = model.activity.presentation.open_artifact_id
                        && ui.button("Open artifact").clicked()
                    {
                        actions.push(ActivityAction::OpenArtifact(artifact_id));
                    }
                    if model.activity.requires_attention && ui.button("Review approval").clicked() {
                        actions.push(ActivityAction::ReviewApproval);
                    }
                });
            }
        });
    actions
}

fn render_detail(ui: &mut Ui, theme: HomeBotTheme, detail: &ActivityDetail) {
    let lines = match detail {
        ActivityDetail::Generic { summary } => vec![summary.clone()],
        ActivityDetail::File {
            action,
            workspace_path,
            bytes_changed,
            sha256,
        } => vec![
            format!("{action} · {workspace_path}"),
            bytes_changed.map_or_else(
                || "Size unchanged".to_owned(),
                |bytes| format!("{bytes} bytes changed"),
            ),
            sha256.as_ref().map_or_else(
                || "Digest unavailable".to_owned(),
                |digest| format!("SHA-256 {digest}"),
            ),
        ],
        ActivityDetail::Terminal {
            command,
            working_directory,
            output_preview,
            exit_code,
            truncated,
        } => vec![
            format!("$ {command}"),
            format!("in {working_directory}"),
            format!("{}{}", output_preview, if *truncated { " …" } else { "" }),
            exit_code.map_or_else(|| "Still running".to_owned(), |code| format!("Exit {code}")),
        ],
        ActivityDetail::Browser {
            action,
            url,
            page_title,
            screenshot_artifact_id,
        } => vec![
            format!(
                "{action} · {}",
                page_title.as_deref().unwrap_or("Untitled page")
            ),
            url.clone(),
            if screenshot_artifact_id.is_some() {
                "Screenshot available"
            } else {
                "No screenshot"
            }
            .to_owned(),
        ],
        ActivityDetail::Artifact {
            name,
            media_type,
            size_bytes,
            ..
        } => vec![
            format!("{name} · {media_type}"),
            format!("{size_bytes} bytes"),
        ],
    };
    for line in lines {
        ui.label(
            RichText::new(line)
                .font(theme.typography.font(theme.typography.caption))
                .color(theme.palette.text_secondary),
        );
    }
}

const fn risk_color(theme: HomeBotTheme, risk: RiskLevel) -> Color32 {
    match risk {
        RiskLevel::Low => theme.palette.success,
        RiskLevel::Elevated => theme.palette.warning,
        RiskLevel::High => theme.palette.danger,
    }
}

#[cfg(test)]
mod tests {
    use homebot_protocol::{ActivityKind, ActivityPresentation, ActivityStatus};

    use super::*;

    #[test]
    fn actions_are_server_identifiers_and_content_not_local_paths() {
        let artifact = Uuid::now_v7();
        let activity = ActivitySummary {
            id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            message_id: None,
            title: "Generated report".to_owned(),
            detail: "report.md".to_owned(),
            kind: ActivityKind::Artifact,
            presentation: ActivityPresentation {
                risk: RiskLevel::Low,
                detail: ActivityDetail::Artifact {
                    artifact_id: artifact,
                    name: "report.md".to_owned(),
                    media_type: "text/markdown".to_owned(),
                    size_bytes: 42,
                },
                copy_text: Some("summary".to_owned()),
                open_artifact_id: Some(artifact),
            },
            status: ActivityStatus::Succeeded,
            requires_attention: false,
            started_at_ms: 1,
            finished_at_ms: Some(2),
        };
        let model = ActivityCardModel::new(activity);
        assert_eq!(model.activity.presentation.open_artifact_id, Some(artifact));
    }
}
