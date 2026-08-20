use eframe::egui;
use egui::{Align, CentralPanel, Layout, RichText, SidePanel, TopBottomPanel};
use homebot_protocol::{BotColor, BotProviderStatus, BotShape, BotSummary};

use crate::{
    bot_roster::{BotEditorDraft, BotRosterModel, ConnectionState, EditorError},
    components::{AvatarShape, BotIdentity, roster_row, section_label},
    tokens::HomeBotTheme,
};

pub struct HomeBotApp {
    pub roster: BotRosterModel,
    pub theme: HomeBotTheme,
    editor_error: Option<EditorError>,
}

impl Default for HomeBotApp {
    fn default() -> Self {
        Self {
            roster: BotRosterModel::default(),
            theme: HomeBotTheme::light(),
            editor_error: None,
        }
    }
}

impl eframe::App for HomeBotApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.render(context);
    }
}

impl HomeBotApp {
    pub fn render(&mut self, context: &egui::Context) {
        self.theme.install(context);
        self.sidebar(context);
        self.titlebar(context);
        CentralPanel::default().show(context, |ui| self.content(ui));
        self.editor(context);
    }

    fn sidebar(&mut self, context: &egui::Context) {
        SidePanel::left("bot_roster")
            .exact_width(self.theme.layout.sidebar_width)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("HomeBot")
                            .font(self.theme.typography.font(self.theme.typography.heading))
                            .strong(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("+").on_hover_text("Create Bot").clicked() {
                            self.roster.begin_create();
                        }
                    });
                });
                ui.add_space(self.theme.spacing.xl);
                section_label(ui, self.theme, "Bots");
                let visible: Vec<BotSummary> =
                    self.roster.visible_bots().into_iter().cloned().collect();
                for bot in visible {
                    let identity = identity(self.theme, &bot);
                    let response = roster_row(
                        ui,
                        self.theme,
                        identity,
                        self.roster.selected == Some(bot.id),
                    );
                    if response.clicked() {
                        self.roster.selected = Some(bot.id);
                        if bot.unread_count > 0 {
                            self.roster.queue_mark_read(bot.id);
                        }
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    let _ = ui.checkbox(&mut self.roster.show_archived, "Show archived Bots");
                });
            });
    }

    fn titlebar(&mut self, context: &egui::Context) {
        TopBottomPanel::top("bot_titlebar")
            .exact_height(self.theme.layout.titlebar_height)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    let selected = self
                        .roster
                        .selected
                        .and_then(|id| self.roster.bots.iter().find(|bot| bot.id == id));
                    ui.label(selected.map_or("Bots", |bot| bot.name.as_str()));
                    if let Some(bot) = selected.cloned() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Edit").clicked() {
                                self.roster.begin_edit(bot.id);
                            }
                            let label = if bot.archived { "Restore" } else { "Archive" };
                            if ui.button(label).clicked() {
                                self.roster.queue_archive(bot.id, bot.archived);
                            }
                        });
                    }
                });
            });
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(self.theme.layout.empty_state_top_padding);
            match self.roster.connection {
                ConnectionState::Disconnected => {
                    ui.heading("HomeBot is offline");
                    ui.label("Your Bots are safe on the server. Reconnect to make changes.");
                }
                ConnectionState::Connecting => {
                    ui.spinner();
                    ui.label("Connecting to HomeBot…");
                }
                ConnectionState::Connected if self.roster.visible_bots().is_empty() => {
                    ui.heading("Your AI team. On your computer.");
                    ui.label("Create a Bot, give it a job, and start a conversation.");
                    if ui.button("Create your first Bot").clicked() {
                        self.roster.begin_create();
                    }
                }
                ConnectionState::Connected => {
                    if let Some(bot) = self
                        .roster
                        .selected
                        .and_then(|id| self.roster.bots.iter().find(|bot| bot.id == id))
                    {
                        ui.heading(&bot.name);
                        ui.label(&bot.title);
                        if bot.provider == BotProviderStatus::Unavailable {
                            ui.colored_label(
                                self.theme.palette.warning,
                                "This Bot's provider is unavailable. Open Advanced settings to choose another.",
                            );
                        }
                    } else {
                        ui.heading("Choose a Bot");
                    }
                }
            }
        });
    }

    fn editor(&mut self, context: &egui::Context) {
        let Some(mut draft) = self.roster.editor.take() else {
            return;
        };
        let mut keep_open = true;
        egui::Window::new(if draft.bot_id.is_some() {
            "Edit Bot"
        } else {
            "Create a Bot"
        })
        .collapsible(false)
        .resizable(false)
        .show(context, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut draft.name);
            ui.label("Title");
            ui.text_edit_singleline(&mut draft.title);
            ui.label("Description");
            ui.text_edit_multiline(&mut draft.description);
            identity_picker(ui, &mut draft);
            ui.collapsing("Advanced settings", |ui| {
                ui.label("Provider profile and permissions are configured here.");
            });
            if let Some(error) = self.editor_error {
                ui.colored_label(self.theme.palette.danger, editor_message(error));
            }
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    keep_open = false;
                    self.editor_error = None;
                }
                if ui
                    .button(if draft.bot_id.is_some() {
                        "Save changes"
                    } else {
                        "Create Bot"
                    })
                    .clicked()
                {
                    self.roster.editor = Some(draft.clone());
                    match self.roster.submit_editor() {
                        Ok(()) => {
                            keep_open = false;
                            self.editor_error = None;
                        }
                        Err(error) => self.editor_error = Some(error),
                    }
                }
            });
        });
        if keep_open && self.roster.editor.is_none() {
            self.roster.editor = Some(draft);
        }
    }
}

