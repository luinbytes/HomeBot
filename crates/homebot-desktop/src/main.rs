fn main() -> eframe::Result {
    eframe::run_native(
        "HomeBot",
        eframe::NativeOptions::default(),
        Box::new(|creation_context| {
            Ok(Box::new(
                homebot_desktop::app::HomeBotApp::from_creation_context(creation_context),
            ))
        }),
    )
}
