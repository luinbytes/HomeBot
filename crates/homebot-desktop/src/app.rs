use eframe::egui;
use egui::{
    Align, CentralPanel, CornerRadius, Frame, Layout, RichText, SidePanel, Stroke, TopBottomPanel,
};
use homebot_protocol::{
    ActivityDetail, ActivityKind, ActivityPresentation, ActivityStatus, ActivitySummary,
    ApprovalStatus, ApprovalSummary, BotAdvancedSettings, BotAttention, BotColor,
    BotPermissionProfile, BotProviderStatus, BotShape, BotSummary, ChatSummary,
    ChatTimelineResponse, GroupBotStatus, GroupChatSummary, GroupParticipantRole,
    GroupParticipantSummary, GroupTimelineResponse, MessageAuthor, MessagePart, MessageStatus,
    MessageSummary, ProviderProfileSummary, PullRequestMetadata, RecordedAction, RecordedActor,
    RiskLevel, RoutineDefinition, RoutineRecordingSummary, RoutineStep, RoutineSummary,
    ServerEvent, ServerEventBody, VcsStatus,
};
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;
use uuid::Uuid;

use crate::{
    activity_surfaces::{ActivityAction, ActivityCardModel, activity_surface},
    bot_roster::{BotEditorDraft, BotRosterModel, ConnectionState, EditorError},
    components::{
        AttentionIndicator, AvatarShape, BotIdentity, activity_card, message, navigation_row,
        recent_conversation_row, roster_row, send_button,
    },
    group_timeline::{GroupComposerError, GroupTimelineModel},
    notifications::{DeepLink, NotificationCenter, NotificationSink, SystemNotificationSink},
    performance::LocalPerformanceTelemetry,
    routines::RoutineProjection,
    settings::{
        DesktopSettings, PluginAction, PluginSettingsItem, PluginViewState, SettingsAction,
        SettingsSection, ThemePreference, UpdateState, settings_view_with,
    },
    skills::SkillProjection,
    timeline::{ComposerError, TimelineEntry, TimelineModel},
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

#[derive(Clone, Debug, Default)]
struct SearchProjection {
    query: String,
    results: Vec<homebot_protocol::SearchResultSummary>,
}

#[derive(Clone, Debug)]
struct RoutineEditorDraft {
    routine_id: Option<Uuid>,
    bot_id: Uuid,
    name: String,
    description: String,
    definition: RoutineDefinition,
    draft: bool,
}

#[derive(Clone, Debug)]
struct AssistantPackInstallDraft {
    pack_id: String,
    bot_id: Uuid,
    timezone: String,
    hour: u8,
    minute: u8,
}

impl RoutineEditorDraft {
    fn from_summary(routine: &RoutineSummary) -> Self {
        Self {
            routine_id: Some(routine.id),
            bot_id: routine.bot_id,
            name: routine.name.clone(),
            description: routine.description.clone(),
            definition: routine.definition.clone(),
            draft: routine.draft,
        }
    }
}

// These flags represent independent transient overlays and focus requests rather than a single
// mutually exclusive state machine (settings, routines, details, composer focus, pairing ack).
#[allow(clippy::struct_excessive_bools)]
pub struct HomeBotApp {
    pub roster: BotRosterModel,
    pub theme: HomeBotTheme,
    pub timeline: TimelineModel,
    pub group_timeline: GroupTimelineModel,
    pub settings: DesktopSettings,
    pub skills: SkillProjection,
    pub routines: RoutineProjection,
    pub workspaces: WorkspaceProjection,
    pub checkpoint_diff: Option<homebot_protocol::CheckpointDiffResponse>,
    pub devices: Vec<homebot_protocol::DeviceSessionSummary>,
    pub pairing_offer: Option<homebot_protocol::PairingOffer>,
    pub capability_rules: Vec<homebot_protocol::CapabilityRuleSummary>,
    pub browser_sessions: Vec<homebot_protocol::BrowserSessionSummary>,
    pub provider_profiles: Vec<ProviderProfileSummary>,
    pub assistant_packs: Vec<homebot_protocol::AssistantPackSummary>,
    pairing_endpoint: String,
    pairing_insecure_private_acknowledged: bool,
    vcs_commit_message: String,
    vcs_branch_name: String,
    vcs_base_branch: String,
    vcs_pr_title: String,
    group_title_draft: String,
    pending_push: Option<PendingPush>,
    pending_pull_request: Option<PendingPullRequest>,
    pub active_deep_link: Option<DeepLink>,
    notification_center: NotificationCenter,
    notification_sink: SystemNotificationSink,
    deep_link_receiver: Receiver<DeepLink>,
    settings_open: bool,
    assistant_packs_open: bool,
    assistant_pack_install: Option<AssistantPackInstallDraft>,
    assistant_pack_notice: Option<String>,
    routines_open: bool,
    routine_editor: Option<RoutineEditorDraft>,
    routine_recording: Option<RoutineRecordingSummary>,
    routine_recording_prompt: String,
    routine_recording_requires_approval: bool,
    details_open: bool,
    sidebar_collapsed: bool,
    search: Option<SearchProjection>,
    editor_error: Option<EditorError>,
    composer_error: Option<ComposerError>,
    transport_error: Option<String>,
    chats: Vec<ChatSummary>,
    groups: Vec<GroupChatSummary>,
    selected_group: Option<Uuid>,
    transport: Option<DesktopTransport>,
    updater: Option<UpdateCoordinator>,
    update_candidate: Option<UpdateCandidate>,
    performance: LocalPerformanceTelemetry,
    focus_composer: bool,
    expanded_activities: HashSet<Uuid>,
    delete_confirmation: Option<(Uuid, String, String)>,
}

impl Default for HomeBotApp {
    fn default() -> Self {
        let (deep_link_sender, deep_link_receiver) = channel();
        Self {
            roster: BotRosterModel::default(),
            theme: HomeBotTheme::light(),
            timeline: TimelineModel::default(),
            group_timeline: GroupTimelineModel::default(),
            settings: DesktopSettings::default(),
            skills: SkillProjection::default(),
            routines: RoutineProjection::default(),
            workspaces: WorkspaceProjection::default(),
            checkpoint_diff: None,
            devices: Vec::new(),
            pairing_offer: None,
            capability_rules: Vec::new(),
            browser_sessions: Vec::new(),
            provider_profiles: Vec::new(),
            assistant_packs: Vec::new(),
            pairing_endpoint: "http://127.0.0.1:7123".to_owned(),
            pairing_insecure_private_acknowledged: false,
            vcs_commit_message: String::new(),
            vcs_branch_name: String::new(),
            vcs_base_branch: "main".to_owned(),
            vcs_pr_title: String::new(),
            group_title_draft: String::new(),
            pending_push: None,
            pending_pull_request: None,
            active_deep_link: None,
            notification_center: NotificationCenter::default(),
            notification_sink: SystemNotificationSink::new(deep_link_sender),
            deep_link_receiver,
            settings_open: false,
            assistant_packs_open: false,
            assistant_pack_install: None,
            assistant_pack_notice: None,
            routines_open: false,
            routine_editor: None,
            routine_recording: None,
            routine_recording_prompt: String::new(),
            routine_recording_requires_approval: true,
            details_open: false,
            sidebar_collapsed: false,
            search: None,
            editor_error: None,
            composer_error: None,
            transport_error: None,
            chats: Vec::new(),
            groups: Vec::new(),
            selected_group: None,
            transport: None,
            updater: None,
            update_candidate: None,
            performance: LocalPerformanceTelemetry::default(),
            focus_composer: false,
            expanded_activities: HashSet::new(),
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
        self.handle_dropped_files(context);
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
        if self.should_show_composer() {
            self.composer_panel(context);
        }
        CentralPanel::default().show(context, |ui| {
            if self.assistant_packs_open {
                self.assistant_pack_content(ui);
            } else if self.routines_open {
                self.routine_content(ui);
            } else if self.search.is_some() {
                self.search_content(ui);
            } else {
                self.content(ui);
            }
        });
        self.settings_dialog(context);
        self.editor(context);
        self.delete_dialog(context);
        self.flush_transport();
        self.performance
            .record("desktop_frame", frame_started.elapsed());
        context.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn settings_dialog(&mut self, context: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut settings = std::mem::take(&mut self.settings);
        let response = egui::Modal::new(egui::Id::new("settings_modal"))
            .backdrop_color(self.theme.palette.overlay)
            .frame(
                Frame::NONE
                    .fill(self.theme.palette.surface)
                    .stroke(Stroke::new(
                        self.theme.layout.hairline,
                        self.theme.palette.border,
                    ))
                    .corner_radius(CornerRadius::same(self.theme.radii.md))
                    .inner_margin(egui::Margin::same(self.theme.insets.lg))
                    .shadow(self.theme.popup_shadow),
            )
            .show(context, |ui| {
                ui.set_min_size(egui::vec2(860.0, 600.0));
                ui.set_max_size(egui::vec2(860.0, 620.0));
                ui.horizontal(|ui| {
                    ui.heading("Settings");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            ui.close();
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("settings_content")
                    .show(ui, |ui| {
                        settings_view_with(ui, self.theme, &mut settings, |ui, section| {
                            if section == SettingsSection::Devices {
                                self.device_pairing_controls(ui);
                                self.capability_policy_controls(ui);
                                self.shared_browser_controls(ui);
                            }
                        })
                    })
                    .inner
            });
        self.settings = settings;
        if response.should_close() {
            self.settings_open = false;
        }
        if let Some(action) = response.inner {
            self.handle_settings_action(action);
        }
    }

    fn handle_keyboard_shortcuts(&mut self, context: &egui::Context) {
        let (settings, create_bot, composer, search, sidebar, escape) =
            context.input_mut(|input| {
                (
                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma),
                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::N),
                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::K),
                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::F),
                    input.consume_key(egui::Modifiers::COMMAND, egui::Key::B),
                    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
        if settings {
            self.settings_open = !self.settings_open;
            self.assistant_packs_open = false;
            self.routines_open = false;
        }
        if create_bot {
            self.settings_open = false;
            self.assistant_packs_open = false;
            self.routines_open = false;
            self.roster.begin_create();
        }
        if composer {
            self.settings_open = false;
            self.assistant_packs_open = false;
            self.routines_open = false;
            self.focus_composer = true;
        }
        if search {
            self.settings_open = false;
            self.assistant_packs_open = false;
            self.routines_open = false;
            self.search.get_or_insert_with(SearchProjection::default);
        }
        if sidebar {
            self.sidebar_collapsed = !self.sidebar_collapsed;
        }
        if escape {
            self.settings_open = false;
            self.assistant_packs_open = false;
            self.routines_open = false;
            self.roster.editor = None;
            self.editor_error = None;
            self.delete_confirmation = None;
        }
    }

    fn handle_dropped_files(&mut self, context: &egui::Context) {
        let files = context.input(|input| input.raw.dropped_files.clone());
        for file in files.into_iter().take(6) {
            let filename = file.name.clone();
            let bytes = file.bytes.map_or_else(
                || file.path.as_ref().and_then(|path| std::fs::read(path).ok()),
                |bytes| Some(bytes.to_vec()),
            );
            if let Some(bytes) = bytes {
                self.send_transport(DesktopCommand::UploadAttachment {
                    filename,
                    media_type: if file.mime.is_empty() {
                        "application/octet-stream".to_owned()
                    } else {
                        file.mime
                    },
                    bytes,
                });
            } else {
                self.transport_error =
                    Some("HomeBot could not read the dropped attachment.".to_owned());
            }
        }
    }

    fn handle_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::CheckForUpdate => {
                let updater = self.updater.get_or_insert_with(|| {
                    UpdateCoordinator::start(DEFAULT_MANIFEST_URL, default_staging_directory())
                });
                self.settings.update_state = UpdateState::Checking;
                self.settings.update_message = None;
                self.settings.update_version = None;
                self.update_candidate = None;
                updater.send(UpdateCommand::Check);
            }
            SettingsAction::StageUpdate => {
                if let Some(candidate) = self.update_candidate.clone() {
                    let updater = self.updater.get_or_insert_with(|| {
                        UpdateCoordinator::start(DEFAULT_MANIFEST_URL, default_staging_directory())
                    });
                    self.settings.update_state = UpdateState::Staging;
                    self.settings.update_message = None;
                    updater.send(UpdateCommand::Stage(candidate));
                }
            }
            SettingsAction::Reconnect => {
                self.settings.server_endpoint = self
                    .settings
                    .server_endpoint
                    .trim()
                    .trim_end_matches('/')
                    .to_owned();
                self.pairing_endpoint
                    .clone_from(&self.settings.server_endpoint);
                self.roster.connection = ConnectionState::Connecting;
                self.transport = Some(DesktopTransport::start(RuntimeConfig::desktop_default(
                    self.settings.server_endpoint.clone(),
                )));
            }
            SettingsAction::RefreshPlugins => {
                self.send_transport(DesktopCommand::LoadPlugins);
            }
            SettingsAction::SetLaunchAtLogin(enabled) => {
                if let Err(error) = set_launch_at_login(enabled) {
                    self.settings.launch_at_login = !enabled;
                    self.transport_error = Some(format!("Could not update login item: {error}"));
                }
            }
            SettingsAction::Plugin { id, action } => {
                let action = match action {
                    PluginAction::Connect => "connect",
                    PluginAction::Enable => "enable",
                    PluginAction::Disable => "disable",
                };
                self.send_transport(DesktopCommand::MutatePlugin {
                    plugin_id: id,
                    action,
                });
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

    #[allow(clippy::too_many_lines)] // Exhaustive dispatch keeps every server event visible here.
    fn pump_transport(&mut self, context: &egui::Context) {
        let events = self
            .transport
            .as_ref()
            .map_or_else(Vec::new, |transport| transport.try_events().collect());
        for event in events {
            let Some(event) = self.apply_vcs_transport_event(event) else {
                continue;
            };
            let Some(event) = self.apply_policy_transport_event(event) else {
                continue;
            };
            let Some(event) = self.apply_browser_transport_event(event) else {
                continue;
            };
            match event {
                DesktopEvent::Connecting => self.roster.connection = ConnectionState::Connecting,
                DesktopEvent::Connected => self.apply_connected(),
                DesktopEvent::Disconnected(error) => {
                    self.roster.connection = ConnectionState::Disconnected;
                    self.transport_error = Some(error.to_string());
                }
                DesktopEvent::Snapshot { snapshot, .. } => {
                    self.apply_snapshot(snapshot);
                    self.load_selected_timeline();
                }
                DesktopEvent::Server(event) => {
                    self.skills.apply(&event);
                    self.routines.apply(&event);
                    self.workspaces.apply(&event);
                    let _ = self.group_timeline.apply_event(event.clone());
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
                        ServerEventBody::GroupChatChanged { group } => {
                            if let Some(existing) =
                                self.groups.iter_mut().find(|item| item.id == group.id)
                            {
                                *existing = group.clone();
                            } else {
                                self.groups.push(group.clone());
                            }
                            if self.selected_group == Some(group.id) {
                                self.send_transport(DesktopCommand::LoadGroupTimeline(group.id));
                            }
                            None
                        }
                        ServerEventBody::PluginChanged { .. }
                        | ServerEventBody::PluginRemoved { .. } => {
                            self.send_transport(DesktopCommand::LoadPlugins);
                            self.roster.selected
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
                DesktopEvent::GroupTimeline(timeline) => self.group_timeline.hydrate(timeline),
                DesktopEvent::GroupCreated(response) => {
                    let group = response.group;
                    self.group_title_draft.clone_from(&group.title);
                    self.groups.push(group.clone());
                    self.roster.selected = None;
                    self.selected_group = Some(group.id);
                    self.send_transport(DesktopCommand::LoadGroupTimeline(group.id));
                }
                DesktopEvent::GroupMutation(group) => {
                    if let Some(existing) = self.groups.iter_mut().find(|item| item.id == group.id)
                    {
                        *existing = group;
                    } else {
                        self.groups.push(group);
                    }
                }
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
                DesktopEvent::Plugins(plugins) => self.apply_plugins(plugins),
                DesktopEvent::PluginMutation(plugin) => self.apply_plugin(plugin),
                DesktopEvent::PairingOffer(offer) => self.pairing_offer = Some(offer),
                DesktopEvent::DeviceRevoked(device) => self.apply_device_revoked(device),
                DesktopEvent::CheckpointDiff(diff) => self.checkpoint_diff = Some(diff),
                DesktopEvent::Search(response) => self.apply_search(response),
                DesktopEvent::AssistantPacks(packs) => self.assistant_packs = packs,
                DesktopEvent::AssistantPackInstalled(installation) => {
                    self.skills.apply_skill(installation.skill);
                    self.routines.apply_routine(installation.routine);
                    self.assistant_pack_install = None;
                    self.assistant_pack_notice =
                        Some(format!("{} installed and scheduled", installation.pack_id));
                }
                DesktopEvent::Routines(routines) => self.routines.hydrate(routines),
                DesktopEvent::RoutineMutation(routine) => {
                    self.routines.apply_routine(routine);
                    self.routine_editor = None;
                    self.routine_recording = None;
                }
                DesktopEvent::RoutineRecording(recording) => {
                    self.routines.apply_recording(recording.clone());
                    self.routine_recording = Some(recording);
                    self.routine_recording_prompt.clear();
                }
                DesktopEvent::RoutineRuns { routine_id, runs } => {
                    self.routines.apply_runs(routine_id, runs);
                }
                DesktopEvent::RoutineRun(run) => self.routines.apply_run(run),
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

    fn apply_search(&mut self, response: homebot_protocol::GlobalSearchResponse) {
        self.search = Some(SearchProjection {
            query: response.query,
            results: response.results,
        });
    }

    fn apply_devices(&mut self, devices: Vec<homebot_protocol::DeviceSessionSummary>) {
        self.devices = devices;
        self.update_device_count();
    }

    fn apply_connected(&mut self) {
        self.roster.connection = ConnectionState::Connected;
        self.transport_error = None;
        self.send_transport(DesktopCommand::LoadDevices);
        self.send_transport(DesktopCommand::LoadPlugins);
        self.send_transport(DesktopCommand::LoadAssistantPacks);
    }

    fn apply_plugins(&mut self, plugins: Vec<homebot_protocol::PluginSummary>) {
        self.settings.plugins = plugins.into_iter().map(plugin_settings_item).collect();
    }

    fn apply_plugin(&mut self, plugin: homebot_protocol::PluginSummary) {
        let changed = plugin_settings_item(plugin);
        if let Some(existing) = self
            .settings
            .plugins
            .iter_mut()
            .find(|item| item.id == changed.id)
        {
            *existing = changed;
        } else {
            self.settings.plugins.push(changed);
        }
    }

    fn apply_snapshot(&mut self, snapshot: homebot_protocol::Snapshot) {
        self.roster.apply_snapshot(snapshot.bots);
        self.chats = snapshot.chats;
        if self.roster.selected.is_none() {
            self.roster.selected = self
                .chats
                .first()
                .map(|chat| chat.bot_id)
                .or_else(|| self.roster.visible_bots().first().map(|bot| bot.id));
        }
        self.groups = snapshot.group_chats;
        self.skills.hydrate(snapshot.skills);
        self.workspaces
            .hydrate(snapshot.repository_workspaces, snapshot.chat_workspaces);
        self.capability_rules = snapshot.capability_rules;
        self.browser_sessions = snapshot.browser_sessions;
        self.provider_profiles = snapshot.provider_profiles;
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

    fn apply_policy_transport_event(&mut self, event: DesktopEvent) -> Option<DesktopEvent> {
        let DesktopEvent::Server(server_event) = &event else {
            return Some(event);
        };
        match &server_event.body {
            ServerEventBody::CapabilityRuleChanged { rule } => {
                if let Some(existing) = self
                    .capability_rules
                    .iter_mut()
                    .find(|item| item.id == rule.id)
                {
                    *existing = rule.clone();
                } else {
                    self.capability_rules.push(rule.clone());
                }
                None
            }
            ServerEventBody::CapabilityRuleRemoved { rule_id } => {
                self.capability_rules.retain(|rule| rule.id != *rule_id);
                None
            }
            _ => Some(event),
        }
    }

    fn apply_browser_transport_event(&mut self, event: DesktopEvent) -> Option<DesktopEvent> {
        let session = match &event {
            DesktopEvent::Server(server_event) => match &server_event.body {
                ServerEventBody::BrowserSessionChanged { session } => Some(session.clone()),
                _ => None,
            },
            DesktopEvent::BrowserAction(response) => Some(response.session.clone()),
            _ => None,
        };
        let Some(session) = session else {
            return Some(event);
        };
        if let Some(existing) = self
            .browser_sessions
            .iter_mut()
            .find(|item| item.id == session.id)
        {
            *existing = session;
        } else {
            self.browser_sessions.push(session);
        }
        None
    }

    fn flush_transport(&mut self) {
        for command in self.roster.take_commands() {
            self.send_transport(DesktopCommand::Bot(command));
        }
        if let Some(chat_id) = self.selected_group {
            for command in self.group_timeline.take_commands() {
                self.send_transport(DesktopCommand::Group { chat_id, command });
            }
            let _ = self.timeline.take_commands();
            return;
        }
        let _ = self.group_timeline.take_commands();
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

    fn capability_policy_controls(&self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("Computer access policy");
        ui.label("These durable rules are evaluated by the HomeBot server. Deny rules always win; audit history never contains secret values.");
        if self.capability_rules.is_empty() {
            ui.label("No custom rules. Server defaults remain in effect.");
        }
        for rule in &self.capability_rules {
            ui.horizontal_wrapped(|ui| {
                ui.monospace(format!("{:?}", rule.capability));
                ui.strong(format!("{:?}", rule.effect));
                if let Some(prefix) = &rule.action_prefix {
                    ui.label(prefix);
                }
            });
        }
    }

    fn shared_browser_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.strong("Shared browser");
        ui.label("Browser login state stays in an owner-scoped server profile. Watch or take over without copying cookies or credentials into chat.");
        let mut command = None;
        for session in &self.browser_sessions {
            ui.horizontal_wrapped(|ui| {
                ui.strong(&session.profile_name);
                ui.label(format!("{:?} • {:?}", session.status, session.controller));
                if ui.button("Watch").clicked() {
                    command = Some(DesktopCommand::BrowserWatch(session.id));
                }
                if session.controller == homebot_protocol::BrowserController::Bot {
                    if ui.button("Take over").clicked() {
                        command = Some(DesktopCommand::BrowserTakeover {
                            session_id: session.id,
                            approval_id: session.pending_approval_id,
                        });
                    }
                } else if ui.button("Return to Bot").clicked() {
                    command = Some(DesktopCommand::BrowserReturn(session.id));
                }
            });
            if let Some(url) = &session.current_url {
                ui.monospace(url);
            }
        }
        if let Some(command) = command {
            self.send_transport(command);
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
        self.routines_open = false;
        self.search = None;
        if self.transport.is_some() {
            self.send_transport(DesktopCommand::LoadTimeline(deep_link.chat_id));
        }
        self.active_deep_link = Some(deep_link);
    }

    #[allow(clippy::too_many_lines)] // One ordered hierarchy; splitting it obscures bottom pinning.
    fn sidebar(&mut self, context: &egui::Context) {
        let available = context.screen_rect().width();
        let expanded_width = (available * crate::tokens::Layout::SIDEBAR_RATIO).clamp(
            crate::tokens::Layout::SIDEBAR_MIN_WIDTH,
            self.theme.layout.sidebar_width,
        );
        let expanded = context.animate_bool_with_time_and_easing(
            egui::Id::new("sidebar_expanded"),
            !self.sidebar_collapsed,
            f32::from(self.theme.motion.standard_ms) / 1_000.0,
            egui::emath::easing::cubic_out,
        );
        let width = egui::lerp(
            crate::tokens::Layout::SIDEBAR_COLLAPSED_WIDTH..=expanded_width,
            expanded,
        );
        SidePanel::left("bot_roster")
            .resizable(false)
            .min_width(crate::tokens::Layout::SIDEBAR_COLLAPSED_WIDTH)
            .exact_width(width)
            .frame(
                Frame::NONE
                    .fill(self.theme.palette.sidebar)
                    .inner_margin(egui::Margin::same(if expanded > 0.8 {
                        self.theme.insets.md
                    } else {
                        0
                    })),
            )
            .show(context, |ui| {
                if expanded <= 0.8 {
                    return;
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("HomeBot")
                            .font(self.theme.typography.font(self.theme.typography.heading))
                            .strong(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.menu_button("+", |ui| {
                            if ui.button("Create Bot").clicked() {
                                self.roster.begin_create();
                                ui.close();
                            }
                            let bot_ids = self
                                .roster
                                .visible_bots()
                                .into_iter()
                                .take(3)
                                .map(|bot| bot.id)
                                .collect::<Vec<_>>();
                            if ui
                                .add_enabled(bot_ids.len() >= 2, egui::Button::new("Create group"))
                                .clicked()
                            {
                                self.send_transport(DesktopCommand::CreateGroup {
                                    title: "New group".to_owned(),
                                    ownership_bot_id: bot_ids[0],
                                    bot_ids,
                                });
                                ui.close();
                            }
                        });
                    });
                });
                ui.add_space(self.theme.spacing.md);
                if navigation_row(ui, self.theme, "Search", self.search.is_some()).clicked() {
                    self.settings_open = false;
                    self.routines_open = false;
                    self.search.get_or_insert_with(SearchProjection::default);
                }
                ui.add_space(self.theme.spacing.lg);
                let visible: Vec<BotSummary> =
                    self.roster.visible_bots().into_iter().cloned().collect();
                for bot in &visible {
                    let chat = self.chats.iter().find(|chat| chat.bot_id == bot.id);
                    let mut bot_identity = identity(self.theme, bot);
                    bot_identity.role = chat.map_or(bot.title.as_str(), |chat| chat.title.as_str());
                    let metadata = chat.map_or_else(String::new, |chat| {
                        if chat.running {
                            "Working".to_owned()
                        } else if chat.queued_count > 0 {
                            format!("{} queued", chat.queued_count)
                        } else if chat.unread_count > 0 {
                            format!("{} new", chat.unread_count)
                        } else {
                            String::new()
                        }
                    });
                    let response = roster_row(
                        ui,
                        self.theme,
                        bot_identity,
                        &metadata,
                        self.roster.selected == Some(bot.id),
                    );
                    if response.clicked() {
                        self.selected_group = None;
                        self.roster.selected = Some(bot.id);
                        self.load_selected_timeline();
                        if bot.unread_count > 0 {
                            self.roster.queue_mark_read(bot.id);
                        }
                    }
                    response.context_menu(|ui| {
                        if ui.button("Edit Profile").clicked() {
                            self.roster.begin_edit(bot.id);
                            ui.close();
                        }
                        if ui
                            .button(if bot.pinned { "Unpin" } else { "Pin" })
                            .clicked()
                        {
                            self.roster.queue_pin(bot.id, bot.pinned);
                            ui.close();
                        }
                        if ui.button("Duplicate").clicked() {
                            self.roster.queue_duplicate(bot.id);
                            ui.close();
                        }
                        if ui
                            .button(if bot.hidden { "Show" } else { "Hide" })
                            .clicked()
                        {
                            self.roster.queue_hide(bot.id, bot.hidden);
                            ui.close();
                        }
                        if ui
                            .button(if bot.archived { "Restore" } else { "Archive" })
                            .clicked()
                        {
                            self.roster.queue_archive(bot.id, bot.archived);
                            ui.close();
                        }
                    });
                }
                if !self.groups.is_empty() {
                    ui.add_space(self.theme.spacing.sm);
                    for group in self.groups.clone().into_iter().take(2) {
                        let response = recent_conversation_row(
                            ui,
                            self.theme,
                            &format!("Group · {}", group.title),
                            "Group chat",
                            if group.stop_requested {
                                "Stopped"
                            } else {
                                "Active"
                            },
                            self.selected_group == Some(group.id),
                        );
                        if response.clicked() {
                            self.roster.selected = None;
                            self.selected_group = Some(group.id);
                            self.group_title_draft.clone_from(&group.title);
                            self.send_transport(DesktopCommand::LoadGroupTimeline(group.id));
                        }
                        response.context_menu(|ui| {
                            if ui.button("Open group").clicked() {
                                self.roster.selected = None;
                                self.selected_group = Some(group.id);
                                self.send_transport(DesktopCommand::LoadGroupTimeline(group.id));
                                ui.close();
                            }
                            if ui.button("Copy title").clicked() {
                                ui.ctx().copy_text(group.title.clone());
                                ui.close();
                            }
                        });
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    if navigation_row(ui, self.theme, "Account & settings", self.settings_open)
                        .clicked()
                    {
                        self.settings_open = true;
                        self.assistant_packs_open = false;
                        self.routines_open = false;
                        self.search = None;
                    }
                    if navigation_row(
                        ui,
                        self.theme,
                        "Plugins",
                        self.settings_open && self.settings.section == SettingsSection::Plugins,
                    )
                    .clicked()
                    {
                        self.settings_open = true;
                        self.assistant_packs_open = false;
                        self.routines_open = false;
                        self.settings.section = SettingsSection::Plugins;
                        self.search = None;
                    }
                    if navigation_row(ui, self.theme, "Assistant Packs", self.assistant_packs_open)
                        .clicked()
                    {
                        self.settings_open = false;
                        self.assistant_packs_open = true;
                        self.routines_open = false;
                        self.search = None;
                        self.send_transport(DesktopCommand::LoadAssistantPacks);
                    }
                    if navigation_row(ui, self.theme, "Routines", self.routines_open).clicked() {
                        self.settings_open = false;
                        self.assistant_packs_open = false;
                        self.routines_open = true;
                        self.search = None;
                        self.send_transport(DesktopCommand::LoadRoutines);
                    }
                    let _ = ui.checkbox(&mut self.roster.show_archived, "Show archived Bots");
                });
            });
    }

    #[allow(clippy::too_many_lines)] // Contextual menus share the titlebar's right-to-left layout.
    fn titlebar(&mut self, context: &egui::Context) {
        TopBottomPanel::top("bot_titlebar")
            .exact_height(self.theme.layout.titlebar_height)
            .frame(
                Frame::NONE
                    .fill(self.theme.palette.canvas)
                    .stroke(Stroke::new(
                        self.theme.layout.hairline,
                        self.theme.palette.border,
                    ))
                    .inner_margin(egui::Margin::symmetric(
                        self.theme.insets.md,
                        self.theme.insets.sm,
                    )),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    let toggle = ui
                        .button(if self.sidebar_collapsed { "›" } else { "‹" })
                        .on_hover_text(if self.sidebar_collapsed {
                            "Show sidebar (⌘B)"
                        } else {
                            "Hide sidebar (⌘B)"
                        });
                    toggle.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            if self.sidebar_collapsed {
                                "Show sidebar"
                            } else {
                                "Hide sidebar"
                            },
                        )
                    });
                    if toggle.clicked() {
                        self.sidebar_collapsed = !self.sidebar_collapsed;
                    }
                    let selected = self
                        .roster
                        .selected
                        .and_then(|id| self.roster.bots.iter().find(|bot| bot.id == id));
                    let selected_group = self
                        .selected_group
                        .and_then(|id| self.groups.iter().find(|group| group.id == id));
                    ui.label(
                        RichText::new(if self.assistant_packs_open {
                            "Assistant Packs"
                        } else if self.routines_open {
                            "Routines"
                        } else if self.search.is_some() {
                            "Search"
                        } else if let Some(group) = selected_group {
                            group.title.as_str()
                        } else {
                            selected.map_or("Bots", |bot| bot.name.as_str())
                        })
                        .strong(),
                    );
                    if self.assistant_packs_open {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Done").clicked() {
                                self.assistant_packs_open = false;
                            }
                        });
                        return;
                    }
                    if self.routines_open {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Done").clicked() {
                                self.routines_open = false;
                            }
                        });
                        return;
                    }
                    if self.search.is_some() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Done").clicked() {
                                self.search = None;
                            }
                        });
                        return;
                    }
                    if let Some(bot) = selected.cloned() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .button("Computer")
                                .on_hover_text("Computer and details")
                                .clicked()
                            {
                                self.details_open = !self.details_open;
                            }
                            ui.menu_button("•••", |ui| {
                                if ui.button("Edit Bot").clicked() {
                                    self.roster.begin_edit(bot.id);
                                    ui.close();
                                }
                                if ui
                                    .button(if bot.pinned { "Unpin" } else { "Pin" })
                                    .clicked()
                                {
                                    self.roster.queue_pin(bot.id, bot.pinned);
                                    ui.close();
                                }
                                if ui.button("Duplicate").clicked() {
                                    self.roster.queue_duplicate(bot.id);
                                    ui.close();
                                }
                                if ui.button("Hide").clicked() {
                                    self.roster.queue_hide(bot.id, bot.hidden);
                                    ui.close();
                                }
                                if ui
                                    .button(if bot.archived { "Restore" } else { "Archive" })
                                    .clicked()
                                {
                                    self.roster.queue_archive(bot.id, bot.archived);
                                    ui.close();
                                }
                                if ui.button("Delete…").clicked() {
                                    self.delete_confirmation =
                                        Some((bot.id, bot.name.clone(), String::new()));
                                    ui.close();
                                }
                            });
                        });
                    } else if let Some(group) = selected_group.cloned() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .button("Computer")
                                .on_hover_text("Computer and details")
                                .clicked()
                            {
                                self.details_open = !self.details_open;
                            }
                            ui.menu_button("•••", |ui| {
                                if !group.stop_requested && ui.button("Stop group").clicked() {
                                    self.group_timeline.stop();
                                    ui.close();
                                }
                            });
                        });
                    }
                });
            });
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        if self.roster.connection == ConnectionState::Connected && self.selected_group.is_some() {
            self.group_content(ui);
            return;
        }
        match self.roster.connection {
            ConnectionState::Connected if !self.roster.visible_bots().is_empty() => {
                if let Some(bot) = self
                    .roster
                    .selected
                    .and_then(|id| self.roster.bots.iter().find(|bot| bot.id == id))
                    .cloned()
                {
                    self.timeline_content(ui, &bot);
                } else {
                    self.empty_state(ui, "Choose a Bot", "Pick a teammate from the sidebar.");
                }
            }
            state => self.empty_state_for_connection(ui, state),
        }
    }

    #[allow(clippy::too_many_lines)] // Group status, details and transcript are one scroll surface.
    fn group_content(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.set_max_width(self.theme.layout.content_max_width);
            let participant_names = self
                .group_timeline
                .participants
                .iter()
                .filter_map(|participant| {
                    self.roster
                        .bots
                        .iter()
                        .find(|bot| bot.id == participant.bot_id)
                        .map(|bot| format!("{} · {:?}", bot.name, participant.status))
                })
                .collect::<Vec<_>>();
            ui.label(
                RichText::new(participant_names.join("   "))
                    .font(self.theme.typography.font(self.theme.typography.caption))
                    .color(self.theme.palette.text_secondary),
            );
            if self.details_open {
                let group = self.group_timeline.group.clone();
                let participants = self.group_timeline.participants.clone();
                Frame::NONE
                    .fill(self.theme.palette.surface)
                    .corner_radius(CornerRadius::same(self.theme.radii.md))
                    .inner_margin(egui::Margin::same(self.theme.insets.md))
                    .show(ui, |ui| {
                        ui.strong("Group coordination");
                        if let Some(group) = &group {
                            ui.label(format!(
                                "{} of {} coordination turns · up to {} Bots in parallel",
                                group.coordination_turns_used,
                                group.coordination_max_turns,
                                group.max_parallel_bots
                            ));
                            ui.horizontal(|ui| {
                                ui.text_edit_singleline(&mut self.group_title_draft);
                                if ui.button("Rename").clicked() {
                                    self.group_timeline.rename(&self.group_title_draft);
                                }
                            });
                            for participant in &participants {
                                let name = self
                                    .roster
                                    .bots
                                    .iter()
                                    .find(|bot| bot.id == participant.bot_id)
                                    .map_or("Bot", |bot| bot.name.as_str());
                                ui.horizontal(|ui| {
                                    ui.label(format!("{name} · {:?}", participant.status));
                                    if participant.bot_id != group.ownership_bot_id
                                        && ui.small_button("Hand off").clicked()
                                    {
                                        self.group_timeline.handoff(
                                            participant.bot_id,
                                            self.group_timeline.messages.last().map(|item| item.id),
                                            "User requested ownership handoff",
                                        );
                                    }
                                    if participant.bot_id != group.ownership_bot_id
                                        && participants.len() > 2
                                        && ui.small_button("Remove").clicked()
                                    {
                                        self.group_timeline.remove_participant(participant.bot_id);
                                    }
                                });
                            }
                            if let Some(bot) = self.roster.bots.iter().find(|bot| {
                                !participants
                                    .iter()
                                    .any(|participant| participant.bot_id == bot.id)
                            }) && ui.button(format!("Add {}", bot.name)).clicked()
                            {
                                self.group_timeline.add_participant(bot.id);
                            }
                        }
                    });
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for item in self.group_timeline.messages.clone() {
                    let text =
                        item.parts
                            .iter()
                            .filter_map(|part| match part {
                                MessagePart::Text { text, .. }
                                | MessagePart::Notice { text, .. } => Some(text.as_str()),
                                MessagePart::Attachment { .. } => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                    let bot = item
                        .author_bot_id
                        .and_then(|id| self.roster.bots.iter().find(|bot| bot.id == id));
                    let response = message(
                        ui,
                        self.theme,
                        bot.map(|bot| identity(self.theme, bot)),
                        &text,
                    );
                    response.context_menu(|ui| {
                        if ui.button("Copy message").clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close();
                        }
                    });
                    ui.add_space(self.theme.spacing.md);
                }
                for handoff in &self.group_timeline.handoffs {
                    let from = self
                        .roster
                        .bots
                        .iter()
                        .find(|bot| bot.id == handoff.from_bot_id)
                        .map_or("Bot", |bot| bot.name.as_str());
                    let to = self
                        .roster
                        .bots
                        .iter()
                        .find(|bot| bot.id == handoff.to_bot_id)
                        .map_or("Bot", |bot| bot.name.as_str());
                    activity_card(
                        ui,
                        self.theme,
                        &format!("{from} handed work to {to}"),
                        &handoff.reason,
                        false,
                    );
                }
            });
        });
    }

    fn empty_state_for_connection(&mut self, ui: &mut egui::Ui, state: ConnectionState) {
        ui.vertical_centered(|ui| {
            ui.add_space(self.theme.layout.empty_state_top_padding);
            match state {
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
                ConnectionState::Connected => {}
            }
        });
    }

    fn empty_state(&self, ui: &mut egui::Ui, title: &str, detail: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(self.theme.layout.empty_state_top_padding);
            ui.heading(title);
            ui.label(detail);
        });
    }

    fn search_content(&mut self, ui: &mut egui::Ui) {
        let search = self.search.get_or_insert_with(SearchProjection::default);
        ui.set_max_width(self.theme.layout.content_max_width);
        ui.heading("Search HomeBot");
        ui.label("Find messages, files, links and routines on your server.");
        let mut submit = false;
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut search.query)
                    .hint_text("Search HomeBot")
                    .desired_width(f32::INFINITY),
            );
            submit = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            submit |= ui.button("Search").clicked();
        });
        let submitted_query = submit.then(|| search.query.trim().to_owned());
        let query_is_empty = search.query.is_empty();
        let results = search.results.clone();
        if let Some(query) = submitted_query.filter(|query| !query.is_empty()) {
            self.send_transport(DesktopCommand::Search(query));
        }
        ui.add_space(self.theme.spacing.lg);
        if !query_is_empty && results.is_empty() {
            ui.label("No matching results.");
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for result in results {
                let kind = format!("{:?}", result.kind);
                if ui
                    .button(format!("{}  ·  {kind}\n{}", result.title, result.snippet))
                    .clicked()
                {
                    if let Some(chat_id) = result.chat_id {
                        let bot_id = self
                            .chats
                            .iter()
                            .find(|chat| chat.id == chat_id)
                            .map(|chat| chat.bot_id);
                        self.open_deep_link(DeepLink {
                            bot_id,
                            chat_id,
                            message_id: result.message_id,
                            activity_id: None,
                        });
                    } else if result.routine_id.is_some() {
                        self.search = None;
                        self.settings_open = false;
                        self.routines_open = true;
                        self.send_transport(DesktopCommand::LoadRoutines);
                    }
                }
            }
        });
    }

    #[allow(clippy::too_many_lines)] // Catalog cards and one contextual install form share one surface.
    fn assistant_pack_content(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.set_max_width(self.theme.layout.content_max_width);
            ui.add_space(self.theme.spacing.xl);
            ui.heading("Assistant Packs");
            ui.label("Install a useful Skill and scheduled routine onto one Bot.");
            if let Some(notice) = &self.assistant_pack_notice {
                ui.colored_label(self.theme.palette.success, notice);
            }
            ui.add_space(self.theme.spacing.lg);
            if self.assistant_packs.is_empty() {
                ui.label("No Assistant Packs are available.");
            }
            for pack in self.assistant_packs.clone() {
                let mut configure = false;
                Frame::NONE
                    .fill(self.theme.palette.surface)
                    .stroke(Stroke::new(
                        self.theme.layout.hairline,
                        self.theme.palette.border,
                    ))
                    .corner_radius(CornerRadius::same(self.theme.radii.md))
                    .inner_margin(egui::Margin::same(self.theme.insets.lg))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.strong(&pack.name);
                                let cadence = match pack.schedule.cadence {
                                    homebot_protocol::AssistantPackCadence::Daily => "Daily",
                                    homebot_protocol::AssistantPackCadence::Weekly => "Weekly",
                                };
                                ui.small(format!(
                                    "{cadence} · default {:02}:{:02}",
                                    pack.schedule.default_hour, pack.schedule.default_minute
                                ));
                                ui.label(&pack.description);
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                configure = ui.button("Configure").clicked();
                            });
                        });
                    });
                ui.add_space(self.theme.spacing.sm);
                if configure {
                    let bot_id = self
                        .roster
                        .selected
                        .filter(|selected| {
                            self.roster
                                .bots
                                .iter()
                                .any(|bot| bot.id == *selected && !bot.archived)
                        })
                        .or_else(|| {
                            self.roster
                                .bots
                                .iter()
                                .find(|bot| !bot.archived)
                                .map(|bot| bot.id)
                        });
                    if let Some(bot_id) = bot_id {
                        self.assistant_pack_install = Some(AssistantPackInstallDraft {
                            pack_id: pack.id,
                            bot_id,
                            timezone: "UTC".to_owned(),
                            hour: pack.schedule.default_hour,
                            minute: pack.schedule.default_minute,
                        });
                        self.assistant_pack_notice = None;
                    } else {
                        self.assistant_pack_notice =
                            Some("Create a Bot before installing an Assistant Pack.".to_owned());
                    }
                }
            }

            let Some(mut draft) = self.assistant_pack_install.take() else {
                return;
            };
            ui.separator();
            let pack_name = self
                .assistant_packs
                .iter()
                .find(|pack| pack.id == draft.pack_id)
                .map_or(draft.pack_id.as_str(), |pack| pack.name.as_str());
            ui.heading(format!("Configure {pack_name}"));
            egui::ComboBox::from_id_salt("assistant_pack_bot")
                .selected_text(
                    self.roster
                        .bots
                        .iter()
                        .find(|bot| bot.id == draft.bot_id)
                        .map_or("Choose Bot", |bot| bot.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for bot in self.roster.bots.iter().filter(|bot| !bot.archived) {
                        ui.selectable_value(&mut draft.bot_id, bot.id, &bot.name);
                    }
                });
            ui.horizontal(|ui| {
                ui.label("Timezone");
                ui.text_edit_singleline(&mut draft.timezone);
            });
            ui.horizontal(|ui| {
                ui.label("Run at");
                ui.add(egui::DragValue::new(&mut draft.hour).range(0..=23));
                ui.label(":");
                ui.add(egui::DragValue::new(&mut draft.minute).range(0..=59));
            });
            let mut keep_draft = true;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !draft.timezone.trim().is_empty(),
                        egui::Button::new("Install and enable"),
                    )
                    .clicked()
                {
                    self.send_transport(DesktopCommand::InstallAssistantPack {
                        pack_id: draft.pack_id.clone(),
                        bot_id: draft.bot_id,
                        timezone: draft.timezone.trim().to_owned(),
                        hour: draft.hour,
                        minute: draft.minute,
                    });
                    self.assistant_pack_notice = Some("Installing Assistant Pack…".to_owned());
                    keep_draft = false;
                }
                if ui.button("Cancel").clicked() {
                    keep_draft = false;
                }
            });
            if keep_draft {
                self.assistant_pack_install = Some(draft);
            }
        });
    }

    #[allow(clippy::too_many_lines)] // List, contextual creation and server actions share one surface.
    fn routine_content(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.set_max_width(self.theme.layout.content_max_width);
            ui.add_space(self.theme.spacing.xl);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Routines");
                    ui.label("Repeat useful work without keeping a client open.");
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let selected_bot = self.roster.selected;
                    if ui
                        .add_enabled(selected_bot.is_some(), egui::Button::new("Teach routine"))
                        .clicked()
                        && let Some(bot_id) = selected_bot
                    {
                        self.send_transport(DesktopCommand::StartRoutineRecording {
                            bot_id,
                            name: "New demonstrated routine".to_owned(),
                            description: String::new(),
                        });
                    }
                    if ui
                        .add_enabled(selected_bot.is_some(), egui::Button::new("New routine"))
                        .clicked()
                        && let Some(bot_id) = selected_bot
                    {
                        self.routine_editor = Some(RoutineEditorDraft {
                            routine_id: None,
                            bot_id,
                            name: "New routine".to_owned(),
                            description: String::new(),
                            definition: RoutineDefinition {
                                inputs: Vec::new(),
                                steps: vec![RoutineStep::BotPrompt {
                                    bot_id,
                                    prompt_template: String::new(),
                                    requires_approval: true,
                                }],
                                expected_outputs: Vec::new(),
                            },
                            draft: true,
                        });
                    }
                });
            });
            ui.add_space(self.theme.spacing.lg);

            if self.routine_recording.is_some() {
                self.routine_recording_content(ui);
                return;
            }
            if self.routine_editor.is_some() {
                self.routine_editor_content(ui);
                return;
            }

            let routines = self.routines.routines().cloned().collect::<Vec<_>>();
            if routines.is_empty() {
                Frame::NONE
                    .fill(self.theme.palette.surface)
                    .corner_radius(CornerRadius::same(self.theme.radii.lg))
                    .inner_margin(egui::Margin::same(self.theme.insets.xl))
                    .show(ui, |ui| {
                        ui.strong("No routines yet");
                        ui.label("Teach a Bot a workflow, then edit and run it here.");
                    });
            }
            for routine in routines {
                Frame::NONE
                    .fill(self.theme.palette.surface)
                    .stroke(Stroke::new(
                        self.theme.layout.hairline,
                        self.theme.palette.border,
                    ))
                    .corner_radius(CornerRadius::same(self.theme.radii.md))
                    .inner_margin(egui::Margin::same(self.theme.insets.lg))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.strong(&routine.name);
                                ui.label(&routine.description);
                                ui.small(format!(
                                    "Version {} · {} structured steps",
                                    routine.version,
                                    routine.definition.steps.len()
                                ));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Run now").clicked() {
                                    self.send_transport(DesktopCommand::RunRoutine {
                                        routine_id: routine.id,
                                        dry_run: false,
                                    });
                                }
                                if ui.button("Dry run").clicked() {
                                    self.send_transport(DesktopCommand::RunRoutine {
                                        routine_id: routine.id,
                                        dry_run: true,
                                    });
                                }
                                if ui
                                    .button(if routine.enabled { "Pause" } else { "Enable" })
                                    .clicked()
                                {
                                    self.send_transport(DesktopCommand::SetRoutineEnabled {
                                        routine_id: routine.id,
                                        enabled: !routine.enabled,
                                    });
                                }
                                if ui.button("Duplicate").clicked() {
                                    self.send_transport(DesktopCommand::DuplicateRoutine {
                                        routine_id: routine.id,
                                        name: format!("{} copy", routine.name),
                                    });
                                }
                                if ui.button("Edit").clicked() {
                                    self.routine_editor =
                                        Some(RoutineEditorDraft::from_summary(&routine));
                                }
                            });
                        });
                        for run in self.routines.runs(routine.id).iter().take(3) {
                            ui.label(format!(
                                "{} · {}{}",
                                run.status,
                                run.attempt_count,
                                if run.dry_run { " · dry run" } else { "" }
                            ));
                        }
                        if ui.small_button("Refresh run history").clicked() {
                            self.send_transport(DesktopCommand::LoadRoutineRuns(routine.id));
                        }
                    });
                ui.add_space(self.theme.spacing.sm);
            }
        });
    }

    fn routine_editor_content(&mut self, ui: &mut egui::Ui) {
        let mut save = false;
        let mut cancel = false;
        let mut remove_step = None;
        let Some(editor) = self.routine_editor.as_mut() else {
            return;
        };
        Frame::NONE
            .fill(self.theme.palette.surface)
            .stroke(Stroke::new(
                self.theme.layout.hairline,
                self.theme.palette.border,
            ))
            .corner_radius(CornerRadius::same(self.theme.radii.lg))
            .inner_margin(egui::Margin::same(self.theme.insets.xl))
            .show(ui, |ui| {
                ui.strong(if editor.routine_id.is_some() {
                    "Edit routine"
                } else {
                    "Create routine"
                });
                ui.text_edit_singleline(&mut editor.name);
                ui.text_edit_multiline(&mut editor.description);
                ui.checkbox(&mut editor.draft, "Keep as draft");
                ui.separator();
                ui.label("Structured steps");
                for (index, step) in editor.definition.steps.iter_mut().enumerate() {
                    Frame::NONE
                        .fill(self.theme.palette.surface_hover)
                        .corner_radius(CornerRadius::same(self.theme.radii.sm))
                        .inner_margin(egui::Margin::same(self.theme.insets.md))
                        .show(ui, |ui| match step {
                            RoutineStep::BotPrompt {
                                prompt_template,
                                requires_approval,
                                ..
                            } => {
                                ui.label(format!("Bot prompt {}", index + 1));
                                ui.text_edit_multiline(prompt_template);
                                ui.checkbox(requires_approval, "Require approval");
                                if ui.small_button("Remove step").clicked() {
                                    remove_step = Some(index);
                                }
                            }
                            RoutineStep::PluginTool { tool_name, .. } => {
                                ui.label(format!("Plugin tool: {tool_name}"));
                            }
                            RoutineStep::RecordOutput { output_key, .. } => {
                                ui.label(format!("Record output: {output_key}"));
                            }
                        });
                    ui.add_space(self.theme.spacing.sm);
                }
                if ui.button("Add Bot prompt").clicked() {
                    editor.definition.steps.push(RoutineStep::BotPrompt {
                        bot_id: editor.bot_id,
                        prompt_template: String::new(),
                        requires_approval: true,
                    });
                }
                ui.horizontal(|ui| {
                    save = ui
                        .add_enabled(
                            !editor.name.trim().is_empty() && !editor.definition.steps.is_empty(),
                            egui::Button::new("Save"),
                        )
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if let Some(index) = remove_step {
            editor.definition.steps.remove(index);
        }
        if cancel {
            self.routine_editor = None;
        } else if save && let Some(editor) = self.routine_editor.clone() {
            let command = if let Some(routine_id) = editor.routine_id {
                DesktopCommand::UpdateRoutine {
                    routine_id,
                    name: editor.name,
                    description: editor.description,
                    definition: editor.definition,
                    draft: editor.draft,
                }
            } else {
                DesktopCommand::CreateRoutine {
                    bot_id: editor.bot_id,
                    name: editor.name,
                    description: editor.description,
                    definition: editor.definition,
                    draft: editor.draft,
                }
            };
            self.send_transport(command);
        }
    }

    fn routine_recording_content(&mut self, ui: &mut egui::Ui) {
        let Some(recording) = self.routine_recording.clone() else {
            return;
        };
        Frame::NONE
            .fill(self.theme.palette.surface)
            .stroke(Stroke::new(
                self.theme.layout.hairline,
                self.theme.palette.warning,
            ))
            .corner_radius(CornerRadius::same(self.theme.radii.lg))
            .inner_margin(egui::Margin::same(self.theme.insets.xl))
            .show(ui, |ui| {
                ui.strong(format!("Teaching {}", recording.name));
                ui.label(format!("{} recorded actions", recording.actions.len()));
                ui.text_edit_multiline(&mut self.routine_recording_prompt);
                ui.checkbox(
                    &mut self.routine_recording_requires_approval,
                    "Preserve an approval boundary",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.routine_recording_prompt.trim().is_empty(),
                            egui::Button::new("Record Bot prompt"),
                        )
                        .clicked()
                    {
                        self.send_transport(DesktopCommand::AppendRoutineRecording {
                            recording_id: recording.id,
                            action: RecordedAction {
                                actor: RecordedActor::User,
                                step: RoutineStep::BotPrompt {
                                    bot_id: recording.bot_id,
                                    prompt_template: self.routine_recording_prompt.clone(),
                                    requires_approval: self.routine_recording_requires_approval,
                                },
                            },
                        });
                    }
                    if ui
                        .add_enabled(
                            !recording.actions.is_empty(),
                            egui::Button::new("Finish and edit"),
                        )
                        .clicked()
                    {
                        self.send_transport(DesktopCommand::FinishRoutineRecording(recording.id));
                    }
                    if ui.button("Cancel recording").clicked() {
                        self.send_transport(DesktopCommand::CancelRoutineRecording(recording.id));
                        self.routine_recording = None;
                    }
                });
            });
    }

    #[allow(clippy::too_many_lines)] // Message, activity and approval ordering must remain explicit.
    fn timeline_content(&mut self, ui: &mut egui::Ui, bot: &BotSummary) {
        ui.vertical_centered(|ui| {
            ui.set_max_width(self.theme.layout.content_max_width);
            if bot.provider == BotProviderStatus::Unavailable {
                Frame::NONE
                    .fill(self.theme.palette.accent_soft)
                    .corner_radius(CornerRadius::same(self.theme.radii.md))
                    .inner_margin(egui::Margin::same(self.theme.insets.md))
                    .show(ui, |ui| {
                        ui.colored_label(
                            self.theme.palette.warning,
                            "Provider unavailable · choose another in Bot settings.",
                        );
                    });
            }
            if self.details_open {
                Frame::NONE
                    .fill(self.theme.palette.surface)
                    .stroke(Stroke::new(
                        self.theme.layout.hairline,
                        self.theme.palette.border,
                    ))
                    .corner_radius(CornerRadius::same(self.theme.radii.md))
                    .inner_margin(egui::Margin::same(self.theme.insets.md))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.strong("Computer & coding details");
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("Close").clicked() {
                                    self.details_open = false;
                                }
                            });
                        });
                        self.workspace_controls(ui);
                        self.checkpoint_controls(ui);
                        self.working_context_controls(ui);
                    });
                ui.add_space(self.theme.spacing.md);
            }
            let scroll = egui::ScrollArea::vertical()
                .id_salt("direct_chat_timeline")
                .stick_to_bottom(self.timeline.scroll.at_bottom)
                .show(ui, |ui| {
                    for entry in self.timeline.ordered_entries() {
                        match entry {
                            TimelineEntry::Message(item) => {
                                let text = message_text(&item);
                                let author = (item.author == MessageAuthor::Bot)
                                    .then(|| identity(self.theme, bot));
                                let response = message(ui, self.theme, author, &text);
                                response.context_menu(|ui| {
                                    if ui.button("Copy message").clicked() {
                                        ui.ctx().copy_text(text.clone());
                                        ui.close();
                                    }
                                    if ui.button("Reply").clicked() {
                                        self.timeline.begin_reply(item.id);
                                        ui.close();
                                    }
                                    if ui.button("React 👍").clicked() {
                                        let active = !item.reactions.iter().any(|reaction| {
                                            reaction.emoji == "👍" && reaction.reacted_by_user
                                        });
                                        self.timeline.set_reaction(item.id, "👍", active);
                                        ui.close();
                                    }
                                    if item.status == MessageStatus::Failed
                                        && ui.button("Retry").clicked()
                                    {
                                        self.timeline.retry(item.id);
                                        ui.close();
                                    }
                                });
                                message_reference_labels(ui, &item);
                                if !item.reactions.is_empty() {
                                    ui.horizontal(|ui| {
                                        for reaction in &item.reactions {
                                            if ui
                                                .small_button(format!(
                                                    "{} {}",
                                                    reaction.emoji, reaction.count
                                                ))
                                                .clicked()
                                            {
                                                self.timeline.set_reaction(
                                                    item.id,
                                                    reaction.emoji.clone(),
                                                    !reaction.reacted_by_user,
                                                );
                                            }
                                        }
                                    });
                                }
                                ui.add_space(self.theme.spacing.md);
                            }
                            TimelineEntry::Activity(item) => {
                                let mut model = ActivityCardModel::new(item);
                                model.expanded = self.expanded_activities.contains(&model.activity.id);
                                for action in activity_surface(ui, self.theme, &mut model) {
                                    match action {
                                        ActivityAction::Copy(text) => ui.ctx().copy_text(text),
                                        ActivityAction::OpenArtifact(_) => {
                                            self.transport_error = Some(
                                                "Artifact preview is not available in this desktop build."
                                                    .to_owned(),
                                            );
                                        }
                                        ActivityAction::ReviewApproval => {
                                            self.transport_error = Some(
                                                "Review the approval request in this conversation."
                                                    .to_owned(),
                                            );
                                        }
                                    }
                                }
                                if model.expanded {
                                    self.expanded_activities.insert(model.activity.id);
                                } else {
                                    self.expanded_activities.remove(&model.activity.id);
                                }
                                ui.add_space(self.theme.spacing.sm);
                            }
                            TimelineEntry::Approval(approval)
                                if approval.status == ApprovalStatus::Pending =>
                            {
                                Frame::NONE
                                    .fill(self.theme.palette.accent_soft)
                                    .stroke(Stroke::new(
                                        self.theme.layout.hairline,
                                        self.theme.palette.warning,
                                    ))
                                    .corner_radius(CornerRadius::same(self.theme.radii.md))
                                    .inner_margin(egui::Margin::same(self.theme.insets.md))
                                    .show(ui, |ui| {
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
                                ui.add_space(self.theme.spacing.sm);
                            }
                            TimelineEntry::Approval(_) => {}
                        }
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
            let max_offset = (scroll.content_size.y - scroll.inner_rect.height()).max(0.0);
            let at_bottom = scroll.state.offset.y >= max_offset - 2.0;
            self.timeline.set_at_bottom(at_bottom);
            if self.timeline.scroll.unseen_updates > 0 {
                let button_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        scroll.inner_rect.right() - 150.0,
                        scroll.inner_rect.bottom() - 38.0,
                    ),
                    egui::vec2(142.0, 30.0),
                );
                if ui
                    .put(
                        button_rect,
                        egui::Button::new(format!(
                            "{} new · Jump to latest",
                            self.timeline.scroll.unseen_updates
                        )),
                    )
                    .clicked()
                {
                    self.timeline.set_at_bottom(true);
                }
            }
        });
    }

    fn should_show_composer(&self) -> bool {
        !self.assistant_packs_open
            && !self.routines_open
            && self.search.is_none()
            && self.roster.connection == ConnectionState::Connected
            && (self.roster.selected.is_some() || self.selected_group.is_some())
    }

    fn composer_panel(&mut self, context: &egui::Context) {
        TopBottomPanel::bottom("homebot_composer")
            .frame(Frame::NONE.fill(self.theme.palette.canvas).inner_margin(
                egui::Margin::symmetric(self.theme.insets.xl, self.theme.insets.md),
            ))
            .show(context, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(self.theme.layout.composer_max_width);
                    if self.selected_group.is_some() {
                        self.group_composer_controls(ui);
                    } else {
                        self.composer_controls(ui);
                    }
                });
            });
    }

    fn group_composer_controls(&mut self, ui: &mut egui::Ui) {
        Frame::NONE
            .fill(self.theme.palette.surface)
            .stroke(Stroke::new(
                self.theme.layout.hairline,
                self.theme.palette.border,
            ))
            .corner_radius(CornerRadius::same(self.theme.radii.composer))
            .shadow(self.theme.panel_shadow)
            .inner_margin(egui::Margin::symmetric(
                self.theme.insets.md,
                self.theme.insets.sm,
            ))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.menu_button("+", |ui| {
                        ui.label("Mention a Bot");
                        for participant in self.group_timeline.participants.clone() {
                            let Some(bot) = self
                                .roster
                                .bots
                                .iter()
                                .find(|bot| bot.id == participant.bot_id)
                            else {
                                continue;
                            };
                            let mut selected = self
                                .group_timeline
                                .composer
                                .mentioned_bot_ids
                                .contains(&bot.id);
                            if ui.checkbox(&mut selected, &bot.name).changed() {
                                if selected {
                                    self.group_timeline.composer.mentioned_bot_ids.push(bot.id);
                                } else {
                                    self.group_timeline
                                        .composer
                                        .mentioned_bot_ids
                                        .retain(|id| *id != bot.id);
                                }
                            }
                        }
                    });
                    ui.add_sized(
                        [
                            ui.available_width() - crate::tokens::Layout::COMPOSER_ACTION_RESERVE,
                            self.theme.layout.composer_editor_height,
                        ],
                        egui::TextEdit::multiline(&mut self.group_timeline.composer.content)
                            .hint_text("Message the group")
                            .frame(false),
                    );
                    let can_send = !self.group_timeline.composer.content.trim().is_empty();
                    if send_button(ui, self.theme, can_send)
                        .on_hover_text("Send message")
                        .clicked()
                        && let Err(error) = self.group_timeline.submit()
                    {
                        self.transport_error = Some(
                            match error {
                                GroupComposerError::EmptyComposer => "Write a message first.",
                                GroupComposerError::UnknownMention => {
                                    "Every mentioned Bot must belong to this group."
                                }
                            }
                            .to_owned(),
                        );
                    }
                });
            });
    }

    fn composer_controls(&mut self, ui: &mut egui::Ui) {
        if let Some(message_id) = self.timeline.composer.reply_to_message_id {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Replying to {}",
                    message_id
                        .simple()
                        .to_string()
                        .chars()
                        .take(8)
                        .collect::<String>()
                ));
                if ui.small_button("Cancel").clicked() {
                    self.timeline.cancel_reply();
                }
            });
        }
        let running = self.timeline.chat.as_ref().is_some_and(|chat| chat.running);
        let composer_hint = self
            .roster
            .selected
            .and_then(|id| self.roster.bots.iter().find(|bot| bot.id == id))
            .map_or_else(
                || "Message a Bot".to_owned(),
                |bot| format!("Message {}", bot.name),
            );
        if self.composer_error == Some(ComposerError::EmptyComposer) {
            let message = "Write a message or attach a file first.";
            ui.colored_label(self.theme.palette.danger, message);
        }
        Frame::NONE
            .fill(self.theme.palette.surface)
            .stroke(Stroke::new(
                self.theme.layout.hairline,
                self.theme.palette.border,
            ))
            .corner_radius(CornerRadius::same(self.theme.radii.composer))
            .shadow(self.theme.panel_shadow)
            .inner_margin(egui::Margin::symmetric(
                self.theme.insets.md,
                self.theme.insets.sm,
            ))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.menu_button("+", |ui| {
                        ui.label("Drop up to six files anywhere in HomeBot to attach them.");
                        if !self.timeline.composer.attachment_ids.is_empty() {
                            ui.label(format!(
                                "{} attached",
                                self.timeline.composer.attachment_ids.len()
                            ));
                        }
                    });
                    let composer = ui.add_sized(
                        [
                            ui.available_width() - crate::tokens::Layout::COMPOSER_ACTION_RESERVE,
                            self.theme.layout.composer_editor_height,
                        ],
                        egui::TextEdit::multiline(&mut self.timeline.composer.content)
                            .hint_text(composer_hint)
                            .frame(false),
                    );
                    if self.focus_composer {
                        composer.request_focus();
                        self.focus_composer = false;
                    }
                    if running {
                        ui.menu_button("●", |ui| {
                            if ui.button("Queue follow-up").clicked() {
                                self.composer_error = self.timeline.submit(false).err();
                                ui.close();
                            }
                            if ui.button("Steer current work").clicked() {
                                self.composer_error = self.timeline.submit(true).err();
                                ui.close();
                            }
                            if ui.button("Stop Bot").clicked() {
                                self.timeline.stop();
                                ui.close();
                            }
                        });
                    } else {
                        let can_send = !self.timeline.composer.content.trim().is_empty()
                            || !self.timeline.composer.attachment_ids.is_empty();
                        if send_button(ui, self.theme, can_send)
                            .on_hover_text("Send message")
                            .clicked()
                        {
                            self.composer_error = self.timeline.submit(false).err();
                        }
                    }
                });
            });
        if self.timeline.scroll.unseen_updates > 0 {
            ui.horizontal_centered(|ui| {
                if ui.button("Jump to latest").clicked() {
                    self.timeline.set_at_bottom(true);
                }
            });
        }
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
                let selected = draft
                    .provider_profile_id
                    .and_then(|id| {
                        self.provider_profiles
                            .iter()
                            .find(|profile| profile.id == id)
                    })
                    .map_or("Not configured", |profile| profile.display_name.as_str());
                egui::ComboBox::from_label("Provider")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut draft.provider_profile_id, None, "Not configured");
                        for profile in &self.provider_profiles {
                            ui.selectable_value(
                                &mut draft.provider_profile_id,
                                Some(profile.id),
                                format!("{} · {}", profile.display_name, profile.availability),
                            );
                        }
                    });
                if let Some(profile) = draft.provider_profile_id.and_then(|id| {
                    self.provider_profiles
                        .iter()
                        .find(|profile| profile.id == id)
                }) {
                    ui.small(&profile.status_message);
                }
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

