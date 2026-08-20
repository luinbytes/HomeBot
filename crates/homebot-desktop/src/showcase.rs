use egui::{Align, CentralPanel, Frame, Layout, RichText, SidePanel, Stroke, TopBottomPanel};

use crate::{
    components::{
        AvatarShape, BotIdentity, activity_card, approval_card, composer, message, roster_row,
        section_label,
    },
    tokens::HomeBotTheme,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureState {
    Empty,
    DirectChat,
    Approval,
}

fn nova(theme: HomeBotTheme) -> BotIdentity<'static> {
    BotIdentity {
        name: "Nova",
        role: "Research and planning",
        initials: "N",
        color: theme.palette.bot_nova,
        shape: AvatarShape::RoundedSquare,
        unread: false,
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
    }
}

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
            roster_row(ui, theme, nova(theme), state != FixtureState::Empty);
            roster_row(ui, theme, patch(theme), false);
            roster_row(ui, theme, scout(theme), false);
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
                    RichText::new(if state == FixtureState::Empty {
                        "Bots"
                    } else {
                        "Nova"
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

    if state != FixtureState::Empty {
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
            ] {
                let context = egui::Context::default();
                let _ = context.run(egui::RawInput::default(), |context| {
                    render_fixture(context, theme, state);
                });
            }
        }
    }
}
