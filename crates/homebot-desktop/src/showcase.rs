use egui::{Align, CentralPanel, Frame, Layout, RichText, SidePanel, Stroke, TopBottomPanel};
use homebot_protocol::{
    ActivityDetail, ActivityKind, ActivityPresentation, ActivityStatus, ActivitySummary, RiskLevel,
};
use uuid::Uuid;

use crate::{
    activity_surfaces::{ActivityCardModel, activity_surface},
    components::{
        AttentionIndicator, AvatarShape, BotIdentity, activity_card, approval_card, composer,
        message, roster_row, section_label,
    },
    routines::{RoutineSurface, routine_surface},
    settings::{
        DesktopSettings, PluginSettingsItem, PluginViewState, SettingsSection, ThemePreference,
        settings_view,
    },
    tokens::HomeBotTheme,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureState {
    Empty,
    DirectChat,
    Approval,
    QueueError,
    GroupChat,
    BotEditor,
    Disconnected,
    ProviderUnavailable,
    ActivitySurfaces,
    Settings,
    SettingsAppearance,
    SettingsPlugins,
    RoutinesList,
    RoutineEditor,
    RoutineRecording,
}

fn nova(theme: HomeBotTheme) -> BotIdentity<'static> {
    BotIdentity {
        name: "Nova",
        role: "Research and planning",
        initials: "N",
        color: theme.palette.bot_nova,
        shape: AvatarShape::RoundedSquare,
        unread: false,
        attention: None,
    }
}

fn patch(theme: HomeBotTheme) -> BotIdentity<'static> {
    BotIdentity {
        name: "Patch",
        role: "Code and repositories",
        initials: "P",
        color: theme.palette.bot_patch,
        shape: AvatarShape::Hexagon,
        unread: true,
        attention: Some(AttentionIndicator::NeedsApproval),
    }
}

fn scout(theme: HomeBotTheme) -> BotIdentity<'static> {
    BotIdentity {
        name: "Scout",
        role: "Web and monitoring",
        initials: "S",
        color: theme.palette.bot_scout,
        shape: AvatarShape::Circle,
        unread: false,
        attention: None,
    }
}

