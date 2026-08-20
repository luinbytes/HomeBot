use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{Align, Color32, CornerRadius, Frame, Layout, RichText, Sense, Shape, Stroke, Ui, Vec2};

use crate::tokens::HomeBotTheme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarShape {
    Circle,
    RoundedSquare,
    Hexagon,
}

#[derive(Clone, Copy, Debug)]
pub struct BotIdentity<'a> {
    pub name: &'a str,
    pub role: &'a str,
    pub initials: &'a str,
    pub color: Color32,
    pub shape: AvatarShape,
    pub unread: bool,
}

pub fn avatar(ui: &mut Ui, theme: HomeBotTheme, bot: BotIdentity<'_>, small: bool) {
    let size = if small {
        theme.layout.avatar_small
    } else {
        theme.layout.avatar_size
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    match bot.shape {
        AvatarShape::Circle => painter.circle_filled(rect.center(), size / 2.0, bot.color),
        AvatarShape::RoundedSquare => {
            painter.rect_filled(rect, CornerRadius::same(theme.radii.sm), bot.color)
        }
        AvatarShape::Hexagon => {
            let radius = size / 2.0;
            let points = (0_i16..6)
                .map(|index| {
                    let angle = f32::from(index) * TAU / 6.0 - FRAC_PI_2;
                    rect.center() + Vec2::angled(angle) * radius
                })
                .collect();
            painter.add(Shape::convex_polygon(points, bot.color, Stroke::NONE))
        }
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        bot.initials,
        theme.typography.font(if small {
            theme.typography.micro
        } else {
            theme.typography.caption
        }),
        theme.palette.avatar_foreground,
    );
}

pub fn roster_row(ui: &mut Ui, theme: HomeBotTheme, bot: BotIdentity<'_>, selected: bool) {
    let fill = if selected {
        theme.palette.surface_selected
    } else {
        theme.palette.transparent
    };
    Frame::NONE
        .fill(fill)
        .corner_radius(CornerRadius::same(theme.radii.md))
        .inner_margin(egui::Margin::symmetric(theme.insets.md, theme.insets.sm))
        .show(ui, |ui| {
            ui.set_min_height(theme.layout.roster_row_height - theme.spacing.lg);
            ui.horizontal(|ui| {
                avatar(ui, theme, bot, false);
                ui.add_space(theme.spacing.xs);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(bot.name)
                            .font(theme.typography.font(theme.typography.body_compact))
                            .color(theme.palette.text_primary)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(bot.role)
                            .font(theme.typography.font(theme.typography.caption))
                            .color(theme.palette.text_tertiary),
                    );
                });
                if bot.unread {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (rect, _) = ui.allocate_exact_size(
                            Vec2::splat(theme.layout.unread_dot),
                            Sense::hover(),
                        );
                        ui.painter().circle_filled(
                            rect.center(),
                            theme.layout.unread_dot / 2.0,
                            theme.palette.accent,
                        );
                    });
                }
            });
        });
}

pub fn section_label(ui: &mut Ui, theme: HomeBotTheme, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .font(theme.typography.font(theme.typography.micro))
            .color(theme.palette.text_tertiary)
            .strong(),
    );
}

pub fn message(ui: &mut Ui, theme: HomeBotTheme, bot: Option<BotIdentity<'_>>, text: &str) {
    ui.horizontal_top(|ui| {
        if let Some(identity) = bot {
            avatar(ui, theme, identity, true);
        } else {
            ui.add_space(theme.layout.avatar_small);
        }
        ui.add_space(theme.spacing.xs);
        ui.vertical(|ui| {
            if let Some(identity) = bot {
                ui.label(
                    RichText::new(identity.name)
                        .font(theme.typography.font(theme.typography.body_compact))
                        .color(theme.palette.text_primary)
                        .strong(),
                );
            }
            ui.label(
                RichText::new(text)
                    .font(theme.typography.font(theme.typography.body))
                    .color(theme.palette.text_primary),
            );
        });
    });
}

pub fn activity_card(ui: &mut Ui, theme: HomeBotTheme, title: &str, detail: &str, risky: bool) {
    let indicator = if risky {
        theme.palette.warning
    } else {
        theme.palette.success
    };
    Frame::NONE
        .fill(theme.palette.surface)
        .stroke(Stroke::new(theme.layout.hairline, theme.palette.border))
        .corner_radius(CornerRadius::same(theme.radii.md))
        .inner_margin(egui::Margin::same(theme.insets.md))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::splat(theme.layout.activity_icon_size),
                    Sense::hover(),
                );
                ui.painter().circle_filled(
                    rect.center(),
                    theme.layout.activity_icon_size / 2.0,
                    indicator,
                );
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(title)
                            .font(theme.typography.font(theme.typography.body_compact))
                            .color(theme.palette.text_primary)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(detail)
                            .font(theme.typography.font(theme.typography.caption))
                            .color(theme.palette.text_secondary),
                    );
                });
            });
        });
}

pub fn approval_card(ui: &mut Ui, theme: HomeBotTheme) {
    Frame::NONE
        .fill(theme.palette.accent_soft)
        .stroke(Stroke::new(theme.layout.hairline, theme.palette.accent))
        .corner_radius(CornerRadius::same(theme.radii.md))
        .inner_margin(egui::Margin::same(theme.insets.lg))
        .show(ui, |ui| {
            ui.label(
                RichText::new("Approval needed")
                    .font(theme.typography.font(theme.typography.heading))
                    .color(theme.palette.text_primary)
                    .strong(),
            );
            ui.add_space(theme.spacing.xs);
            ui.label(
                RichText::new("Nova wants to run tests in the attached repository.")
                    .font(theme.typography.font(theme.typography.body_compact))
                    .color(theme.palette.text_secondary),
            );
            ui.add_space(theme.spacing.md);
            ui.horizontal(|ui| {
                let _ = ui.button("Allow once");
                let _ = ui.button("Deny");
            });
        });
}

pub fn composer(ui: &mut Ui, theme: HomeBotTheme, placeholder: &str) {
    Frame::NONE
        .fill(theme.palette.surface)
        .stroke(Stroke::new(theme.layout.hairline, theme.palette.border))
        .corner_radius(CornerRadius::same(theme.radii.lg))
        .shadow(theme.panel_shadow)
        .inner_margin(egui::Margin::same(theme.insets.lg))
        .show(ui, |ui| {
            ui.set_min_height(theme.layout.composer_min_height - theme.spacing.xxl);
            ui.label(
                RichText::new(placeholder)
                    .font(theme.typography.font(theme.typography.body))
                    .color(theme.palette.text_tertiary),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let _ = ui.button("Send");
                ui.label(
                    RichText::new("+")
                        .font(theme.typography.font(theme.typography.heading))
                        .color(theme.palette.text_secondary),
                );
            });
        });
}
