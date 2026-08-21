use eframe::egui;
use egui::{Align, CentralPanel, Layout, RichText, SidePanel, TopBottomPanel};
use homebot_protocol::{
    BotAttention, BotColor, BotProviderStatus, BotShape, BotSummary, ChatSummary,
    PullRequestMetadata, ServerEvent, ServerEventBody, VcsStatus,
};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;
use uuid::Uuid;

use crate::{
    bot_roster::{BotEditorDraft, BotRosterModel, ConnectionState, EditorError},
    components::{
        AttentionIndicator, AvatarShape, BotIdentity, activity_card, message, roster_row,
        section_label,
    },
    notifications::{DeepLink, NotificationCenter, NotificationSink, SystemNotificationSink},
    performance::LocalPerformanceTelemetry,
    settings::{DesktopSettings, SettingsAction, SettingsSection, UpdateState, settings_view},
    skills::SkillProjection,
    timeline::{ComposerError, TimelineModel},
    tokens::HomeBotTheme,
    transport::{DesktopCommand, DesktopEvent, DesktopTransport, RuntimeConfig},
    updater::{
        DEFAULT_MANIFEST_URL, UpdateCandidate, UpdateCommand, UpdateCoordinator, UpdateEvent,
        default_staging_directory,
    },
    workspaces::{WorkspaceCommand, WorkspaceProjection},
};

const SETTINGS_STORAGE_KEY: &str = "homebot.desktop.settings.v1";

#[derive(Clone, Debug)]
struct PendingPush {
    chat_id: uuid::Uuid,
    request_id: uuid::Uuid,
    idempotency_key: uuid::Uuid,
    remote: String,
    branch: String,
    approval_id: Option<uuid::Uuid>,
}

#[derive(Clone, Debug)]
struct PendingPullRequest {
    chat_id: uuid::Uuid,
    request_id: uuid::Uuid,
    idempotency_key: uuid::Uuid,
    remote: String,
    head_branch: String,
    base_branch: String,
    title: String,
    approval_id: Option<uuid::Uuid>,
}