#[allow(clippy::too_many_lines)]
pub fn render_fixture(context: &egui::Context, theme: HomeBotTheme, state: FixtureState) {
    theme.install(context);
    SidePanel::left("homebot_sidebar")
        .exact_width(theme.layout.sidebar_width)
        .frame(
            Frame::NONE
                .fill(theme.palette.sidebar)
                .inner_margin(egui::Margin::same(theme.insets.lg)),
        )
        .show(context, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("HomeBot")
                        .font(theme.typography.font(theme.typography.heading))
                        .color(theme.palette.text_primary)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let _ = ui.button("+");
                });
            });
            ui.add_space(theme.spacing.xl);
            section_label(ui, theme, "Bots");
            ui.add_space(theme.spacing.sm);
            let _ = roster_row(ui, theme, nova(theme), "", state != FixtureState::Empty);
            let _ = roster_row(ui, theme, patch(theme), "Working", false);
            let _ = roster_row(ui, theme, scout(theme), "", false);
            ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                ui.separator();
                ui.label(
                    RichText::new("Settings")
                        .font(theme.typography.font(theme.typography.body_compact))
                        .color(theme.palette.text_secondary),
                );
            });
        });

    TopBottomPanel::top("homebot_titlebar")
        .exact_height(theme.layout.titlebar_height)
        .frame(
            Frame::NONE
                .fill(theme.palette.canvas)
                .stroke(Stroke::new(theme.layout.hairline, theme.palette.border))
                .inner_margin(egui::Margin::symmetric(theme.insets.xl, theme.insets.md)),
        )
        .show(context, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(match state {
                        FixtureState::Empty => "Bots",
                        FixtureState::BotEditor => "New Bot",
                        FixtureState::ActivitySurfaces => "Nova · activity",
                        FixtureState::Settings
                        | FixtureState::SettingsAppearance
                        | FixtureState::SettingsPlugins => "Settings",
                        FixtureState::RoutinesList => "Routines",
                        FixtureState::RoutineEditor => "Edit routine",
                        FixtureState::RoutineRecording => "Teach a routine",
                        _ => "Nova",
                    })
                    .font(theme.typography.font(theme.typography.body_compact))
                    .color(theme.palette.text_primary)
                    .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let _ = ui.button("•••");
                });
            });
        });

    if !matches!(
        state,
        FixtureState::Empty
            | FixtureState::BotEditor
            | FixtureState::ActivitySurfaces
            | FixtureState::Settings
            | FixtureState::SettingsAppearance
            | FixtureState::SettingsPlugins
            | FixtureState::RoutinesList
            | FixtureState::RoutineEditor
            | FixtureState::RoutineRecording
    ) {
        TopBottomPanel::bottom("homebot_composer")
            .exact_height(theme.layout.composer_min_height + theme.spacing.xl)
            .frame(
                Frame::NONE
                    .fill(theme.palette.canvas)
                    .inner_margin(egui::Margin::symmetric(theme.insets.xl, theme.insets.md)),
            )
            .show(context, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(theme.layout.composer_max_width);
                    composer(ui, theme, "Message Nova");
                });
            });
    }

    CentralPanel::default()
        .frame(Frame::NONE.fill(theme.palette.canvas))
        .show(context, |ui| match state {
            FixtureState::Empty => empty_state(ui, theme),
            FixtureState::DirectChat => chat_state(ui, theme, false),
            FixtureState::Approval => chat_state(ui, theme, true),
            FixtureState::QueueError => queue_error_state(ui, theme),
            FixtureState::GroupChat => group_chat_state(ui, theme),
            FixtureState::BotEditor => bot_editor(ui, theme),
            FixtureState::Disconnected => disconnected_state(ui, theme),
            FixtureState::ProviderUnavailable => provider_unavailable(ui, theme),
            FixtureState::ActivitySurfaces => activity_surfaces_state(ui, theme),
            FixtureState::Settings => settings_state(ui, theme, false),
            FixtureState::SettingsAppearance => settings_state(ui, theme, true),
            FixtureState::SettingsPlugins => plugin_settings_state(ui, theme),
            FixtureState::RoutinesList => routine_surface(ui, theme, RoutineSurface::List),
            FixtureState::RoutineEditor => routine_surface(ui, theme, RoutineSurface::Editor),
            FixtureState::RoutineRecording => routine_surface(ui, theme, RoutineSurface::Recording),
        });
}

fn plugin_settings_state(ui: &mut egui::Ui, theme: HomeBotTheme) {
    ui.add_space(theme.spacing.xl);
    ui.horizontal_centered(|ui| {
        let mut settings = DesktopSettings {
            section: SettingsSection::Plugins,
            plugins: vec![
                PluginSettingsItem {
                    id: None,
                    name: "Repository tools".to_owned(),
                    detail: String::new(),
                    state: PluginViewState::Connected,
                    enabled: true,
                },
                PluginSettingsItem {
                    id: None,
                    name: "Local notes".to_owned(),
                    detail: "Connection error".to_owned(),
                    state: PluginViewState::Error,
                    enabled: false,
                },
            ],
            ..DesktopSettings::default()
        };
        let _ = settings_view(ui, theme, &mut settings, |_| {});
    });
}

fn settings_state(ui: &mut egui::Ui, theme: HomeBotTheme, appearance: bool) {
    ui.add_space(theme.spacing.xl);
    ui.horizontal_centered(|ui| {
        let mut settings = DesktopSettings {
            paired_devices: 2,
            ..DesktopSettings::default()
        };
        if appearance {
            settings.section = SettingsSection::Appearance;
            settings.theme = ThemePreference::Dark;
        }
        let _ = settings_view(ui, theme, &mut settings, |_| {});
    });
}

