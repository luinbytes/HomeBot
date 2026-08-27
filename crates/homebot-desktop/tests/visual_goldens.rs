use egui::Vec2;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use homebot_desktop::{HomeBotTheme, ProductionFixtureState, render_production_fixture};

mod support;

fn snapshot(name: &str, theme: HomeBotTheme, state: ProductionFixtureState) {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(
            theme.layout.reference_width,
            homebot_desktop::tokens::Layout::REFERENCE_HEIGHT,
        ))
        .with_pixels_per_point(1.0)
        .renderer(support::CpuRenderer::default())
        .build(|context| render_production_fixture(context, theme, state));
    harness.run_steps(2);
    let image = harness
        .render()
        .unwrap_or_else(|error| panic!("visual render failed: {error}"));
    egui_kittest::image_snapshot(&image, name);
}

#[test]
fn production_composio_event_setup_is_visible() {
    let theme = HomeBotTheme::light();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(
            theme.layout.reference_width,
            homebot_desktop::tokens::Layout::REFERENCE_HEIGHT,
        ))
        .with_pixels_per_point(1.0)
        .renderer(support::CpuRenderer::default())
        .build(|context| {
            render_production_fixture(context, theme, ProductionFixtureState::SettingsPlugins);
        });
    harness.run_steps(2);
    for _ in 0..8 {
        harness.get_by_label("Integrations").scroll_down();
        harness.step();
    }
    let event_row = harness.get_by_label("Scheduled account events").rect();
    assert!(
        event_row.min.y >= 0.0
            && event_row.max.y <= homebot_desktop::tokens::Layout::REFERENCE_HEIGHT,
        "Composio event setup row is outside the production viewport: {event_row:?}"
    );
    let image = harness
        .render()
        .unwrap_or_else(|error| panic!("visual render failed: {error}"));
    egui_kittest::image_snapshot(&image, "production_composio_events_light");
}

#[test]
fn production_devices_settings_exposes_pairing_action() {
    let theme = HomeBotTheme::light();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(
            theme.layout.reference_width,
            homebot_desktop::tokens::Layout::REFERENCE_HEIGHT,
        ))
        .build(|context| {
            render_production_fixture(context, theme, ProductionFixtureState::SettingsDevices);
        });
    harness.run_steps(2);

    let action = harness.get_by_label("Generate link");
    assert!(!action.accesskit_node().is_disabled());
    assert!(
        action.rect().max.y <= homebot_desktop::tokens::Layout::REFERENCE_HEIGHT,
        "pairing action is clipped below the production settings viewport"
    );
}

#[test]
fn production_shell_exposes_working_navigation_and_send_states() {
    let theme = HomeBotTheme::dark();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(
            theme.layout.reference_width,
            homebot_desktop::tokens::Layout::REFERENCE_HEIGHT,
        ))
        .build(|context| {
            render_production_fixture(context, theme, ProductionFixtureState::PopulatedChat);
        });
    harness.run_steps(2);

    for label in [
        "Search",
        "Assistant Packs",
        "Routines",
        "Plugins",
        "Account & settings",
        "Hide sidebar",
    ] {
        assert!(!harness.get_by_label(label).accesskit_node().is_disabled());
    }
    assert!(
        harness
            .get_by_label("Send message")
            .accesskit_node()
            .is_disabled(),
        "an empty composer must expose a real disabled send state"
    );
}

#[test]
fn interaction_cards_expose_native_accessible_controls() {
    let theme = HomeBotTheme::dark();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(
            theme.layout.reference_width,
            homebot_desktop::tokens::Layout::REFERENCE_HEIGHT,
        ))
        .build(|context| {
            render_production_fixture(context, theme, ProductionFixtureState::Interaction);
        });
    harness.run_steps(2);

    for label in ["Review first", "Publish now", "Secret value"] {
        assert!(
            harness.get_by_label(label).rect().max.y
                <= homebot_desktop::tokens::Layout::REFERENCE_HEIGHT,
            "{label} is clipped outside the interaction card"
        );
    }
    assert!(
        harness
            .get_by_label("Store securely")
            .accesskit_node()
            .is_disabled(),
        "an empty secret field must not submit"
    );
}

#[test]
fn compact_modal_actions_remain_inside_the_viewport() {
    const COMPACT_SIZE: Vec2 = Vec2::new(800.0, 600.0);
    for (state, action) in [
        (ProductionFixtureState::Settings, "Close"),
        (ProductionFixtureState::BotEditor, "Save changes"),
        (ProductionFixtureState::DeleteBot, "Delete permanently"),
        (ProductionFixtureState::GroupDetails, "Done"),
        (ProductionFixtureState::RoutineEditor, "Save routine"),
        (
            ProductionFixtureState::AssistantPackConfigure,
            "Install and enable",
        ),
    ] {
        let theme = HomeBotTheme::dark();
        let mut harness = Harness::builder()
            .with_size(COMPACT_SIZE)
            .build(|context| render_production_fixture(context, theme, state));
        harness.run_steps(2);

        let rect = harness.get_by_label(action).rect();
        assert!(
            rect.min.x >= 0.0
                && rect.min.y >= 0.0
                && rect.max.x <= COMPACT_SIZE.x
                && rect.max.y <= COMPACT_SIZE.y,
            "{action} is clipped at compact desktop size: {rect:?}"
        );
    }
}

#[test]
fn production_desktop_visual_goldens() {
    snapshot(
        "production_chat_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::PopulatedChat,
    );
    snapshot(
        "production_approval_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::Approval,
    );
    snapshot(
        "production_interaction_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::Interaction,
    );
    snapshot(
        "production_group_chat_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::GroupChat,
    );
    snapshot(
        "production_disconnected_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::Disconnected,
    );
    snapshot(
        "production_provider_unavailable_light",
        HomeBotTheme::light(),
        ProductionFixtureState::ProviderUnavailable,
    );
    snapshot(
        "production_computer_details_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::ComputerDetails,
    );
    snapshot(
        "production_group_details_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::GroupDetails,
    );
    snapshot(
        "production_bot_editor_light",
        HomeBotTheme::light(),
        ProductionFixtureState::BotEditor,
    );
    snapshot(
        "production_delete_bot_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::DeleteBot,
    );
    snapshot(
        "production_settings_light",
        HomeBotTheme::light(),
        ProductionFixtureState::Settings,
    );
    snapshot(
        "production_plugins_light",
        HomeBotTheme::light(),
        ProductionFixtureState::SettingsPlugins,
    );
    snapshot(
        "production_memory_provider_activate_light",
        HomeBotTheme::light(),
        ProductionFixtureState::MemoryProviderActivate,
    );
    snapshot(
        "production_devices_light",
        HomeBotTheme::light(),
        ProductionFixtureState::SettingsDevices,
    );
    snapshot(
        "production_assistant_packs_light",
        HomeBotTheme::light(),
        ProductionFixtureState::AssistantPacks,
    );
    snapshot(
        "production_assistant_pack_configure_light",
        HomeBotTheme::light(),
        ProductionFixtureState::AssistantPackConfigure,
    );
    snapshot(
        "production_routines_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::Routines,
    );
    snapshot(
        "production_routine_editor_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::RoutineEditor,
    );
    snapshot(
        "production_routine_recording_dark",
        HomeBotTheme::dark(),
        ProductionFixtureState::RoutineRecording,
    );
}
