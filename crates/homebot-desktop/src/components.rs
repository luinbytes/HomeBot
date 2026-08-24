use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{
    Align, Color32, CornerRadius, Frame, Layout, RichText, Sense, Shape, Stroke, Ui, Vec2,
    WidgetInfo, WidgetType,
};

use crate::tokens::HomeBotTheme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarShape {
    Circle,
    RoundedSquare,
    Hexagon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttentionIndicator {
    Working,
    NeedsApproval,
    Failed,
}

#[derive(Clone, Copy, Debug)]
pub struct BotIdentity<'a> {
    pub name: &'a str,
    pub role: &'a str,
    pub initials: &'a str,
    pub color: Color32,
    pub shape: AvatarShape,
    pub unread: bool,
    pub attention: Option<AttentionIndicator>,
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
    // HomeBot avatars are deliberately original, procedural characters. Their
    // face geometry is derived from the stable identity instead of relying on
    // generic initial badges or bundled artwork.
    let seed = bot.name.bytes().fold(0_u32, |value, byte| {
        value.wrapping_mul(31) + u32::from(byte)
    });
    let eye_y = rect.center().y - size * if small { 0.07 } else { 0.09 };
    let eye_gap = size * (0.13 + f32::from((seed % 3) as u8) * 0.015);
    let eye_radius = (size * 0.055).max(1.2);
    for x in [rect.center().x - eye_gap, rect.center().x + eye_gap] {
        painter.circle_filled(
            egui::pos2(x, eye_y),
            eye_radius,
            theme.palette.avatar_foreground,
        );
    }
    if !small {
        let mouth_y = rect.center().y + size * 0.14;
        let mouth_width = size * 0.18;
        painter.line_segment(
            [
                egui::pos2(rect.center().x - mouth_width, mouth_y),
                egui::pos2(rect.center().x + mouth_width, mouth_y),
            ],
            Stroke::new((size * 0.035).max(1.0), theme.palette.avatar_foreground),
        );
        let accent = egui::Rect::from_center_size(
            egui::pos2(rect.right() - size * 0.12, rect.top() + size * 0.14),
            Vec2::splat(size * 0.16),
        );
        painter.circle_filled(accent.center(), accent.width() / 2.0, theme.palette.canvas);
    }
}

pub fn bot_tile(
    ui: &mut Ui,
    theme: HomeBotTheme,
    bot: BotIdentity<'_>,
    selected: bool,
) -> egui::Response {
    let fill = if selected {
        theme.palette.surface_selected
    } else {
        theme.palette.transparent
    };
    Frame::NONE
        .fill(fill)
        .corner_radius(CornerRadius::same(theme.radii.md))
        .inner_margin(egui::Margin::same(theme.insets.sm))
        .show(ui, |ui| {
            ui.set_min_size(Vec2::new(
                crate::tokens::Layout::BOT_TILE_MIN_WIDTH,
                theme.layout.bot_tile_height,
            ));
            ui.vertical_centered(|ui| {
                avatar(ui, theme, bot, false);
                ui.add_space(theme.spacing.xs);
                ui.label(
                    RichText::new(bot.name)
                        .font(theme.typography.font(theme.typography.body_compact))
                        .color(theme.palette.text_primary)
                        .strong(),
                );
                ui.label(
                    RichText::new(bot.role)
                        .font(theme.typography.font(theme.typography.micro))
                        .color(theme.palette.text_tertiary),
                );
            });
        })
        .response
        .interact(Sense::click())
}

pub fn recent_conversation_row(
    ui: &mut Ui,
    theme: HomeBotTheme,
    title: &str,
    preview: &str,
    metadata: &str,
    selected: bool,
) -> egui::Response {
    Frame::NONE
        .fill(if selected {
            theme.palette.surface_selected
        } else {
            theme.palette.transparent
        })
        .corner_radius(CornerRadius::same(theme.radii.sm))
        .inner_margin(egui::Margin::symmetric(theme.insets.sm, theme.insets.sm))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(title)
                            .strong()
                            .color(theme.palette.text_primary),
                    );
                    ui.label(
                        RichText::new(preview)
                            .font(theme.typography.font(theme.typography.caption))
                            .color(theme.palette.text_tertiary),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                    ui.label(
                        RichText::new(metadata)
                            .font(theme.typography.font(theme.typography.micro))
                            .color(theme.palette.text_tertiary),
                    );
                });
            });
        })
        .response
        .interact(Sense::click())
}

pub fn navigation_row(
    ui: &mut Ui,
    theme: HomeBotTheme,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme.layout.sidebar_action_height),
        Sense::click(),
    );
    let hover = ui.ctx().animate_bool_with_time_and_easing(
        response.id,
        response.hovered(),
        f32::from(theme.motion.quick_ms) / 1_000.0,
        egui::emath::easing::cubic_out,
    );
    let fill = if selected {
        theme.palette.surface_selected
    } else {
        theme.palette.surface_hover.gamma_multiply(hover)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(theme.radii.sm), fill);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(theme.layout.hairline),
            CornerRadius::same(theme.radii.sm),
            Stroke::new(theme.layout.hairline, theme.palette.accent),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + theme.spacing.md, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        theme.typography.font(theme.typography.body_compact),
        theme.palette.text_primary,
    );
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    response
}