fn fixture_activity(
    kind: ActivityKind,
    title: &str,
    detail: &str,
    risk: RiskLevel,
    presentation: ActivityDetail,
    open_artifact_id: Option<Uuid>,
) -> ActivityCardModel {
    ActivityCardModel {
        activity: ActivitySummary {
            id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            message_id: None,
            title: title.to_owned(),
            detail: detail.to_owned(),
            kind,
            presentation: ActivityPresentation {
                risk,
                detail: presentation,
                copy_text: Some(detail.to_owned()),
                open_artifact_id,
            },
            status: ActivityStatus::Succeeded,
            requires_attention: risk != RiskLevel::Low,
            started_at_ms: 1,
            finished_at_ms: Some(2),
        },
        expanded: true,
    }
}

fn activity_surfaces_state(ui: &mut egui::Ui, theme: HomeBotTheme) {
    ui.vertical_centered(|ui| {
        ui.set_max_width(theme.layout.content_max_width);
        ui.add_space(theme.spacing.lg);
        let artifact_id = Uuid::from_u128(4);
        let mut cards = [
            fixture_activity(
                ActivityKind::Filesystem,
                "Updated release notes",
                "docs/releasing.md · 184 bytes",
                RiskLevel::Low,
                ActivityDetail::File {
                    action: "write".to_owned(),
                    workspace_path: "docs/releasing.md".to_owned(),
                    bytes_changed: Some(184),
                    sha256: Some("b87d…2a10".to_owned()),
                },
                None,
            ),
            fixture_activity(
                ActivityKind::Terminal,
                "Ran release checks",
                "cargo test --workspace · exit 0",
                RiskLevel::Elevated,
                ActivityDetail::Terminal {
                    command: "cargo test --workspace".to_owned(),
                    working_directory: "HomeBot".to_owned(),
                    output_preview: "87 tests passed".to_owned(),
                    exit_code: Some(0),
                    truncated: false,
                },
                None,
            ),
            fixture_activity(
                ActivityKind::Browser,
                "Checked provider documentation",
                "docs.rs · screenshot saved",
                RiskLevel::Low,
                ActivityDetail::Browser {
                    action: "navigate".to_owned(),
                    url: "https://docs.rs/axum".to_owned(),
                    page_title: Some("axum documentation".to_owned()),
                    screenshot_artifact_id: Some(artifact_id),
                },
                Some(artifact_id),
            ),
            fixture_activity(
                ActivityKind::Artifact,
                "Generated audit report",
                "release-audit.md · 18 KB",
                RiskLevel::Low,
                ActivityDetail::Artifact {
                    artifact_id,
                    name: "release-audit.md".to_owned(),
                    media_type: "text/markdown".to_owned(),
                    size_bytes: 18_432,
                },
                Some(artifact_id),
            ),
        ];
        cards[2].expanded = false;
        cards[3].expanded = false;
        for card in &mut cards {
            let _ = activity_surface(ui, theme, card);
            ui.add_space(theme.spacing.sm);
        }
    });
}

fn empty_state(ui: &mut egui::Ui, theme: HomeBotTheme) {
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        ui.add_space(theme.layout.empty_state_top_padding);
        ui.label(
            RichText::new("Your AI team. On your computer.")
                .font(theme.typography.font(theme.typography.display))
                .color(theme.palette.text_primary)
                .strong(),
        );
        ui.add_space(theme.spacing.sm);
        ui.label(
            RichText::new("Create a Bot, give it a job, and start a conversation.")
                .font(theme.typography.font(theme.typography.body))
                .color(theme.palette.text_secondary),
        );
        ui.add_space(theme.spacing.xl);
        let _ = ui.button("Create your first Bot");
    });
}

fn chat_state(ui: &mut egui::Ui, theme: HomeBotTheme, approval: bool) {
    ui.vertical_centered(|ui| {
        ui.set_max_width(theme.layout.content_max_width);
        ui.add_space(theme.spacing.xl);
        message(
            ui,
            theme,
            None,
            "Review the repository and tell me what needs attention before release.",
        );
        ui.add_space(theme.spacing.xl);
        message(
            ui,
            theme,
            Some(nova(theme)),
            "I’ll inspect the project structure and run the existing checks first.",
        );
        ui.add_space(theme.spacing.md);
        activity_card(
            ui,
            theme,
            "Checked repository status",
            "3 files changed · main",
            false,
        );
        ui.add_space(theme.spacing.md);
        if approval {
            approval_card(ui, theme);
        } else {
            activity_card(ui, theme, "Running test suite", "42 checks passed", false);
        }
    });
}