fn message_text(message: &MessageSummary) -> String {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } | MessagePart::Notice { text, .. } => {
                Some(text.as_str())
            }
            MessagePart::Attachment { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn plugin_settings_item(plugin: homebot_protocol::PluginSummary) -> PluginSettingsItem {
    PluginSettingsItem {
        id: Some(plugin.id),
        name: plugin.name,
        detail: plugin.error_message.unwrap_or(plugin.description),
        state: match plugin.connection_state {
            homebot_protocol::PluginConnectionState::Connect => PluginViewState::Connect,
            homebot_protocol::PluginConnectionState::Waiting => PluginViewState::Waiting,
            homebot_protocol::PluginConnectionState::Reopen => PluginViewState::Reopen,
            homebot_protocol::PluginConnectionState::Connected => PluginViewState::Connected,
            homebot_protocol::PluginConnectionState::Error => PluginViewState::Error,
        },
        enabled: plugin.enabled,
    }
}

fn set_launch_at_login(enabled: bool) -> std::io::Result<()> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "home directory"))?;
    let executable = std::env::current_exe()?;

    #[cfg(target_os = "macos")]
    let (path, contents) = {
        let path = home
            .join("Library/LaunchAgents")
            .join("dev.homebot.desktop.plist");
        let executable = xml_escape(&executable.to_string_lossy());
        let contents = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>dev.homebot.desktop</string><key>ProgramArguments</key><array><string>{executable}</string></array><key>RunAtLoad</key><true/></dict></plist>\n"
        );
        (path, contents)
    };

    #[cfg(not(target_os = "macos"))]
    let (path, contents) = {
        let path = std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), std::path::PathBuf::from)
            .join("autostart/dev.homebot.desktop.desktop");
        let executable = executable
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let contents = format!(
            "[Desktop Entry]\nType=Application\nName=HomeBot\nExec=\"{executable}\"\nTerminal=false\n"
        );
        (path, contents)
    };

    if enabled {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn message_reference_labels(ui: &mut egui::Ui, message: &homebot_protocol::MessageSummary) {
    let labels = message
        .references
        .iter()
        .map(|reference| format!("@{}", reference.label))
        .chain(
            message
                .applied_skills
                .iter()
                .map(|skill| format!("/{} v{}", skill.name, skill.version)),
        )
        .collect::<Vec<_>>();
    if !labels.is_empty() {
        ui.label(labels.join("  "));
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

/// Deterministic visual states rendered through the real production app shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductionFixtureState {
    PopulatedChat,
    Approval,
    GroupChat,
    Disconnected,
    ProviderUnavailable,
    Settings,
    SettingsDevices,
    AssistantPacks,
    AssistantPackConfigure,
    Routines,
    RoutineEditor,
    RoutineRecording,
    ComputerDetails,
}

/// Renders a deterministic fixture through `HomeBotApp::render` without a live server.
///
/// This is intentionally a thin state-construction boundary: all shell geometry,
/// navigation, transcript, activity, approval and composer rendering remains the
/// production application path.
pub fn render_production_fixture(
    context: &egui::Context,
    theme: HomeBotTheme,
    state: ProductionFixtureState,
) {
    let mut app = production_fixture(theme, state);
    app.render(context);
}

#[allow(clippy::too_many_lines)] // A single deterministic fixture prevents scenario drift.
fn production_fixture(theme: HomeBotTheme, state: ProductionFixtureState) -> HomeBotApp {
    let mut app = HomeBotApp::default();
    app.settings.theme = if theme.mode == crate::tokens::ThemeMode::Dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    };
    app.theme = theme;
    if state == ProductionFixtureState::Disconnected {
        app.roster.connection = ConnectionState::Disconnected;
        app.transport_error = Some("The server will reconnect automatically.".to_owned());
        return app;
    }

    let mut bots = vec![
        fixture_bot(
            1,
            "Nova",
            "Research",
            BotColor::Violet,
            BotShape::RoundedSquare,
        ),
        fixture_bot(2, "Patch", "Code", BotColor::Green, BotShape::Hexagon),
        fixture_bot(3, "Scout", "Web", BotColor::Orange, BotShape::Circle),
        fixture_bot(
            4,
            "Mica",
            "Operations",
            BotColor::Rose,
            BotShape::RoundedSquare,
        ),
    ];
    if state == ProductionFixtureState::ProviderUnavailable {
        bots[0].provider = BotProviderStatus::Unavailable;
    }
    let chat = ChatSummary {
        id: Uuid::from_u128(10),
        title: if state == ProductionFixtureState::GroupChat {
            "Launch crew".to_owned()
        } else {
            "Homepage launch".to_owned()
        },
        bot_id: bots[0].id,
        unread_count: 0,
        running: false,
        queued_count: 0,
        last_sequence: 9,
    };
    app.roster.apply_snapshot(bots);
    app.roster.selected = Some(Uuid::from_u128(1));
    app.chats = vec![
        chat.clone(),
        ChatSummary {
            id: Uuid::from_u128(11),
            title: "Weekly product pulse".to_owned(),
            bot_id: Uuid::from_u128(2),
            unread_count: 2,
            running: true,
            queued_count: 1,
            last_sequence: 7,
        },
        ChatSummary {
            id: Uuid::from_u128(12),
            title: "Customer research".to_owned(),
            bot_id: Uuid::from_u128(3),
            unread_count: 0,
            running: false,
            queued_count: 0,
            last_sequence: 4,
        },
    ];
    app.timeline.hydrate(ChatTimelineResponse {
        chat,
        messages: fixture_messages(state),
        activities: vec![ActivitySummary {
            id: Uuid::from_u128(30),
            chat_id: Uuid::from_u128(10),
            message_id: Some(Uuid::from_u128(22)),
            title: "Updated launch checklist".to_owned(),
            detail: "3 files reviewed · tests passed".to_owned(),
            kind: ActivityKind::Filesystem,
            presentation: ActivityPresentation {
                risk: RiskLevel::Low,
                detail: ActivityDetail::Generic {
                    summary: "Prepared the release checklist".to_owned(),
                },
                copy_text: None,
                open_artifact_id: None,
            },
            status: ActivityStatus::Succeeded,
            requires_attention: false,
            started_at_ms: 3,
            finished_at_ms: Some(4),
        }],
        approvals: if state == ProductionFixtureState::Approval {
            vec![ApprovalSummary {
                id: Uuid::from_u128(31),
                chat_id: Uuid::from_u128(10),
                message_id: Some(Uuid::from_u128(22)),
                title: "Approval needed".to_owned(),
                detail: "Patch wants to push the release branch to origin.".to_owned(),
                status: ApprovalStatus::Pending,
                created_at_ms: 5,
                decided_at_ms: None,
            }]
        } else {
            Vec::new()
        },
        queued_prompts: Vec::new(),
        working_context: None,
        checkpoints: Vec::new(),
        boundary_sequence: 9,
    });
    app.timeline.set_at_bottom(false);
    if state == ProductionFixtureState::GroupChat {
        let group = GroupChatSummary {
            id: Uuid::from_u128(40),
            title: "Launch crew".to_owned(),
            ownership_bot_id: Uuid::from_u128(1),
            coordination_max_turns: 12,
            coordination_turns_used: 3,
            max_parallel_bots: 3,
            stop_requested: false,
        };
        app.groups = vec![group.clone()];
        app.selected_group = Some(group.id);
        app.roster.selected = None;
        app.group_timeline.hydrate(GroupTimelineResponse {
            group,
            participants: [1_u128, 2, 3]
                .into_iter()
                .map(|id| GroupParticipantSummary {
                    chat_id: Uuid::from_u128(40),
                    bot_id: Uuid::from_u128(id),
                    role: if id == 1 {
                        GroupParticipantRole::Owner
                    } else {
                        GroupParticipantRole::Member
                    },
                    status: if id == 2 {
                        GroupBotStatus::Running
                    } else {
                        GroupBotStatus::Completed
                    },
                    active_operation_id: None,
                    updated_at_ms: 4,
                })
                .collect(),
            messages: fixture_messages(state),
            handoffs: Vec::new(),
            boundary_sequence: 9,
        });
    }
    app.settings_open = matches!(
        state,
        ProductionFixtureState::Settings | ProductionFixtureState::SettingsDevices
    );
    app.assistant_packs_open = matches!(
        state,
        ProductionFixtureState::AssistantPacks | ProductionFixtureState::AssistantPackConfigure
    );
    if app.assistant_packs_open {
        app.assistant_packs = vec![
            fixture_assistant_pack(
                "morning-brief",
                "Morning Brief",
                "Start the day with priorities, commitments, and anything needing attention.",
                homebot_protocol::AssistantPackCadence::Daily,
                None,
                8,
            ),
            fixture_assistant_pack(
                "weekly-rundown",
                "Weekly Rundown",
                "Wrap up the week with progress, loose ends, and next-week priorities.",
                homebot_protocol::AssistantPackCadence::Weekly,
                Some(5),
                17,
            ),
            fixture_assistant_pack(
                "end-of-day-review",
                "End-of-Day Review",
                "Close the day with completed work, open loops, and tomorrow's first move.",
                homebot_protocol::AssistantPackCadence::Daily,
                None,
                18,
            ),
        ];
        if state == ProductionFixtureState::AssistantPackConfigure {
            app.assistant_pack_install = Some(AssistantPackInstallDraft {
                pack_id: "morning-brief".to_owned(),
                bot_id: Uuid::from_u128(2),
                timezone: "Europe/London".to_owned(),
                hour: 7,
                minute: 45,
            });
        }
    }
    if state == ProductionFixtureState::SettingsDevices {
        app.settings.section = SettingsSection::Devices;
        app.settings.paired_devices = 1;
        "https://homebot.example.test".clone_into(&mut app.pairing_endpoint);
    }
    app.routines_open = matches!(
        state,
        ProductionFixtureState::Routines
            | ProductionFixtureState::RoutineEditor
            | ProductionFixtureState::RoutineRecording
    );
    if app.routines_open {
        app.routines.hydrate(vec![RoutineSummary {
            id: Uuid::from_u128(50),
            bot_id: Uuid::from_u128(1),
            name: "Weekly product pulse".to_owned(),
            description: "Review metrics and prepare the Monday brief.".to_owned(),
            enabled: true,
            draft: false,
            active_version_id: Uuid::from_u128(51),
            version: 3,
            definition: RoutineDefinition {
                inputs: Vec::new(),
                steps: vec![RoutineStep::BotPrompt {
                    bot_id: Uuid::from_u128(1),
                    prompt_template: "Prepare this week's product pulse.".to_owned(),
                    requires_approval: false,
                }],
                expected_outputs: Vec::new(),
            },
            created_at_unix_ms: 1,
            updated_at_unix_ms: 2,
        }]);
        if state == ProductionFixtureState::RoutineEditor {
            if let Some(routine) = app.routines.routines().next().cloned() {
                app.routine_editor = Some(RoutineEditorDraft::from_summary(&routine));
            }
        } else if state == ProductionFixtureState::RoutineRecording {
            app.routine_recording = Some(RoutineRecordingSummary {
                id: Uuid::from_u128(52),
                bot_id: Uuid::from_u128(1),
                name: "Publish release notes".to_owned(),
                description: "Demonstrated from a successful launch".to_owned(),
                actions: vec![RecordedAction {
                    actor: RecordedActor::User,
                    step: RoutineStep::BotPrompt {
                        bot_id: Uuid::from_u128(1),
                        prompt_template: "Summarise the merged changes".to_owned(),
                        requires_approval: true,
                    },
                }],
                created_at_unix_ms: 1,
                updated_at_unix_ms: 2,
            });
            "Draft the public announcement".clone_into(&mut app.routine_recording_prompt);
        }
    }
    app.details_open = state == ProductionFixtureState::ComputerDetails;
    app
}

fn fixture_assistant_pack(
    id: &str,
    name: &str,
    description: &str,
    cadence: homebot_protocol::AssistantPackCadence,
    weekday: Option<u8>,
    default_hour: u8,
) -> homebot_protocol::AssistantPackSummary {
    homebot_protocol::AssistantPackSummary {
        id: id.to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
        skill_name: name.to_owned(),
        routine_name: name.to_owned(),
        schedule: homebot_protocol::AssistantPackSchedule {
            cadence,
            weekday,
            default_hour,
            default_minute: 0,
        },
    }
}

fn fixture_bot(id: u128, name: &str, title: &str, color: BotColor, shape: BotShape) -> BotSummary {
    BotSummary {
        id: Uuid::from_u128(id),
        name: name.to_owned(),
        title: title.to_owned(),
        description: format!("{title} teammate"),
        shape,
        color,
        archived: false,
        pinned: id < 3,
        hidden: false,
        unread_count: u32::from(id == 2),
        attention: if id == 2 {
            BotAttention::Working
        } else {
            BotAttention::None
        },
        provider: BotProviderStatus::Ready,
        advanced: BotAdvancedSettings {
            provider_profile_id: None,
            permission_profile: BotPermissionProfile::AskBeforeChanges,
        },
    }
}

fn fixture_messages(state: ProductionFixtureState) -> Vec<MessageSummary> {
    let chat_id = Uuid::from_u128(if state == ProductionFixtureState::GroupChat {
        40
    } else {
        10
    });
    let user = MessageSummary {
        id: Uuid::from_u128(21),
        chat_id,
        author: MessageAuthor::User,
        author_bot_id: None,
        status: MessageStatus::Completed,
        parts: vec![MessagePart::Text {
            id: Uuid::from_u128(211),
            ordinal: 0,
            text: if state == ProductionFixtureState::GroupChat {
                "@Nova and @Patch, get the launch ready together.".to_owned()
            } else {
                "Can you prepare the homepage launch checklist?".to_owned()
            },
        }],
        reply_to_message_id: None,
        mentioned_bot_ids: Vec::new(),
        shared_context_message_ids: Vec::new(),
        applied_skills: Vec::new(),
        reactions: Vec::new(),
        references: Vec::new(),
        created_at_ms: 1,
        completed_at_ms: Some(1),
        error: None,
    };
    let bot = MessageSummary {
        id: Uuid::from_u128(22),
        chat_id,
        author: MessageAuthor::Bot,
        author_bot_id: Some(Uuid::from_u128(1)),
        status: MessageStatus::Completed,
        parts: vec![MessagePart::Text {
            id: Uuid::from_u128(221),
            ordinal: 0,
            text: if state == ProductionFixtureState::GroupChat {
                "We split the work: I checked the brief while Patch verified the repository. The launch is ready for review.".to_owned()
            } else {
                "Done. I checked the copy, links, analytics events, and rollback steps. The remaining decision is the launch window.".to_owned()
            },
        }],
        reply_to_message_id: Some(user.id),
        mentioned_bot_ids: Vec::new(),
        shared_context_message_ids: vec![user.id],
        applied_skills: Vec::new(),
        reactions: Vec::new(),
        references: Vec::new(),
        created_at_ms: 2,
        completed_at_ms: Some(3),
        error: None,
    };
    vec![user, bot]
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

    #[test]
    fn production_group_fixture_uses_real_group_projection() {
        let app = production_fixture(HomeBotTheme::dark(), ProductionFixtureState::GroupChat);
        assert_eq!(app.selected_group, Some(Uuid::from_u128(40)));
        assert!(app.roster.selected.is_none());
        assert_eq!(app.group_timeline.participants.len(), 3);
        assert_eq!(app.group_timeline.messages.len(), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_path_is_valid_xml() {
        assert_eq!(
            xml_escape("/Applications/HomeBot & Friends/<beta>/\"app\""),
            "/Applications/HomeBot &amp; Friends/&lt;beta&gt;/&quot;app&quot;"
        );
    }
}
