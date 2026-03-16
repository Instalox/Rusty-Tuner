mod audio;
mod gui;
mod pitch;
mod tuning;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([400.0, 600.0])
            .with_min_inner_size([350.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        "RustyTuner",
        options,
        Box::new(|cc| Ok(Box::new(gui::TunerApp::new(cc)))),
    )
}
