use dmo_gui::DmoApp;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DMO — Open Modern DAW",
        options,
        Box::new(|creation_context| Ok(Box::new(DmoApp::new(creation_context)))),
    )
}
