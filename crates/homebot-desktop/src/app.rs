use eframe::egui;
use egui::{Align, CentralPanel, Layout, RichText, SidePanel, TopBottomPanel};
use homebot_protocol::{
    BotAttention, BotColor, BotProviderStatus, BotShape, BotSummary, ChatSummary, ServerEvent,
    ServerEventBody,
};
use std::sync::mpsc::{Receiver, channel};

use crate::{
    bot_roster::{BotEditorDraft, BotRosterModel, ConnectionState, EditorError},
    components::{
        AttentionIndicator, AvatarShape, BotIdentity, activity_card, message, roster_row,
        section_label,
    },
    notifications::{DeepLink, NotificationCenter, NotificationSink, SystemNotificationSink},
    settings::{DesktopSettings, settings_view},
    skills::SkillProjection,
    timeline::{ComposerError, TimelineModel},
    tokens::HomeBotTheme,
    transport::{DesktopCommand, DesktopEvent, DesktopTransport, RuntimeConfig},
    workspaces::{WorkspaceCommand, WorkspaceProjection},
};

const SETTINGS_STORAGE_KEY: &str = "homebot.desktop.settings.v1";

pub struct HomeBotApp {
    pub roster: BotRosterModel,
    pub theme: HomeBotTheme,
    pub timeline: TimelineModel,
    pub settings: DesktopSettings,
    pub skills: SkillProjection,
    pub workspaces: WorkspaceProjection,
    pub checkpoint_diff: Option<homebot_protocol::CheckpointDiffResponse>,
    pub active_deep_link: Option<DeepLink>,
    notification_center: NotificationCenter,
    notification_sink: SystemNotificationSink,
    deep_link_receiver: Receiver<DeepLink>,
    settings_open: bool,
    editor_error: Option<EditorError>,
    composer_error: Option<ComposerError>,
    transport_error: Option<String>,
    chats: Vec<ChatSummary>,
    transport: Option<DesktopTransport>,
}

impl Default for HomeBotApp {
    fn default() -> Self {
        let (deep_link_sender, deep_link_receiver) = channel();
        Self {
            roster: BotRosterModel::default(),
            theme: HomeBotTheme::light(),
            timeline: TimelineModel::default(),
            settings: DesktopSettings::default(),
            skills: SkillProjection::default(),
            workspaces: WorkspaceProjection::default(),
            checkpoint_diff: None,
            active_deep_link: None,
            notification_center: NotificationCenter::default(),
            notification_sink: SystemNotificationSink::new(deep_link_sender),
            deep_link_receiver,
            settings_open: false,
            editor_error: None,
            composer_error: None,
            transport_error: None,
            chats: Vec::new(),
            transport: None,
        }
    }
}

impl eframe::App for HomeBotApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.render(context);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(encoded) = serde_json::to_string(&self.settings) {
            storage.set_string(SETTINGS_STORAGE_KEY, encoded);
        }
    }
}

impl HomeBotApp {
    #[must_use]
    pub fn from_creation_context(context: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::default();
        if let Some(settings) = context
            .storage
            .and_then(|storage| storage.get_string(SETTINGS_STORAGE_KEY))
            .and_then(|encoded| serde_json::from_str(&encoded).ok())
        {
            app.settings = settings;
        }
        app.transport = Some(DesktopTransport::start(RuntimeConfig::desktop_default(
            app.settings.server_endpoint.clone(),
        )));
        app
    }