fn identity_picker(ui: &mut egui::Ui, draft: &mut BotEditorDraft) {
    egui::ComboBox::from_label("Shape")
        .selected_text(format!("{:?}", draft.shape))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut draft.shape, BotShape::Circle, "Circle");
            ui.selectable_value(&mut draft.shape, BotShape::RoundedSquare, "Rounded square");
            ui.selectable_value(&mut draft.shape, BotShape::Hexagon, "Hexagon");
        });
    egui::ComboBox::from_label("Color")
        .selected_text(format!("{:?}", draft.color))
        .show_ui(ui, |ui| {
            for (color, label) in [
                (BotColor::Violet, "Violet"),
                (BotColor::Blue, "Blue"),
                (BotColor::Green, "Green"),
                (BotColor::Orange, "Orange"),
                (BotColor::Rose, "Rose"),
                (BotColor::Slate, "Slate"),
            ] {
                ui.selectable_value(&mut draft.color, color, label);
            }
        });
}

fn identity(theme: HomeBotTheme, bot: &BotSummary) -> BotIdentity<'_> {
    BotIdentity {
        name: &bot.name,
        role: &bot.title,
        initials: bot.name.get(0..1).unwrap_or("?"),
        color: match bot.color {
            BotColor::Violet | BotColor::Blue => theme.palette.bot_nova,
            BotColor::Green => theme.palette.bot_patch,
            BotColor::Orange | BotColor::Rose | BotColor::Slate => theme.palette.bot_scout,
        },
        shape: match bot.shape {
            BotShape::Circle => AvatarShape::Circle,
            BotShape::RoundedSquare => AvatarShape::RoundedSquare,
            BotShape::Hexagon => AvatarShape::Hexagon,
        },
        unread: bot.unread_count > 0,
    }
}

const fn editor_message(error: EditorError) -> &'static str {
    match error {
        EditorError::EmptyName => "Give this Bot a name.",
        EditorError::NameTooLong => "Bot names can be up to 48 characters.",
        EditorError::TitleTooLong => "Bot titles can be up to 80 characters.",
        EditorError::DuplicateName => "A Bot with that name already exists.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_app_renders_disconnected_and_editor_states() {
        let context = egui::Context::default();
        let mut app = HomeBotApp::default();
        app.roster.connection = ConnectionState::Disconnected;
        app.roster.begin_create();
        let _ = context.run(egui::RawInput::default(), |context| app.render(context));
        assert!(app.roster.editor.is_some());
    }
}
