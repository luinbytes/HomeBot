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
        "production_settings_light",
        HomeBotTheme::light(),
        ProductionFixtureState::Settings,
    );
    snapshot(
        "production_devices_light",
        HomeBotTheme::light(),
        ProductionFixtureState::SettingsDevices,
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