fn queue_error_state(ui: &mut egui::Ui, theme: HomeBotTheme) {
    ui.vertical_centered(|ui| {
        ui.set_max_width(theme.layout.content_max_width);
        ui.add_space(theme.spacing.xl);
        message(
            ui,
            theme,
            None,
            "Run the release checks and summarize any failures.",
        );
        ui.add_space(theme.spacing.xl);
        message(
            ui,
            theme,
            Some(nova(theme)),
            "I couldn't finish because the provider connection closed.",
        );
        ui.add_space(theme.spacing.sm);
        Frame::NONE
            .fill(theme.palette.surface)
            .stroke(Stroke::new(theme.layout.hairline, theme.palette.danger))
            .corner_radius(egui::CornerRadius::same(theme.radii.sm))
            .inner_margin(egui::Margin::same(theme.insets.md))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Response failed")
                            .color(theme.palette.danger)
                            .strong(),
                    );
                    ui.label(
                        RichText::new("The provider can be retried safely.")
                            .color(theme.palette.text_secondary),
                    );
                    let _ = ui.button("Retry");
                });
            });
        ui.add_space(theme.spacing.lg);
        Frame::NONE
            .fill(theme.palette.surface_hover)
            .corner_radius(egui::CornerRadius::same(theme.radii.sm))
            .inner_margin(egui::Margin::same(theme.insets.md))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Queued next")
                        .color(theme.palette.text_secondary)
                        .strong(),
                );
                ui.label("Open the failing test output and propose a fix.");
            });
    });
}

fn group_chat_state(ui: &mut egui::Ui, theme: HomeBotTheme) {
    ui.vertical_centered(|ui| {
        ui.set_max_width(theme.layout.content_max_width);
        ui.add_space(theme.spacing.lg);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Release team").strong());
            ui.label(RichText::new("Nova owns this task").color(theme.palette.text_secondary));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let _ = ui.button("Stop all");
            });
        });
        ui.horizontal(|ui| {
            for (name, status, color) in [
                ("Nova", "working", theme.palette.bot_nova),
                ("Patch", "working", theme.palette.bot_patch),
                ("Scout", "waiting", theme.palette.bot_scout),
            ] {
                Frame::NONE
                    .fill(theme.palette.surface)
                    .corner_radius(egui::CornerRadius::same(theme.radii.sm))
                    .inner_margin(egui::Margin::same(theme.insets.sm))
                    .show(ui, |ui| {
                        ui.colored_label(color, format!("● {name}"));
                        ui.label(RichText::new(status).color(theme.palette.text_secondary));
                    });
            }
        });
        ui.add_space(theme.spacing.xl);
        message(
            ui,
            theme,
            None,
            "@Nova @Patch review the release blockers together.",
        );
        ui.add_space(theme.spacing.lg);
        message(
            ui,
            theme,
            Some(nova(theme)),
            "@Patch I found a migration risk. Please validate it against the test fixture.",
        );
        ui.add_space(theme.spacing.md);
        activity_card(
            ui,
            theme,
            "Parallel work",
            "Nova · Patch · 2 of 3 active",
            false,
        );
        ui.add_space(theme.spacing.md);
        Frame::NONE
            .fill(theme.palette.surface_hover)
            .corner_radius(egui::CornerRadius::same(theme.radii.sm))
            .inner_margin(egui::Margin::same(theme.insets.md))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Ownership handoff").strong());
                    ui.label("Nova to Scout");
                    ui.label(
                        RichText::new("Final verification").color(theme.palette.text_secondary),
                    );
                });
            });
    });
}