    pub fn render(&mut self, context: &egui::Context) {
        self.pump_transport(context);
        let activated: Vec<_> = self.deep_link_receiver.try_iter().collect();
        for deep_link in activated {
            self.open_deep_link(deep_link);
        }
        let system_dark = context.system_theme() == Some(egui::Theme::Dark);
        self.theme = match self.settings.resolved_theme(system_dark) {
            crate::tokens::ThemeMode::Light => HomeBotTheme::light(),
            crate::tokens::ThemeMode::Dark => HomeBotTheme::dark(),
        };
        self.theme.install(context);
        self.sidebar(context);
        self.titlebar(context);
        CentralPanel::default().show(context, |ui| {
            if self.settings_open {
                settings_view(ui, self.theme, &mut self.settings);
            } else {
                self.content(ui);
            }
        });
        self.editor(context);
        self.flush_transport();
        context.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn pump_transport(&mut self, context: &egui::Context) {
        let events = self
            .transport
            .as_ref()
            .map_or_else(Vec::new, |transport| transport.try_events().collect());
        for event in events {
            match event {
                DesktopEvent::Connecting => self.roster.connection = ConnectionState::Connecting,
                DesktopEvent::Connected => {
                    self.roster.connection = ConnectionState::Connected;
                    self.transport_error = None;
                }
                DesktopEvent::Disconnected(error) => {
                    self.roster.connection = ConnectionState::Disconnected;
                    self.transport_error = Some(error.to_string());
                }
                DesktopEvent::Snapshot { snapshot, .. } => {
                    self.roster.apply_snapshot(snapshot.bots);
                    self.chats = snapshot.chats;
                    self.skills.hydrate(snapshot.skills);
                    self.workspaces
                        .hydrate(snapshot.repository_workspaces, snapshot.chat_workspaces);
                    self.load_selected_timeline();
                }
                DesktopEvent::Server(event) => {
                    self.skills.apply(&event);
                    self.workspaces.apply(&event);
                    let bot_id = match &event.body {
                        ServerEventBody::BotChanged { bot } => {
                            self.roster.apply_change(bot.clone());
                            Some(bot.id)
                        }
                        ServerEventBody::ChatChanged { chat } => {
                            upsert_chat(&mut self.chats, chat.clone());
                            if self.roster.selected == Some(chat.bot_id) {
                                self.send_transport(DesktopCommand::LoadTimeline(chat.id));
                            }
                            Some(chat.bot_id)
                        }
                        _ => self.roster.selected,
                    };
                    let _ = self.timeline.apply_event(event.clone());
                    let _ = self.apply_notification_event(context, &event, bot_id);
                    if self.timeline.needs_snapshot {
                        self.load_selected_timeline();
                    }
                }
                DesktopEvent::Timeline(timeline) => self.timeline.hydrate(timeline),
                DesktopEvent::BotMutation(response) => self.roster.apply_change(response.bot),
                DesktopEvent::AttachmentUploaded(attachment_id) => {
                    self.timeline.composer.attachment_ids.push(attachment_id);
                }
                DesktopEvent::RepositoryWorkspaceRegistered(workspace) => {
                    self.workspaces.apply_repository(workspace);
                }
                DesktopEvent::ChatWorkspaceAttached(workspace) => {
                    self.workspaces.apply_chat(workspace);
                }
                DesktopEvent::ChatWorkspaceDetached(chat_id) => {
                    self.workspaces.remove_chat(chat_id);
                }
                DesktopEvent::WorkspaceBranches { .. } => {}
                DesktopEvent::CheckpointDiff(diff) => self.checkpoint_diff = Some(diff),
                DesktopEvent::MutationFailed(error) => {
                    self.transport_error = Some(error.to_string());
                }
            }
        }
    }

    fn flush_transport(&mut self) {
        for command in self.roster.take_commands() {
            self.send_transport(DesktopCommand::Bot(command));
        }
        let Some(bot_id) = self.roster.selected else {
            let _ = self.timeline.take_commands();
            return;
        };
        let chat_id = self
            .timeline
            .chat
            .as_ref()
            .filter(|chat| chat.bot_id == bot_id)
            .map(|chat| chat.id);
        for command in self.timeline.take_commands() {
            self.send_transport(DesktopCommand::Timeline {
                bot_id,
                chat_id,
                command,
            });
        }
    }

    fn load_selected_timeline(&mut self) {
        let Some(bot_id) = self.roster.selected else {
            self.timeline = TimelineModel::default();
            return;
        };
        if let Some(chat) = self.chats.iter().find(|chat| chat.bot_id == bot_id) {
            self.send_transport(DesktopCommand::LoadTimeline(chat.id));
        } else {
            self.timeline = TimelineModel::default();
        }
    }

    fn send_transport(&mut self, command: DesktopCommand) {
        let result = self.transport.as_ref().map_or_else(
            || Err(crate::transport::TransportFailure::ServerUnavailable),
            |transport| transport.send(command),
        );
        if let Err(error) = result {
            self.transport_error = Some(error.to_string());
            self.roster.connection = ConnectionState::Disconnected;
        }
    }

    /// Emits a native notification for a newly observed server event when policy permits.
    ///
    /// # Errors
    ///
    /// Returns a safe platform error when the host notification service is unavailable.
    pub fn apply_notification_event(
        &mut self,
        context: &egui::Context,
        event: &ServerEvent,
        bot_id: Option<uuid::Uuid>,
    ) -> Result<(), String> {
        let focused = context.input(|input| input.viewport().focused.unwrap_or(false));
        let Some(intent) = self
            .notification_center
            .observe(event, bot_id, focused, &self.settings)
        else {
            return Ok(());
        };
        if !focused {
            context.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                match intent.kind {
                    crate::notifications::NotificationKind::Finished => {
                        egui::UserAttentionType::Informational
                    }
                    crate::notifications::NotificationKind::NeedsApproval
                    | crate::notifications::NotificationKind::Error => {
                        egui::UserAttentionType::Critical
                    }
                },
            ));
        }
        self.notification_sink.show(intent)
    }

    pub fn open_deep_link(&mut self, deep_link: DeepLink) {
        if let Some(bot_id) = deep_link.bot_id {
            self.roster.selected = Some(bot_id);
        }
        self.settings_open = false;
        self.active_deep_link = Some(deep_link);
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
                        self.load_selected_timeline();
                        if bot.unread_count > 0 {
                            self.roster.queue_mark_read(bot.id);
                        }
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    let _ = ui.checkbox(&mut self.roster.show_archived, "Show archived Bots");
                    if ui.button("Settings").clicked() {
                        self.settings_open = true;
                    }
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
                    ui.label(if self.settings_open {
                        "Settings"
                    } else {
                        selected.map_or("Bots", |bot| bot.name.as_str())
                    });
                    if self.settings_open {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Done").clicked() {
                                self.settings_open = false;
                            }
                        });
                        return;
                    }
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
                    if let Some(error) = &self.transport_error {
                        ui.colored_label(self.theme.palette.danger, error);
                    }
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
                        .cloned()
                    {
                        self.timeline_content(ui, &bot);
                    } else {
                        ui.heading("Choose a Bot");
                    }
                }
            }
        });
    }

    fn timeline_content(&mut self, ui: &mut egui::Ui, bot: &BotSummary) {
        ui.set_max_width(self.theme.layout.content_max_width);
        ui.heading(&bot.name);
        self.workspace_controls(ui);
        self.checkpoint_controls(ui);
        if bot.provider == BotProviderStatus::Unavailable {
            ui.colored_label(
                self.theme.palette.warning,
                "This Bot's provider is unavailable. Open Advanced settings to choose another.",
            );
        }
        egui::ScrollArea::vertical()
            .stick_to_bottom(self.timeline.scroll.at_bottom)
            .show(ui, |ui| {
                for item in &self.timeline.messages {
                    let text = item
                        .parts
                        .iter()
                        .filter_map(|part| match part {
                            homebot_protocol::MessagePart::Text { text, .. }
                            | homebot_protocol::MessagePart::Notice { text, .. } => {
                                Some(text.as_str())
                            }
                            homebot_protocol::MessagePart::Attachment { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let identity = (item.author == homebot_protocol::MessageAuthor::Bot)
                        .then(|| identity(self.theme, bot));
                    message(ui, self.theme, identity, &text);
                    ui.add_space(self.theme.spacing.md);
                }
                for item in &self.timeline.activities {
                    activity_card(
                        ui,
                        self.theme,
                        &item.title,
                        &item.detail,
                        item.requires_attention,
                    );
                }
                let pending: Vec<_> = self
                    .timeline
                    .approvals
                    .iter()
                    .filter(|approval| approval.status == homebot_protocol::ApprovalStatus::Pending)
                    .cloned()
                    .collect();
                for approval in pending {
                    ui.group(|ui| {
                        ui.strong(&approval.title);
                        ui.label(&approval.detail);
                        ui.horizontal(|ui| {
                            if ui.button("Allow once").clicked() {
                                self.timeline.decide_approval(approval.id, true);
                            }
                            if ui.button("Deny").clicked() {
                                self.timeline.decide_approval(approval.id, false);
                            }
                        });
                    });
                }
                for prompt in &self.timeline.queued_prompts {
                    ui.label(format!(
                        "Queued {} · {}",
                        prompt.position + 1,
                        prompt.content
                    ));
                }
            });
        ui.separator();
        ui.text_edit_multiline(&mut self.timeline.composer.content);
        if self.composer_error == Some(ComposerError::EmptyComposer) {
            ui.colored_label(
                self.theme.palette.danger,
                "Write a message or attach a file first.",
            );
        }
        ui.horizontal(|ui| {
            let running = self.timeline.chat.as_ref().is_some_and(|chat| chat.running);
            if ui.button(if running { "Queue" } else { "Send" }).clicked() {
                self.composer_error = self.timeline.submit(false).err();
            }
            if running && ui.button("Steer").clicked() {
                self.composer_error = self.timeline.submit(true).err();
            }
            if running && ui.button("Stop").clicked() {
                self.timeline.stop();
            }
            if self.timeline.scroll.unseen_updates > 0 && ui.button("Jump to latest").clicked() {
                self.timeline.set_at_bottom(true);
            }
        });
    }

    fn workspace_controls(&mut self, ui: &mut egui::Ui) {
        if let Some(chat_id) = self.timeline.chat.as_ref().map(|chat| chat.id) {
            let attached = self.workspaces.for_chat(chat_id).cloned();
            let first_repository = self.workspaces.repositories().next().cloned();
            ui.horizontal(|ui| {
                if let Some(workspace) = attached {
                    ui.label(format!(
                        "Repository · {} · {:?}",
                        workspace.branch_name.as_deref().unwrap_or("detached HEAD"),
                        workspace.condition
                    ));
                    if ui.button("Detach").clicked() {
                        self.send_transport(DesktopCommand::Workspace(WorkspaceCommand::Detach {
                            chat_id,
                        }));
                    }
                } else if let Some(repository) = first_repository
                    && ui.button("Attach isolated repository").clicked()
                {
                    self.send_transport(DesktopCommand::Workspace(WorkspaceCommand::Attach {
                        chat_id,
                        workspace_id: repository.id,
                        mode: homebot_protocol::WorkspaceMode::Isolated,
                        base_ref: repository.current_branch,
                        branch_name: None,
                    }));
                }
            });
        }
    }

    fn checkpoint_controls(&mut self, ui: &mut egui::Ui) {
        let checkpoints = self.timeline.checkpoints.clone();
        if checkpoints.is_empty() {
            return;
        }
        ui.horizontal(|ui| {
            ui.label(format!("{} checkpoints", checkpoints.len()));
            if checkpoints.len() >= 2 && ui.button("View latest diff").clicked() {
                let from = checkpoints[checkpoints.len() - 2].id;
                let to = checkpoints[checkpoints.len() - 1].id;
                self.timeline.load_checkpoint_diff(from, to);
            }
            if let Some(checkpoint) = checkpoints.iter().rev().find(|checkpoint| {
                checkpoint.phase == homebot_protocol::CheckpointPhase::BeforeTurn
            }) && ui.button("Restore before last turn").clicked()
            {
                self.timeline.restore_checkpoint(checkpoint.id);
            }
        });
        if let Some(diff) = &self.checkpoint_diff {
            ui.collapsing(format!("Changed files ({})", diff.files.len()), |ui| {
                for file in &diff.files {
                    ui.label(format!("{:?} · {}", file.status, file.path));
                }
            });
        }
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

fn upsert_chat(chats: &mut Vec<ChatSummary>, changed: ChatSummary) {
    if let Some(chat) = chats.iter_mut().find(|chat| chat.id == changed.id) {
        *chat = changed;
    } else {
        chats.push(changed);
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
        attention: match bot.attention {
            BotAttention::None => None,
            BotAttention::Working => Some(AttentionIndicator::Working),
            BotAttention::NeedsApproval => Some(AttentionIndicator::NeedsApproval),
            BotAttention::Failed => Some(AttentionIndicator::Failed),
        },
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

    #[test]
    fn notification_deep_link_selects_exact_bot_chat_and_activity() {
        let mut app = HomeBotApp::default();
        let link = DeepLink {
            bot_id: Some(uuid::Uuid::from_u128(1)),
            chat_id: uuid::Uuid::from_u128(2),
            message_id: Some(uuid::Uuid::from_u128(3)),
            activity_id: Some(uuid::Uuid::from_u128(4)),
        };
        app.open_deep_link(link.clone());
        assert_eq!(app.roster.selected, link.bot_id);
        assert_eq!(app.active_deep_link, Some(link));
    }
}
