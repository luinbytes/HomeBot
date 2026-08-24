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
            install_platform_font(&creation_context.egui_ctx);
            Ok(Box::new(
                homebot_desktop::app::HomeBotApp::from_creation_context(creation_context),
            ))
        }),
    )
}

fn install_platform_font(context: &egui::Context) {
    let Some((path, data)) = platform_font_paths()
        .iter()
        .find_map(|path| std::fs::read(path).ok().map(|data| (*path, data)))
    else {
        return;
    };
    context.add_font(egui::epaint::text::FontInsert::new(
        path,
        egui::FontData::from_owned(data),
        vec![egui::epaint::text::InsertFontFamily {
            family: egui::FontFamily::Proportional,
            priority: egui::epaint::text::FontPriority::Highest,
        }],
    ));
}

#[cfg(target_os = "macos")]
const fn platform_font_paths() -> &'static [&'static str] {
    &["/System/Library/Fonts/SFNS.ttf"]
}

#[cfg(target_os = "linux")]
const fn platform_font_paths() -> &'static [&'static str] {
    &[
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const fn platform_font_paths() -> &'static [&'static str] {
    &[]
}
