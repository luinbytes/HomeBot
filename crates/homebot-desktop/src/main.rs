fn main() -> eframe::Result {
    eframe::run_native(
        "HomeBot",
        eframe::NativeOptions::default(),
        Box::new(|_creation_context| Ok(Box::new(homebot_desktop::app::HomeBotApp::default()))),
    )
}