fn bot_editor(ui: &mut egui::Ui, theme: HomeBotTheme) {
    ui.vertical_centered(|ui| {
        ui.set_max_width(520.0);
        ui.add_space(theme.spacing.xxl);
        ui.label(
            RichText::new("Create a Bot")
                .font(theme.typography.font(theme.typography.title))
                .color(theme.palette.text_primary)
                .strong(),
        );
        ui.label(
            RichText::new("Give your teammate a name and a clear role.")
                .font(theme.typography.font(theme.typography.body))
                .color(theme.palette.text_secondary),
        );
        ui.add_space(theme.spacing.xl);
        for (label, value) in [
            ("Name", "Nova"),
            ("Title", "Research and planning"),
            (
                "Description",
                "Finds context and turns it into useful plans.",
            ),
        ] {
            ui.label(
                RichText::new(label)
                    .font(theme.typography.font(theme.typography.caption))
                    .color(theme.palette.text_secondary)
                    .strong(),
            );
            Frame::NONE
                .fill(theme.palette.surface)
                .stroke(Stroke::new(theme.layout.hairline, theme.palette.border))
                .corner_radius(egui::CornerRadius::same(theme.radii.sm))
                .inner_margin(egui::Margin::same(theme.insets.md))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(value)
                            .font(theme.typography.font(theme.typography.body))
                            .color(theme.palette.text_primary),
                    );
                });
            ui.add_space(theme.spacing.md);
        }
        ui.horizontal(|ui| {
            ui.label("Shape  ◉   ◼   ⬢");
            ui.label("Color  ●  ●  ●  ●");
        });
        ui.add_space(theme.spacing.lg);
        ui.collapsing("Advanced settings", |ui| {
            ui.label("Provider profile");
            ui.label("Permissions · Ask before changes");
        });
        ui.add_space(theme.spacing.xl);
        ui.horizontal(|ui| {
            let _ = ui.button("Cancel");
            let _ = ui.button("Create Bot");
        });
    });
}

fn disconnected_state(ui: &mut egui::Ui, theme: HomeBotTheme) {
    ui.vertical_centered(|ui| {
        ui.add_space(theme.layout.empty_state_top_padding);
        ui.label(
            RichText::new("HomeBot is reconnecting")
                .font(theme.typography.font(theme.typography.title))
                .color(theme.palette.text_primary)
                .strong(),
        );
        ui.label(
            RichText::new("Your Bots and chats are safe on the server.")
                .font(theme.typography.font(theme.typography.body))
                .color(theme.palette.text_secondary),
        );
        ui.add_space(theme.spacing.lg);
        let _ = ui.button("Try again");
    });
}

fn provider_unavailable(ui: &mut egui::Ui, theme: HomeBotTheme) {
    ui.vertical_centered(|ui| {
        ui.set_max_width(theme.layout.content_max_width);
        ui.add_space(theme.spacing.xl);
        Frame::NONE
            .fill(theme.palette.accent_soft)
            .stroke(Stroke::new(theme.layout.hairline, theme.palette.warning))
            .corner_radius(egui::CornerRadius::same(theme.radii.md))
            .inner_margin(egui::Margin::same(theme.insets.lg))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Nova’s provider is unavailable")
                        .font(theme.typography.font(theme.typography.heading))
                        .color(theme.palette.text_primary)
                        .strong(),
                );
                ui.label(
                    RichText::new("Choose another provider in Advanced settings or reconnect the current one.")
                        .font(theme.typography.font(theme.typography.body_compact))
                        .color(theme.palette.text_secondary),
                );
                let _ = ui.button("Open Bot settings");
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_major_fixture_renders_in_both_themes() {
        for theme in [HomeBotTheme::light(), HomeBotTheme::dark()] {
            for state in [
                FixtureState::Empty,
                FixtureState::DirectChat,
                FixtureState::Approval,
                FixtureState::QueueError,
                FixtureState::GroupChat,
                FixtureState::BotEditor,
                FixtureState::Disconnected,
                FixtureState::ProviderUnavailable,
                FixtureState::ActivitySurfaces,
                FixtureState::Settings,
                FixtureState::SettingsAppearance,
            ] {
                let context = egui::Context::default();
                let _ = context.run(egui::RawInput::default(), |context| {
                    render_fixture(context, theme, state);
                });
            }
        }
    }
}
