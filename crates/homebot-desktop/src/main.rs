fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1120.0, 760.0])
        .with_min_inner_size([800.0, 600.0]);
    eframe::run_native(
        "HomeBot",
        eframe::NativeOptions {
            viewport,
            ..eframe::NativeOptions::default()
        },
        Box::new(|creation_context| {
            Ok(Box::new(
                homebot_desktop::app::HomeBotApp::from_creation_context(creation_context),
            ))
        }),
    )
}