pub fn send_button(ui: &mut Ui, theme: HomeBotTheme, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .fill(theme.palette.text_primary)
            .corner_radius(CornerRadius::same(theme.radii.pill))
            .min_size(Vec2::splat(30.0)),
    );
    let color = if enabled {
        theme.palette.canvas
    } else {
        theme.palette.text_tertiary
    };
    let center = response.rect.center();
    let stroke = Stroke::new(1.5_f32, color);
    ui.painter().line_segment(
        [center + egui::vec2(0.0, 5.0), center - egui::vec2(0.0, 5.0)],
        stroke,
    );
    ui.painter().line_segment(
        [
            center - egui::vec2(0.0, 5.0),
            center + egui::vec2(-4.0, -1.0),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            center - egui::vec2(0.0, 5.0),
            center + egui::vec2(4.0, -1.0),
        ],
        stroke,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, "Send message"));
    response
}

pub fn roster_row(
    ui: &mut Ui,
    theme: HomeBotTheme,
    bot: BotIdentity<'_>,
    metadata: &str,
    selected: bool,
) -> egui::Response {
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
            ui.set_min_width(ui.available_width());
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
                if !metadata.is_empty() || bot.unread || bot.attention.is_some() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !metadata.is_empty() {
                            ui.label(
                                RichText::new(metadata)
                                    .font(theme.typography.font(theme.typography.micro))
                                    .color(theme.palette.text_tertiary),
                            );
                        }
                        if bot.unread || bot.attention.is_some() {
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::splat(theme.layout.unread_dot),
                                Sense::hover(),
                            );
                            let color = match bot.attention {
                                Some(AttentionIndicator::Working) => theme.palette.success,
                                Some(AttentionIndicator::NeedsApproval) => theme.palette.warning,
                                Some(AttentionIndicator::Failed) => theme.palette.danger,
                                None => theme.palette.accent,
                            };
                            ui.painter().circle_filled(
                                rect.center(),
                                theme.layout.unread_dot / 2.0,
                                color,
                            );
                        }
                    });
                }
            });
        })
        .response
        .interact(Sense::click())
}

pub fn section_label(ui: &mut Ui, theme: HomeBotTheme, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .font(theme.typography.font(theme.typography.micro))
            .color(theme.palette.text_tertiary)
            .strong(),
    );
}

pub fn message(
    ui: &mut Ui,
    theme: HomeBotTheme,
    bot: Option<BotIdentity<'_>>,
    text: &str,
) -> egui::Response {
    let assistant = bot.is_some();
    ui.with_layout(
        if assistant {
            Layout::left_to_right(Align::TOP)
        } else {
            Layout::right_to_left(Align::TOP)
        },
        |ui| {
            Frame::NONE
                .fill(if assistant {
                    theme.message_assistant()
                } else {
                    theme.message_user()
                })
                .corner_radius(CornerRadius::same(theme.radii.lg))
                .inner_margin(egui::Margin::symmetric(theme.insets.md, theme.insets.sm))
                .show(ui, |ui| {
                    ui.set_max_width(if assistant {
                        theme.layout.assistant_message_max_width
                    } else {
                        theme.layout.user_message_max_width
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(text)
                                .font(theme.typography.font(theme.typography.body))
                                .color(if assistant {
                                    theme.palette.text_primary
                                } else {
                                    theme.palette.avatar_foreground
                                }),
                        )
                        .wrap(),
                    );
                });
        },
    )
    .response
}

pub fn activity_card(
    ui: &mut Ui,
    theme: HomeBotTheme,
    title: &str,
    detail: &str,
    risky: bool,
) -> egui::Response {
    let indicator = if risky {
        theme.palette.warning
    } else {
        theme.palette.success
    };
    Frame::NONE
        .fill(theme.palette.surface)
        .stroke(Stroke::new(theme.layout.hairline, theme.palette.border))
        .corner_radius(CornerRadius::same(theme.radii.sm))
        .inner_margin(egui::Margin::symmetric(theme.insets.sm, theme.insets.sm))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let icon_size = theme.layout.activity_icon_size * 0.45;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(icon_size), Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), icon_size / 2.0, indicator);
                ui.label(
                    RichText::new(title)
                        .font(theme.typography.font(theme.typography.caption))
                        .color(theme.palette.text_primary)
                        .strong(),
                );
                if !detail.is_empty() && detail != title {
                    ui.add(
                        egui::Label::new(
                            RichText::new(detail)
                                .font(theme.typography.font(theme.typography.caption))
                                .color(theme.palette.text_secondary),
                        )
                        .truncate(),
                    );
                }
            });
        })
        .response
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