pub struct HomeBotApp {
    pub roster: BotRosterModel,
    pub theme: HomeBotTheme,
    pub timeline: TimelineModel,
    pub settings: DesktopSettings,
    pub skills: SkillProjection,
    pub workspaces: WorkspaceProjection,
    pub checkpoint_diff: Option<homebot_protocol::CheckpointDiffResponse>,
    pub devices: Vec<homebot_protocol::DeviceSessionSummary>,
    pub pairing_offer: Option<homebot_protocol::PairingOffer>,
    pairing_endpoint: String,
    pairing_insecure_private_acknowledged: bool,
    vcs_commit_message: String,
    vcs_branch_name: String,
    vcs_base_branch: String,
    vcs_pr_title: String,
    pending_push: Option<PendingPush>,
    pending_pull_request: Option<PendingPullRequest>,
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
    updater: Option<UpdateCoordinator>,
    update_candidate: Option<UpdateCandidate>,
    performance: LocalPerformanceTelemetry,
    focus_composer: bool,
    delete_confirmation: Option<(Uuid, String, String)>,
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
            devices: Vec::new(),
            pairing_offer: None,
            pairing_endpoint: "http://127.0.0.1:7123".to_owned(),
            pairing_insecure_private_acknowledged: false,
            vcs_commit_message: String::new(),
            vcs_branch_name: String::new(),
            vcs_base_branch: "main".to_owned(),
            vcs_pr_title: String::new(),
            pending_push: None,
            pending_pull_request: None,
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
            updater: None,
            update_candidate: None,
            performance: LocalPerformanceTelemetry::default(),
            focus_composer: false,
            delete_confirmation: None,
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
        app.pairing_endpoint
            .clone_from(&app.settings.server_endpoint);
        app.transport = Some(DesktopTransport::start(RuntimeConfig::desktop_default(
            app.settings.server_endpoint.clone(),
        )));
        app
    }

    pub fn render(&mut self, context: &egui::Context) {
        let frame_started = Instant::now();
        self.handle_keyboard_shortcuts(context);
        self.pump_transport(context);
        self.pump_updater();
        let activated: Vec<_> = self.deep_link_receiver.try_iter().collect();
        for deep_link in activated {
            self.open_deep_link(deep_link);
        }
        let system_dark = context.system_theme() == Some(egui::Theme::Dark);
        self.theme = match self.settings.resolved_theme(system_dark) {
            crate::tokens::ThemeMode::Light => HomeBotTheme::light(),
            crate::tokens::ThemeMode::Dark => HomeBotTheme::dark(),
        }
        .with_text_scale(f32::from(self.settings.text_scale_percent) / 100.0);
        self.theme.install(context);
        self.sidebar(context);
        self.titlebar(context);
        CentralPanel::default().show(context, |ui| {
            if self.settings_open {
                if let Some(action) = settings_view(ui, self.theme, &mut self.settings) {
                    self.handle_settings_action(action);
                }
                if self.settings.section == SettingsSection::Devices {
                    self.device_pairing_controls(ui);
                }
            } else {
                self.content(ui);
            }
        });
        self.editor(context);
        self.delete_dialog(context);
        self.flush_transport();
        self.performance
            .record("desktop_frame", frame_started.elapsed());
        context.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn handle_keyboard_shortcuts(&mut self, context: &egui::Context) {
        let (settings, create_bot, composer, escape) = context.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma),
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::N),
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::K),
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if settings {
            self.settings_open = !self.settings_open;
        }
        if create_bot {
            self.settings_open = false;
            self.roster.begin_create();
        }
        if composer {
            self.settings_open = false;
            self.focus_composer = true;
        }
        if escape {
            self.settings_open = false;
            self.roster.editor = None;
            self.editor_error = None;
            self.delete_confirmation = None;
        }
    }

    fn handle_settings_action(&mut self, action: SettingsAction) {
        let updater = self.updater.get_or_insert_with(|| {
            UpdateCoordinator::start(DEFAULT_MANIFEST_URL, default_staging_directory())
        });
        match action {
            SettingsAction::CheckForUpdate => {
                self.settings.update_state = UpdateState::Checking;
                self.settings.update_message = None;
                self.settings.update_version = None;
                self.update_candidate = None;
                updater.send(UpdateCommand::Check);
            }
            SettingsAction::StageUpdate => {
                if let Some(candidate) = self.update_candidate.clone() {
                    self.settings.update_state = UpdateState::Staging;
                    self.settings.update_message = None;
                    updater.send(UpdateCommand::Stage(candidate));
                }
            }
        }
    }

    fn pump_updater(&mut self) {
        let events = self
            .updater
            .as_ref()
            .map_or_else(Vec::new, |updater| updater.try_events().collect());
        for event in events {
            match event {
                UpdateEvent::Current => {
                    self.settings.update_state = UpdateState::Current;
                    self.settings.update_version = None;
                    self.settings.update_message = None;
                    self.update_candidate = None;
                }
                UpdateEvent::Available(candidate) => {
                    self.settings.update_state = UpdateState::Available;
                    self.settings.update_version = Some(candidate.version.clone());
                    self.settings.update_message =
                        Some("HomeBot will download only after you approve it.".to_owned());
                    self.update_candidate = Some(candidate);
                }
                UpdateEvent::Staged { version, path } => {
                    self.settings.update_state = UpdateState::Ready;
                    self.settings.update_version = Some(version);
                    self.settings.update_message = Some(format!(
                        "Verified package staged at {}. Installation remains a separate explicit action.",
                        path.display()
                    ));
                    self.update_candidate = None;
                }
                UpdateEvent::Failed(message) => {
                    self.settings.update_state = UpdateState::Failed;
                    self.settings.update_message = Some(message);
                }
            }
        }
    }

    fn pump_transport(&mut self, context: &egui::Context) {
        let events = self
            .transport
            .as_ref()
            .map_or_else(Vec::new, |transport| transport.try_events().collect());
        for event in events {
            let Some(event) = self.apply_vcs_transport_event(event) else {
                continue;
            };
            match event {
                DesktopEvent::Connecting => self.roster.connection = ConnectionState::Connecting,
                DesktopEvent::Connected => {
                    self.roster.connection = ConnectionState::Connected;
                    self.transport_error = None;
                    self.send_transport(DesktopCommand::LoadDevices);
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
                        ServerEventBody::BotDeleted { bot_id } => self.apply_bot_deleted(*bot_id),
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
                    if let ServerEventBody::ApprovalChanged { approval } = &event.body
                        && self
                            .pending_push
                            .as_ref()
                            .and_then(|pending| pending.approval_id)
                            == Some(approval.id)
                        && approval.status == homebot_protocol::ApprovalStatus::Denied
                    {
                        self.pending_push = None;
                    }
                    if let ServerEventBody::ApprovalChanged { approval } = &event.body
                        && self
                            .pending_pull_request
                            .as_ref()
                            .and_then(|pending| pending.approval_id)
                            == Some(approval.id)
                        && approval.status == homebot_protocol::ApprovalStatus::Denied
                    {
                        self.pending_pull_request = None;
                    }
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
                DesktopEvent::WorkingContext(context) => {
                    self.timeline.working_context = Some(context);
                }
                DesktopEvent::Devices(devices) => self.apply_devices(devices),
                DesktopEvent::PairingOffer(offer) => self.pairing_offer = Some(offer),
                DesktopEvent::DeviceRevoked(device) => self.apply_device_revoked(device),
                DesktopEvent::CheckpointDiff(diff) => self.checkpoint_diff = Some(diff),
                DesktopEvent::MutationFailed(error) => {
                    self.transport_error = Some(error.to_string());
                }
                _ => unreachable!("source-control events are consumed before primary dispatch"),
            }
        }
    }

    fn apply_bot_deleted(&mut self, bot_id: Uuid) -> Option<Uuid> {
        self.roster.apply_delete(bot_id);
        self.chats.retain(|chat| chat.bot_id != bot_id);
        None
    }

    fn apply_devices(&mut self, devices: Vec<homebot_protocol::DeviceSessionSummary>) {
        self.devices = devices;
        self.update_device_count();
    }

    fn apply_device_revoked(&mut self, device: homebot_protocol::DeviceSessionSummary) {
        if let Some(existing) = self.devices.iter_mut().find(|item| item.id == device.id) {
            *existing = device;
        }
        self.update_device_count();
    }

    fn update_device_count(&mut self) {
        self.settings.paired_devices = u32::try_from(
            self.devices
                .iter()
                .filter(|device| device.revoked_at_unix_ms.is_none())
                .count(),
        )
        .unwrap_or(u32::MAX);
    }

    fn apply_vcs_transport_event(&mut self, event: DesktopEvent) -> Option<DesktopEvent> {
        match event {
            DesktopEvent::VcsStatus { chat_id, status } => {
                self.workspaces.apply_vcs_status(chat_id, status);
            }
            DesktopEvent::VcsDiff { chat_id, diff } => {
                self.workspaces.apply_vcs_diff(chat_id, diff);
            }
            DesktopEvent::VcsCommit { chat_id, .. } => {
                self.vcs_commit_message.clear();
                self.send_transport(DesktopCommand::Workspace(WorkspaceCommand::LoadStatus {
                    chat_id,
                }));
            }
            DesktopEvent::VcsRemoteMutation { chat_id, response } => {
                if response.status == homebot_protocol::VcsMutationStatus::ApprovalRequired {
                    if let Some(pending) = &mut self.pending_push {
                        pending.approval_id = response.approval.as_ref().map(|value| value.id);
                    }
                } else {
                    self.pending_push = None;
                }
                self.workspaces.apply_remote_mutation(chat_id, response);
            }
            DesktopEvent::PullRequestMetadata { chat_id, metadata } => {
                self.workspaces.apply_pull_request(chat_id, metadata);
            }
            DesktopEvent::PullRequestMutation { chat_id, response } => {
                if response.status == homebot_protocol::VcsMutationStatus::ApprovalRequired {
                    if let Some(pending) = &mut self.pending_pull_request {
                        pending.approval_id = response.approval.as_ref().map(|value| value.id);
                    }
                } else {
                    self.pending_pull_request = None;
                    self.vcs_pr_title.clear();
                }
                self.workspaces
                    .apply_pull_request_mutation(chat_id, response);
            }
            other => return Some(other),
        }
        None
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
            let chat_id = chat.id;
            self.send_transport(DesktopCommand::LoadTimeline(chat_id));
            if self.workspaces.for_chat(chat_id).is_some() {
                self.send_transport(DesktopCommand::Workspace(WorkspaceCommand::LoadStatus {
                    chat_id,
                }));
            }
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

    fn device_pairing_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("Secure Android pairing");
        ui.label("Generate a five-minute, single-use link. Persistent device credentials are returned only after exchange and never appear in the link.");
        ui.horizontal(|ui| {
            ui.label("Endpoint");
            ui.text_edit_singleline(&mut self.pairing_endpoint);
            if ui.button("Generate link").clicked() {
                self.pairing_offer = None;
                self.send_transport(DesktopCommand::CreatePairing {
                    endpoint: self.pairing_endpoint.trim().to_owned(),
                    allow_insecure_private_network: self.pairing_insecure_private_acknowledged,
                });
            }
        });
        let _ = ui.checkbox(
            &mut self.pairing_insecure_private_acknowledged,
            "Allow plain HTTP only on this private LAN/Tailscale endpoint",
        );
        if let Some(offer) = &self.pairing_offer {
            ui.label(format!("Expires at {} ms", offer.expires_at_unix_ms));
            if let Some(warning) = &offer.warning {
                ui.colored_label(self.theme.palette.warning, warning);
            }
            ui.horizontal_wrapped(|ui| {
                ui.monospace(&offer.deep_link);
                if ui.button("Copy pairing link").clicked() {
                    ui.ctx().copy_text(offer.deep_link.clone());
                }
            });
        }
        ui.separator();
        ui.strong("Device sessions");
        let mut revoke = None;
        for device in &self.devices {
            ui.horizontal(|ui| {
                ui.label(&device.name);
                ui.label(format!("{:?}", device.endpoint_kind));
                if device.revoked_at_unix_ms.is_some() {
                    ui.label("Revoked");
                } else if ui.button("Revoke").clicked() {
                    revoke = Some(device.id);
                }
            });
        }
        if let Some(device_id) = revoke {
            self.send_transport(DesktopCommand::RevokeDevice(device_id));
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
                            if ui
                                .button(if bot.pinned { "Unpin" } else { "Pin" })
                                .clicked()
                            {
                                self.roster.queue_pin(bot.id, bot.pinned);
                            }
                            if ui.button("Duplicate").clicked() {
                                self.roster.queue_duplicate(bot.id);
                            }
                            if ui.button("Hide").clicked() {
                                self.roster.queue_hide(bot.id, bot.hidden);
                            }
                            if ui.button("Delete…").clicked() {
                                self.delete_confirmation =
                                    Some((bot.id, bot.name.clone(), String::new()));
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
                    let label = match prompt.kind {
                        homebot_protocol::QueuedPromptKind::FollowUp => "Queued",
                        homebot_protocol::QueuedPromptKind::Steering => "Steering",
                    };
                    ui.label(format!(
                        "{label} {} · {}",
                        prompt.position + 1,
                        prompt.content
                    ));
                }
            });
        self.working_context_controls(ui);
        self.composer_controls(ui);
    }

    fn composer_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        let composer = ui.text_edit_multiline(&mut self.timeline.composer.content);
        if self.focus_composer {
            composer.request_focus();
            self.focus_composer = false;
        }
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

    fn working_context_controls(&mut self, ui: &mut egui::Ui) {
        let Some(context) = self.timeline.working_context.clone() else {
            return;
        };
        let idle = self
            .timeline
            .chat
            .as_ref()
            .is_some_and(|chat| !chat.running);
        let context_idle =
            context.compaction_status != homebot_protocol::ContextCompactionStatus::Running;
        ui.horizontal(|ui| {
            let usage = match (context.used_tokens, context.context_window_tokens) {
                (Some(used), Some(limit)) => format!("Context {used}/{limit} tokens"),
                (Some(used), None) => format!("Context {used} tokens"),
                _ => "Context size unavailable".to_owned(),
            };
            ui.label(usage);
            if context.plan_mode_available {
                let plan = context.interaction_mode == homebot_protocol::InteractionMode::Plan;
                if ui
                    .button(if plan { "Use default" } else { "Use plan" })
                    .clicked()
                {
                    self.timeline.set_interaction_mode(if plan {
                        homebot_protocol::InteractionMode::Default
                    } else {
                        homebot_protocol::InteractionMode::Plan
                    });
                }
            }
            if context.compaction_available
                && ui
                    .add_enabled(idle && context_idle, egui::Button::new("Compact"))
                    .clicked()
            {
                self.timeline
                    .compact_context(homebot_protocol::ContextCompactionStrategy::Compact);
            }
            if context.reset_available
                && ui
                    .add_enabled(idle && context_idle, egui::Button::new("Reset context"))
                    .clicked()
            {
                self.timeline
                    .compact_context(homebot_protocol::ContextCompactionStrategy::Reset);
            }
            match context.compaction_status {
                homebot_protocol::ContextCompactionStatus::Running => {
                    ui.label("Updating context…");
                }
                homebot_protocol::ContextCompactionStatus::Completed => {
                    ui.label(format!("Context generation {}", context.generation));
                }
                homebot_protocol::ContextCompactionStatus::Failed => {
                    ui.colored_label(self.theme.palette.danger, "Context operation failed");
                }
                homebot_protocol::ContextCompactionStatus::Idle => {}
            }
        });
    }

    fn workspace_controls(&mut self, ui: &mut egui::Ui) {
        if let Some(chat_id) = self.timeline.chat.as_ref().map(|chat| chat.id) {
            let attached = self.workspaces.for_chat(chat_id).cloned();
            let first_repository = self.workspaces.repositories().next().cloned();
            ui.horizontal(|ui| {
                if let Some(workspace) = attached.as_ref() {
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
                    if ui.button("Source control").clicked() {
                        self.send_transport(DesktopCommand::Workspace(
                            WorkspaceCommand::LoadStatus { chat_id },
                        ));
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
            if attached.is_some() {
                self.source_control_controls(ui, chat_id);
            }
        }
    }

    fn source_control_controls(&mut self, ui: &mut egui::Ui, chat_id: Uuid) {
        let status = self.workspaces.vcs_status(chat_id).cloned();
        let Some(status) = status else {
            return;
        };
        let pull_request = self.workspaces.pull_request(chat_id).cloned();
        ui.collapsing("Source control", |ui| {
            ui.label(format!(
                "{} · {} changed · ↑{} ↓{}",
                status.branch.as_deref().unwrap_or("detached HEAD"),
                status.entries.len(),
                status.ahead,
                status.behind
            ));
            self.vcs_diff_controls(ui, chat_id);
            self.vcs_commit_controls(ui, chat_id);
            self.vcs_branch_push_controls(ui, chat_id, &status);
            self.vcs_pull_request_controls(ui, chat_id, &status, pull_request.as_ref());
        });
    }

    fn vcs_diff_controls(&mut self, ui: &mut egui::Ui, chat_id: Uuid) {
        ui.horizontal(|ui| {
            for (label, staged) in [("Staged diff", true), ("Working diff", false)] {
                if ui.button(label).clicked() {
                    self.send_transport(DesktopCommand::Workspace(WorkspaceCommand::LoadDiff {
                        chat_id,
                        staged,
                    }));
                }
            }
            if let Some(diff) = self.workspaces.vcs_diff(chat_id, true) {
                ui.label(format!("{} staged", diff.files.len()));
            }
            if let Some(diff) = self.workspaces.vcs_diff(chat_id, false) {
                ui.label(format!("{} working", diff.files.len()));
            }
        });
    }

    fn vcs_commit_controls(&mut self, ui: &mut egui::Ui, chat_id: Uuid) {
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.vcs_commit_message);
            if ui
                .add_enabled(
                    !self.vcs_commit_message.trim().is_empty(),
                    egui::Button::new("Commit all"),
                )
                .clicked()
            {
                self.send_transport(DesktopCommand::Workspace(WorkspaceCommand::Commit {
                    chat_id,
                    message: self.vcs_commit_message.clone(),
                    stage_all: true,
                }));
            }
        });
    }

    fn vcs_branch_push_controls(&mut self, ui: &mut egui::Ui, chat_id: Uuid, status: &VcsStatus) {
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.vcs_branch_name);
            if ui
                .add_enabled(
                    !self.vcs_branch_name.trim().is_empty() && status.entries.is_empty(),
                    egui::Button::new("Create branch"),
                )
                .clicked()
            {
                self.send_transport(DesktopCommand::Workspace(WorkspaceCommand::CreateBranch {
                    chat_id,
                    branch: self.vcs_branch_name.trim().to_owned(),
                    start_point: Some("HEAD".to_owned()),
                }));
            }
            let can_push = status.branch.is_some()
                && status.remotes.iter().any(|remote| remote.push_configured);
            if ui
                .add_enabled(can_push, egui::Button::new("Push"))
                .clicked()
            {
                self.push_current_branch(chat_id, status);
            }
        });
    }

    fn push_current_branch(&mut self, chat_id: Uuid, status: &VcsStatus) {
        let Some(branch) = status.branch.clone() else {
            return;
        };
        let pending = self.pending_push.clone().unwrap_or_else(|| PendingPush {
            chat_id,
            request_id: Uuid::now_v7(),
            idempotency_key: Uuid::now_v7(),
            remote: status
                .remotes
                .iter()
                .find(|remote| remote.push_configured)
                .map_or_else(|| "origin".to_owned(), |remote| remote.name.clone()),
            branch,
            approval_id: None,
        });
        self.send_transport(DesktopCommand::Workspace(WorkspaceCommand::Push {
            chat_id: pending.chat_id,
            request_id: pending.request_id,
            idempotency_key: pending.idempotency_key,
            remote: pending.remote.clone(),
            branch: pending.branch.clone(),
            set_upstream: true,
            approval_id: pending.approval_id,
        }));
        self.pending_push = Some(pending);
    }

    fn vcs_pull_request_controls(
        &mut self,
        ui: &mut egui::Ui,
        chat_id: Uuid,
        status: &VcsStatus,
        metadata: Option<&PullRequestMetadata>,
    ) {
        let remote = status
            .remotes
            .iter()
            .find(|remote| remote.push_configured)
            .map(|remote| remote.name.clone());
        let head = status.branch.clone();
        ui.horizontal(|ui| {
            ui.label("Base");
            ui.text_edit_singleline(&mut self.vcs_base_branch);
            let can_load =
                remote.is_some() && head.is_some() && !self.vcs_base_branch.trim().is_empty();
            if ui
                .add_enabled(can_load, egui::Button::new("PR status"))
                .clicked()
                && let (Some(remote), Some(head_branch)) = (remote.clone(), head.clone())
            {
                self.send_transport(DesktopCommand::Workspace(
                    WorkspaceCommand::LoadPullRequest {
                        chat_id,
                        remote,
                        head_branch,
                        base_branch: self.vcs_base_branch.trim().to_owned(),
                    },
                ));
            }
            if let Some(current) = metadata.and_then(|value| value.current.as_ref()) {
                ui.label(format!("PR #{} · {}", current.number, current.state));
            }
        });
        if metadata.is_some_and(|value| value.current.is_none() && value.create_available) {
            self.vcs_create_pull_request_controls(ui, chat_id, remote, head);
        }
    }

    fn vcs_create_pull_request_controls(
        &mut self,
        ui: &mut egui::Ui,
        chat_id: Uuid,
        remote: Option<String>,
        head: Option<String>,
    ) {
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.vcs_pr_title);
            if ui
                .add_enabled(
                    !self.vcs_pr_title.trim().is_empty(),
                    egui::Button::new("Create PR"),
                )
                .clicked()
                && let (Some(remote), Some(head_branch)) = (remote, head)
            {
                let pending =
                    self.pending_pull_request
                        .clone()
                        .unwrap_or_else(|| PendingPullRequest {
                            chat_id,
                            request_id: Uuid::now_v7(),
                            idempotency_key: Uuid::now_v7(),
                            remote,
                            head_branch,
                            base_branch: self.vcs_base_branch.trim().to_owned(),
                            title: self.vcs_pr_title.trim().to_owned(),
                            approval_id: None,
                        });
                self.send_transport(DesktopCommand::Workspace(
                    WorkspaceCommand::CreatePullRequest {
                        chat_id: pending.chat_id,
                        request_id: pending.request_id,
                        idempotency_key: pending.idempotency_key,
                        remote: pending.remote.clone(),
                        head_branch: pending.head_branch.clone(),
                        base_branch: pending.base_branch.clone(),
                        title: pending.title.clone(),
                        body: "Created with HomeBot.".to_owned(),
                        draft: false,
                        approval_id: pending.approval_id,
                    },
                ));
                self.pending_pull_request = Some(pending);
            }
        });
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

    fn delete_dialog(&mut self, context: &egui::Context) {
        let Some((bot_id, name, mut confirmation)) = self.delete_confirmation.take() else {
            return;
        };
        let mut keep_open = true;
        egui::Window::new("Delete Bot")
            .collapsible(false).resizable(false)
            .show(context, |ui| {
                ui.label(format!("Delete {name} and its HomeBot chat and routines? Shared computer files are not deleted."));
                ui.label(format!("Type {name} to confirm"));
                ui.text_edit_singleline(&mut confirmation);
                ui.horizontal(|ui| {
                    if ui.add_enabled(confirmation == name, egui::Button::new("Delete permanently")).clicked() {
                        self.roster.queue_delete(bot_id, confirmation.clone());
                        keep_open = false;
                    }
                    if ui.button("Cancel").clicked() { keep_open = false; }
                });
            });
        if keep_open {
            self.delete_confirmation = Some((bot_id, name, confirmation));
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
