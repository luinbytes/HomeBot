//! Desktop settings projection and navigation.

use egui::{Align, Frame, Layout, RichText, Sense, Stroke, Ui};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::tokens::{HomeBotTheme, ThemeMode};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsAction {
    CheckForUpdate,
    StageUpdate,
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
    pub name: String,
    pub detail: String,
    pub state: PluginViewState,
    pub enabled: bool,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct DesktopSettings {
    pub section: SettingsSection,
    pub theme: ThemePreference,
    pub text_scale_percent: u16,
    pub notifications: NotificationPreferences,
    pub launch_at_login: bool,
    pub server_endpoint: String,
    pub provider_status: String,
    pub paired_devices: u32,
    pub update_state: UpdateState,
    pub update_version: Option<String>,
    pub update_message: Option<String>,
    pub plugins: Vec<PluginSettingsItem>,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            section: SettingsSection::General,
            theme: ThemePreference::System,
            text_scale_percent: 100,
            notifications: NotificationPreferences::default(),
            launch_at_login: false,
            server_endpoint: "http://127.0.0.1:7123".to_owned(),
            provider_status: "Codex · Ready".to_owned(),
            paired_devices: 0,
            update_state: UpdateState::Current,
            update_version: None,
            update_message: None,
            plugins: Vec::new(),
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
    let mut action = None;
    ui.horizontal_top(|ui| {
        ui.set_min_width(150.0);
        ui.vertical(|ui| {
            for section in SettingsSection::ALL {
                if ui
                    .selectable_label(settings.section == section, section.label())
                    .clicked()
                {
                    settings.section = section;
                }
            }
        });
        ui.separator();
        ui.add_space(theme.spacing.lg);
        ui.vertical(|ui| {
            ui.set_min_width(480.0);
            ui.label(
                RichText::new(settings.section.label())
                    .font(theme.typography.font(theme.typography.title))
                    .color(theme.palette.text_primary)
                    .strong(),
            );
            ui.add_space(theme.spacing.lg);
            match settings.section {
                SettingsSection::General => general(ui, settings),
                SettingsSection::Plugins => plugins(ui, theme, &settings.plugins),
                SettingsSection::Appearance => appearance(ui, settings),
                SettingsSection::Updates => action = updates(ui, theme, settings),
                SettingsSection::Connection => connection(ui, theme, settings),
                SettingsSection::Devices => {
                    devices(ui, theme, settings.paired_devices);
                    devices_content(ui);
                }
            }
        });
    });
    action
}

fn general(ui: &mut Ui, settings: &mut DesktopSettings) {
    let _ = ui.checkbox(&mut settings.launch_at_login, "Launch HomeBot at login");
    ui.separator();
    ui.strong("Notifications");
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

fn plugins(ui: &mut Ui, theme: HomeBotTheme, plugins: &[PluginSettingsItem]) {
    ui.label("Connect local MCP tools and choose which Bots can use them.");
    ui.add_space(theme.spacing.md);
    if plugins.is_empty() {
        settings_row(ui, theme, "Local MCP", "No plugins connected", "Connect");
        return;
    }
    for plugin in plugins {
        let (state, action) = match plugin.state {
            PluginViewState::Connect => ("Ready to connect", "Connect"),
            PluginViewState::Waiting => ("Waiting for connection…", "Waiting"),
            PluginViewState::Reopen => ("Connection closed", "Reopen"),
            PluginViewState::Connected if plugin.enabled => ("Connected · Enabled", "Disable"),
            PluginViewState::Connected => ("Connected · Disabled", "Enable"),
            PluginViewState::Error => ("Connection error", "Reopen"),
        };
        let detail = if plugin.detail.is_empty() {
            state
        } else {
            &plugin.detail
        };
        settings_row(ui, theme, &plugin.name, detail, action);
    }
}

fn appearance(ui: &mut Ui, settings: &mut DesktopSettings) {
    ui.strong("Theme");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut settings.theme, ThemePreference::System, "System");
        ui.selectable_value(&mut settings.theme, ThemePreference::Light, "Light");
        ui.selectable_value(&mut settings.theme, ThemePreference::Dark, "Dark");
    });
    ui.add_space(8.0);
    ui.strong("Text size");
    ui.add(
        egui::Slider::new(&mut settings.text_scale_percent, 80..=200)
            .suffix("%")
            .text("Text scale"),
    );
    ui.label("HomeBot supports 80% through 200% text scaling without changing server state.");
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
    ui.colored_label(color, label);
    if let Some(version) = &settings.update_version {
        ui.label(format!("Version {version}"));
    }
    if let Some(message) = &settings.update_message {
        ui.label(RichText::new(message).color(theme.palette.text_secondary));
    }
    match settings.update_state {
        UpdateState::Available if ui.button("Download verified update").clicked() => {
            Some(SettingsAction::StageUpdate)
        }
        UpdateState::Checking | UpdateState::Staging => None,
        _ if ui.button("Check again").clicked() => Some(SettingsAction::CheckForUpdate),
        _ => None,
    }
}

fn connection(ui: &mut Ui, theme: HomeBotTheme, settings: &DesktopSettings) {
    settings_row(
        ui,
        theme,
        "HomeBot server",
        &settings.server_endpoint,
        "Change",
    );
    settings_row(ui, theme, "Provider", &settings.provider_status, "Manage");
}

fn devices(ui: &mut Ui, theme: HomeBotTheme, count: u32) {
    settings_row(
        ui,
        theme,
        "Paired devices",
        &format!("{count} active"),
        "Manage",
    );
    let _ = ui.button("Pair Android device");
}

fn settings_row(ui: &mut Ui, theme: HomeBotTheme, title: &str, detail: &str, action: &str) {
    Frame::NONE
        .fill(theme.palette.surface)
        .stroke(Stroke::new(theme.layout.hairline, theme.palette.border))
        .corner_radius(egui::CornerRadius::same(theme.radii.md))
        .inner_margin(egui::Margin::same(theme.insets.md))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.strong(title);
                    ui.label(RichText::new(detail).color(theme.palette.text_secondary));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let _ = ui.add(egui::Button::new(action).sense(Sense::click()));
                });
            });
        });
    ui.add_space(theme.spacing.sm);
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
