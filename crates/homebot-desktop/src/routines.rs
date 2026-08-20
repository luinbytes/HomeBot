//! Server-projected routine list, editor, and demonstration recording surfaces.

use crate::tokens::HomeBotTheme;
use egui::{Align, Frame, Layout, RichText, Stroke, Ui};

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
