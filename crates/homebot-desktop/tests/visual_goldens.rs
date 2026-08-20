use egui::Vec2;
use egui_kittest::Harness;
use homebot_desktop::{FixtureState, HomeBotTheme, render_fixture};

mod support;

fn snapshot(name: &str, theme: HomeBotTheme, state: FixtureState) {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(
            theme.layout.reference_width,
            theme.layout.reference_height,
        ))
        .with_pixels_per_point(1.0)
        .renderer(support::CpuRenderer::default())
        .build(|context| render_fixture(context, theme, state));
    harness.run();
    let image = harness
        .render()
        .unwrap_or_else(|error| panic!("visual render failed: {error}"));
    egui_kittest::image_snapshot(&image, name);
}

#[test]
fn desktop_shell_visual_goldens() {
    snapshot(
        "desktop_empty_light",
        HomeBotTheme::light(),
        FixtureState::Empty,
    );
    snapshot(
        "desktop_chat_light",
        HomeBotTheme::light(),
        FixtureState::DirectChat,
    );
    snapshot(
        "desktop_approval_dark",
        HomeBotTheme::dark(),
        FixtureState::Approval,
    );
    snapshot(
        "desktop_queue_error_dark",
        HomeBotTheme::dark(),
        FixtureState::QueueError,
    );
    snapshot(
        "desktop_group_chat_light",
        HomeBotTheme::light(),
        FixtureState::GroupChat,
    );
    snapshot(
        "desktop_bot_editor_light",
        HomeBotTheme::light(),
        FixtureState::BotEditor,
    );
    snapshot(
        "desktop_disconnected_dark",
        HomeBotTheme::dark(),
        FixtureState::Disconnected,
    );
    snapshot(
        "desktop_provider_unavailable_light",
        HomeBotTheme::light(),
        FixtureState::ProviderUnavailable,
    );
    snapshot(
        "desktop_activity_surfaces_dark",
        HomeBotTheme::dark(),
        FixtureState::ActivitySurfaces,
    );
    snapshot(
        "desktop_settings_general_light",
        HomeBotTheme::light(),
        FixtureState::Settings,
    );
    snapshot(
        "desktop_settings_appearance_dark",
        HomeBotTheme::dark(),
        FixtureState::SettingsAppearance,
    );
    snapshot(
        "desktop_settings_plugins_light",
        HomeBotTheme::light(),
        FixtureState::SettingsPlugins,
    );
}
