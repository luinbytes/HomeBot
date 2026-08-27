//! Desktop settings projection and navigation.

use egui::{Align, Frame, Layout, RichText, Sense, Ui};
use homebot_protocol::{MemoryProviderPresetSummary, PluginEventIngressState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::{
    components::navigation_row,
    tokens::{HomeBotTheme, ThemeMode},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SettingsSection {
    General,
    Plugins,
    Appearance,
    Updates,
    Connection,
    Devices,
}

impl SettingsSection {
    pub const ALL: [Self; 6] = [
        Self::General,
        Self::Plugins,
        Self::Appearance,
        Self::Updates,
        Self::Connection,
        Self::Devices,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Plugins => "Plugins",
            Self::Appearance => "Appearance",
            Self::Updates => "Updates",
            Self::Connection => "Connection",
            Self::Devices => "Devices",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::General => "Startup and notification preferences.",
            Self::Plugins => "Connect memory and MCP tools, then control their availability.",
            Self::Appearance => "Choose how HomeBot looks and moves on this computer.",
            Self::Updates => "Check, verify, and stage desktop updates.",
            Self::Connection => "Manage the HomeBot server and provider status.",
            Self::Devices => "Pair devices and review security capabilities.",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ThemePreference {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum UpdateState {
    Current,
    Checking,
    Available,
    Staging,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsAction {
    CheckForUpdate,
    StageUpdate,
    Reconnect,
    RefreshPlugins,
    SetLaunchAtLogin(bool),
    ConfigureMemoryProvider(String),
    ConfigureRemoteMcp,
    ConfigureComposio {
        google_workspace: bool,
    },
    ComposioAccount {
        id: Uuid,
        toolkit: String,
        reauthorize: bool,
    },
    ConfigureComposioEvents {
        id: Uuid,
    },
    Plugin {
        id: Uuid,
        action: PluginAction,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginAction {
    Connect,
    Authorize,
    Enable,
    Disable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PluginViewState {
    Connect,
    Waiting,
    Reopen,
    Connected,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginSettingsItem {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub name: String,
    pub detail: String,
    pub state: PluginViewState,
    pub enabled: bool,
    #[serde(default)]
    pub managed_services: Vec<String>,
    #[serde(default)]
    pub oauth_authorization_available: bool,
    #[serde(default)]
    pub event_ingress_state: PluginEventIngressState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum NotificationTopic {
    Finished,
    Approval,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationPreferences {
    pub enabled: BTreeSet<NotificationTopic>,
    pub when_focused: bool,
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            enabled: [
                NotificationTopic::Finished,
                NotificationTopic::Approval,
                NotificationTopic::Error,
            ]
            .into_iter()
            .collect(),
            when_focused: false,
        }
    }
}

impl NotificationPreferences {
    #[must_use]
    pub fn includes(&self, topic: NotificationTopic) -> bool {
        self.enabled.contains(&topic)
    }

    pub fn set(&mut self, topic: NotificationTopic, enabled: bool) {
        if enabled {
            self.enabled.insert(topic);
        } else {
            self.enabled.remove(&topic);
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct DesktopSettings {
    pub section: SettingsSection,
    pub theme: ThemePreference,
    pub text_scale_percent: u16,
    pub reduce_motion: bool,
    pub notifications: NotificationPreferences,
    pub launch_at_login: bool,
    pub server_endpoint: String,
    pub provider_status: String,
    pub paired_devices: u32,
    pub update_state: UpdateState,
    pub update_version: Option<String>,
    pub update_message: Option<String>,
    #[serde(skip)]
    pub plugins: Vec<PluginSettingsItem>,
    #[serde(skip)]
    pub memory_providers: Vec<MemoryProviderPresetSummary>,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            section: SettingsSection::General,
            theme: ThemePreference::System,
            text_scale_percent: 100,
            reduce_motion: false,
            notifications: NotificationPreferences::default(),
            launch_at_login: false,
            server_endpoint: "http://127.0.0.1:7123".to_owned(),
            provider_status: "Codex · Ready".to_owned(),
            paired_devices: 0,
            update_state: UpdateState::Current,
            update_version: None,
            update_message: None,
            plugins: Vec::new(),
            memory_providers: Vec::new(),
        }
    }
}

impl DesktopSettings {
    #[must_use]
    pub const fn resolved_theme(&self, system_dark: bool) -> ThemeMode {
        match self.theme {
            ThemePreference::System if system_dark => ThemeMode::Dark,
            ThemePreference::System | ThemePreference::Light => ThemeMode::Light,
            ThemePreference::Dark => ThemeMode::Dark,
        }
    }
}

pub fn settings_view(
    ui: &mut Ui,
    theme: HomeBotTheme,
    settings: &mut DesktopSettings,
    devices_content: impl FnOnce(&mut Ui),
) -> Option<SettingsAction> {
    settings_view_with(ui, theme, settings, |ui, section| {
        if section == SettingsSection::Devices {
            devices_content(ui);
        }
    })
}

pub(crate) fn settings_view_with(
    ui: &mut Ui,
    theme: HomeBotTheme,
    settings: &mut DesktopSettings,
    extra: impl FnOnce(&mut Ui, SettingsSection),
) -> Option<SettingsAction> {
    let mut action = None;
    let content_width = (ui.available_width() - 176.0 - theme.spacing.xl).max(320.0);
    ui.horizontal_top(|ui| {
        ui.set_min_width(176.0);
        ui.set_max_width(176.0);
        ui.vertical(|ui| {
            for section in SettingsSection::ALL {
                if navigation_row(ui, theme, section.label(), settings.section == section).clicked()
                {
                    settings.section = section;
                }
            }
        });
        ui.add(egui::Separator::default().vertical().grow(0.0));
        ui.add_space(theme.spacing.lg);
        ui.vertical(|ui| {
            ui.set_width(content_width);
            ui.label(
                RichText::new(settings.section.label())
                    .font(theme.typography.font(theme.typography.title))
                    .color(theme.palette.text_primary)
                    .strong(),
            );
            ui.label(
                RichText::new(settings.section.description()).color(theme.palette.text_secondary),
            );
            ui.add_space(theme.spacing.xl);
            match settings.section {
                SettingsSection::General => action = general(ui, theme, settings),
                SettingsSection::Plugins => {
                    action = plugins(ui, theme, &settings.memory_providers, &settings.plugins);
                }
                SettingsSection::Appearance => appearance(ui, theme, settings),
                SettingsSection::Updates => action = updates(ui, theme, settings),
                SettingsSection::Connection => action = connection(ui, theme, settings),
                SettingsSection::Devices => devices(ui, theme, settings.paired_devices),
            }
            extra(ui, settings.section);
        });
    });
    action
}

fn general(
    ui: &mut Ui,
    theme: HomeBotTheme,
    settings: &mut DesktopSettings,
) -> Option<SettingsAction> {
    let mut launch_changed = false;
    settings_card(ui, theme, |ui| {
        ui.strong("Startup");
        ui.label(
            RichText::new("Open HomeBot automatically after you sign in.")
                .color(theme.palette.text_secondary),
        );
        ui.add_space(theme.spacing.sm);
        launch_changed = ui
            .checkbox(&mut settings.launch_at_login, "Launch HomeBot at login")
            .changed();
    });
    ui.add_space(theme.spacing.md);
    settings_card(ui, theme, |ui| {
        ui.strong("Notifications");
        ui.label(
            RichText::new("Choose which Bot events can interrupt you.")
                .color(theme.palette.text_secondary),
        );
        ui.add_space(theme.spacing.sm);
        notification_checkbox(
            ui,
            &mut settings.notifications,
            NotificationTopic::Finished,
            "Bot finishes work",
        );
        notification_checkbox(
            ui,
            &mut settings.notifications,
            NotificationTopic::Approval,
            "Bot needs approval",
        );
        notification_checkbox(
            ui,
            &mut settings.notifications,
            NotificationTopic::Error,
            "Bot encounters an error",
        );
        let _ = ui.checkbox(
            &mut settings.notifications.when_focused,
            "Notify while HomeBot is focused",
        );
    });
    launch_changed.then_some(SettingsAction::SetLaunchAtLogin(settings.launch_at_login))
}

fn notification_checkbox(
    ui: &mut Ui,
    preferences: &mut NotificationPreferences,
    topic: NotificationTopic,
    label: &str,
) {
    let mut enabled = preferences.includes(topic);
    if ui.checkbox(&mut enabled, label).changed() {
        preferences.set(topic, enabled);
    }
}

fn plugins(
    ui: &mut Ui,
    theme: HomeBotTheme,
    memory_providers: &[MemoryProviderPresetSummary],
    plugins: &[PluginSettingsItem],
) -> Option<SettingsAction> {
    if let Some(action) = integrations(ui, theme) {
        return Some(action);
    }
    ui.add_space(theme.spacing.lg);
    ui.strong("Memory providers");
    ui.label("Memory stays on the HomeBot server and is isolated per Bot.");
    ui.add_space(theme.spacing.md);
    for provider in memory_providers {
        let available = matches!(
            provider.connection_kind.as_str(),
            "streamable_http"
                | "streamable_http_bridge"
                | "custom_mcp"
                | "memory_rest"
                | "oauth_mcp"
                | "builtin_memory"
        );
        let detail = format!(
            "{} · {}",
            if !available {
                "Adapter planned"
            } else if provider.automatic_recall {
                "Automatic scoped recall"
            } else {
                "Manual lifecycle"
            },
            provider.credential_kind.replace('_', " ")
        );
        if settings_row(
            ui,
            theme,
            &provider.name,
            &detail,
            available.then_some("Configure"),
        ) && available
        {
            return Some(SettingsAction::ConfigureMemoryProvider(provider.id.clone()));
        }
    }
    ui.add_space(theme.spacing.lg);
    ui.strong("Plugins & MCP");
    ui.label("Connect tools and control their availability.");
    ui.add_space(theme.spacing.md);
    plugin_rows(ui, theme, plugins)
}

fn plugin_rows(
    ui: &mut Ui,
    theme: HomeBotTheme,
    plugins: &[PluginSettingsItem],
) -> Option<SettingsAction> {
    if plugins.is_empty() {
        return settings_row(
            ui,
            theme,
            "Local MCP",
            "No plugins configured on this server",
            Some("Refresh"),
        )
        .then_some(SettingsAction::RefreshPlugins);
    }
    for plugin in plugins {
        let (state, label, plugin_action) = match plugin.state {
            PluginViewState::Connect => (
                "Ready to connect",
                Some("Connect"),
                Some(PluginAction::Connect),
            ),
            PluginViewState::Waiting => (
                "Waiting for connection…",
                Some("Check"),
                Some(PluginAction::Connect),
            ),
            PluginViewState::Reopen if plugin.oauth_authorization_available => (
                "Authorization required",
                Some("Sign in"),
                Some(PluginAction::Authorize),
            ),
            PluginViewState::Reopen => (
                "Connection closed",
                Some("Reopen"),
                Some(PluginAction::Connect),
            ),
            PluginViewState::Connected if plugin.enabled => (
                "Connected · Enabled",
                Some("Disable"),
                Some(PluginAction::Disable),
            ),
            PluginViewState::Connected => (
                "Connected · Disabled",
                Some("Enable"),
                Some(PluginAction::Enable),
            ),
            PluginViewState::Error => (
                "Connection error",
                Some("Retry"),
                Some(PluginAction::Connect),
            ),
        };
        let detail = if plugin.detail.is_empty() {
            state
        } else {
            &plugin.detail
        };
        if settings_row(ui, theme, &plugin.name, detail, label)
            && let (Some(id), Some(action)) = (plugin.id, plugin_action)
        {
            return Some(SettingsAction::Plugin { id, action });
        }
        if let Some(id) = plugin.id {
            for toolkit in &plugin.managed_services {
                let mut action = None;
                ui.horizontal(|ui| {
                    ui.label(format!("{toolkit} account"));
                    if ui.button("Switch").clicked() {
                        action = Some(true);
                    }
                    if ui.button("Revoke").clicked() {
                        action = Some(false);
                    }
                });
                if let Some(reauthorize) = action {
                    return Some(SettingsAction::ComposioAccount {
                        id,
                        toolkit: toolkit.clone(),
                        reauthorize,
                    });
                }
            }
            if !plugin.managed_services.is_empty() {
                let (label, action) = match plugin.event_ingress_state {
                    PluginEventIngressState::Ready => ("Account events ready", None),
                    PluginEventIngressState::NotConfigured => {
                        ("Account events need public HTTPS", Some("Configure"))
                    }
                    PluginEventIngressState::Error => {
                        ("Account event secret unavailable", Some("Repair"))
                    }
                };
                if settings_row(ui, theme, "Scheduled account events", label, action)
                    && action.is_some()
                {
                    return Some(SettingsAction::ConfigureComposioEvents { id });
                }
            }
        }
    }
    None
}

fn integrations(ui: &mut Ui, theme: HomeBotTheme) -> Option<SettingsAction> {
    ui.strong("Integrations");
    ui.label("Create owner-scoped OAuth connections without storing credentials in a Bot.");
    ui.add_space(theme.spacing.md);
    if settings_row(
        ui,
        theme,
        "Google Workspace",
        "Gmail, Drive, Calendar, Docs, Sheets, Slides, Meet, and Tasks through one Composio connection",
        Some("Connect Google"),
    ) {
        return Some(SettingsAction::ConfigureComposio {
            google_workspace: true,
        });
    }
    if settings_row(
        ui,
        theme,
        "Composio toolkit",
        "Connect one allowlisted service from the Composio catalog",
        Some("Configure"),
    ) {
        return Some(SettingsAction::ConfigureComposio {
            google_workspace: false,
        });
    }
    if settings_row(
        ui,
        theme,
        "Remote MCP server",
        "Connect a public, bearer-authenticated, or OAuth MCP endpoint",
        Some("Configure"),
    ) {
        return Some(SettingsAction::ConfigureRemoteMcp);
    }
    None
}

fn appearance(ui: &mut Ui, theme: HomeBotTheme, settings: &mut DesktopSettings) {
    settings_card(ui, theme, |ui| {
        ui.strong("Theme");
        ui.label(
            RichText::new("Follow this computer or keep a consistent appearance.")
                .color(theme.palette.text_secondary),
        );
        ui.add_space(theme.spacing.sm);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut settings.theme, ThemePreference::System, "System");
            ui.selectable_value(&mut settings.theme, ThemePreference::Light, "Light");
            ui.selectable_value(&mut settings.theme, ThemePreference::Dark, "Dark");
        });
    });
    ui.add_space(theme.spacing.md);
    settings_card(ui, theme, |ui| {
        ui.strong("Accessibility");
        ui.add(
            egui::Slider::new(&mut settings.text_scale_percent, 80..=200)
                .suffix("%")
                .text("Text size"),
        );
        ui.label(
            RichText::new("Text scales without changing server data.")
                .color(theme.palette.text_secondary),
        );
        ui.add_space(theme.spacing.md);
        ui.checkbox(&mut settings.reduce_motion, "Reduce interface motion");
        ui.label(
            RichText::new("Makes sidebar and state transitions immediate.")
                .color(theme.palette.text_secondary),
        );
    });
}

fn updates(ui: &mut Ui, theme: HomeBotTheme, settings: &DesktopSettings) -> Option<SettingsAction> {
    let (label, color) = match settings.update_state {
        UpdateState::Current => ("HomeBot is up to date", theme.palette.success),
        UpdateState::Checking => ("Checking for updates…", theme.palette.text_secondary),
        UpdateState::Available => ("An update is ready", theme.palette.accent),
        UpdateState::Staging => (
            "Downloading and verifying update…",
            theme.palette.text_secondary,
        ),
        UpdateState::Ready => ("Verified update is ready to install", theme.palette.success),
        UpdateState::Failed => ("Update check failed", theme.palette.danger),
    };
    let mut action = None;
    settings_card(ui, theme, |ui| {
        ui.strong("Desktop app");
        ui.colored_label(color, label);
        if let Some(version) = &settings.update_version {
            ui.label(format!("Version {version}"));
        }
        if let Some(message) = &settings.update_message {
            ui.label(RichText::new(message).color(theme.palette.text_secondary));
        }
        ui.add_space(theme.spacing.md);
        action = match settings.update_state {
            UpdateState::Available if ui.button("Download verified update").clicked() => {
                Some(SettingsAction::StageUpdate)
            }
            UpdateState::Checking | UpdateState::Staging => {
                ui.spinner();
                None
            }
            _ if ui.button("Check again").clicked() => Some(SettingsAction::CheckForUpdate),
            _ => None,
        };
    });
    action
}

fn connection(
    ui: &mut Ui,
    theme: HomeBotTheme,
    settings: &mut DesktopSettings,
) -> Option<SettingsAction> {
    let mut action = None;
    settings_card(ui, theme, |ui| {
        ui.strong("HomeBot server");
        ui.label(
            RichText::new("The desktop reconnects securely after this address changes.")
                .color(theme.palette.text_secondary),
        );
        ui.add_space(theme.spacing.sm);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut settings.server_endpoint)
                    .desired_width((ui.available_width() - 96.0).max(120.0)),
            );
            if ui.button("Reconnect").clicked() {
                action = Some(SettingsAction::Reconnect);
            }
        });
    });
    ui.add_space(theme.spacing.md);
    settings_row(ui, theme, "Provider", &settings.provider_status, None);
    action
}

fn devices(ui: &mut Ui, theme: HomeBotTheme, count: u32) {
    settings_row(
        ui,
        theme,
        "Paired devices",
        &format!("{count} active"),
        None,
    );
}

fn settings_row(
    ui: &mut Ui,
    theme: HomeBotTheme,
    title: &str,
    detail: &str,
    action: Option<&str>,
) -> bool {
    let mut clicked = false;
    Frame::NONE
        .fill(theme.palette.surface_hover)
        .stroke(egui::Stroke::new(
            theme.layout.hairline,
            theme.palette.border,
        ))
        .corner_radius(egui::CornerRadius::same(theme.radii.sm))
        .inner_margin(egui::Margin::same(theme.insets.md))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.strong(title);
                    ui.label(RichText::new(detail).color(theme.palette.text_secondary));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(action) = action {
                        clicked = ui
                            .add(egui::Button::new(action).sense(Sense::click()))
                            .clicked();
                    }
                });
            });
        });
    ui.add_space(theme.spacing.sm);
    clicked
}

pub(crate) fn settings_card(ui: &mut Ui, theme: HomeBotTheme, content: impl FnOnce(&mut Ui)) {
    Frame::NONE
        .fill(theme.palette.surface_hover)
        .stroke(egui::Stroke::new(
            theme.layout.hairline,
            theme.palette.border,
        ))
        .corner_radius(egui::CornerRadius::same(theme.radii.sm))
        .inner_margin(egui::Margin::same(theme.insets.lg))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            content(ui);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_resolution_and_navigation_are_deterministic() {
        let mut settings = DesktopSettings::default();
        assert_eq!(settings.resolved_theme(true), ThemeMode::Dark);
        settings.theme = ThemePreference::Light;
        assert_eq!(settings.resolved_theme(true), ThemeMode::Light);
        assert_eq!(SettingsSection::ALL.len(), 6);
        settings.notifications.set(NotificationTopic::Error, false);
        let encoded = serde_json::to_string(&settings).unwrap_or_else(|error| panic!("{error}"));
        let decoded: DesktopSettings =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, settings);
    }
}
