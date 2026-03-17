mod gauge;

use crate::audio::{self, AudioBuffer, AudioEngine, ToneGenerator, ToneState};
use crate::pitch::PitchEngine;
use crate::tuning::{self, Tuning};
use eframe::egui;
use std::f32::consts::PI;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Hysteresis: require this many consecutive frames of a new note before switching display.
const HYSTERESIS_FRAMES: u32 = 5;

/// How quickly the needle animates toward target (0 = frozen, 1 = instant).
const NEEDLE_LERP: f32 = 0.18;

/// Frames of silence before clearing the display.
const SILENCE_TIMEOUT: u32 = 20;

pub struct TunerApp {
    shared_buffer: Arc<Mutex<AudioBuffer>>,
    _audio_engine: Option<AudioEngine>,
    audio_error: Option<String>,
    pitch_engine: PitchEngine,

    // Display state
    smoothed_cents: f64,
    display_note: String,
    display_octave: i32,
    detected_freq: f64,
    clarity: f64,
    rms_level: f32,
    silence_frames: u32,

    // Animated needle angle (radians, PI=left, 0=right)
    needle_angle: f32,

    // Auto-detected nearest string
    auto_string_idx: Option<usize>,
    auto_string_cents: f64,

    // Hysteresis
    candidate_note: String,
    candidate_octave: i32,
    candidate_count: u32,

    // Settings
    tunings: Vec<Tuning>,
    selected_tuning: usize,
    a4_freq: f64,
    selected_string: Option<usize>,

    // Reference tone
    tone_state: Arc<ToneState>,
    _tone_generator: Option<ToneGenerator>,
    tone_volume: f32,
    playing_string: Option<usize>,

    // VU meter peak hold
    vu_peak: f32,
    vu_peak_timer: f32,

    // Waveform display
    waveform_samples: Vec<f32>,
    show_waveform: bool,

    // Input device
    input_devices: Vec<String>,
    selected_device: usize, // 0 = "Default", 1+ = named devices

    // Background Image
    bg_texture: Option<egui::TextureHandle>,

    // Zone calibration (fractions of pedal image)
    show_calibration: bool,
    zone_screen: [f32; 4],     // [left, top, right, bottom]
    zone_strip: [f32; 4],
    zone_controls: [f32; 4],
    zone_foot_center: [f32; 2], // [x, y]
    zone_foot_r: f32,
    zone_leds_y: f32,
    zone_knob_left: [f32; 2],  // [x, y]
    zone_knob_right: [f32; 2],
    zone_branding_y: f32,
    zone_labels_y: f32,
    calibration_drag: Option<(&'static str, usize)>, // which handle is being dragged
}

fn load_image_from_path(path: &std::path::Path) -> Result<egui::ColorImage, image::ImageError> {
    let image = image::open(path)?;
    let size = [image.width() as _, image.height() as _];
    let image_buffer = image.to_rgba8();
    let pixels = image_buffer.as_flat_samples();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        pixels.as_slice(),
    ))
}

fn draw_3d_button(
    painter: &egui::Painter,
    rect: egui::Rect,
    is_pressed: bool,
    is_hovered: bool,
    glow_color: Option<egui::Color32>,
    corner_radius: f32,
) -> egui::Rect {
    // Outer shadow/bevel (black housing)
    let shadow_rect = rect.translate(egui::vec2(0.0, 2.0)).expand(1.5);
    painter.rect_filled(shadow_rect, corner_radius + 1.0, egui::Color32::from_black_alpha(255));
    
    let btn_rect = if is_pressed {
        rect.translate(egui::vec2(0.0, 1.0))
    } else {
        rect
    };

    // Dark base color
    let base_color = if is_hovered { egui::Color32::from_rgb(42, 44, 48) } else { egui::Color32::from_rgb(32, 34, 38) };
    painter.rect_filled(btn_rect, corner_radius, base_color);

    // Inner bevel shadow and highlight
    painter.rect_stroke(btn_rect, corner_radius, egui::Stroke::new(1.0, egui::Color32::from_black_alpha(200)), egui::StrokeKind::Outside);
    painter.line_segment(
        [btn_rect.left_top() + egui::vec2(corner_radius, 1.0), btn_rect.right_top() + egui::vec2(-corner_radius, 1.0)],
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(30))
    );
    painter.line_segment(
        [btn_rect.left_bottom() + egui::vec2(corner_radius, -1.0), btn_rect.right_bottom() + egui::vec2(-corner_radius, -1.0)],
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(150))
    );

    // Top gloss
    let top_half = egui::Rect::from_min_max(btn_rect.min, egui::Pos2::new(btn_rect.right(), btn_rect.top() + btn_rect.height() * 0.45));
    painter.rect_filled(top_half, corner_radius, egui::Color32::from_white_alpha(8));

    if let Some(color) = glow_color {
        // Glowing border (active/selected)
        painter.rect_stroke(btn_rect, corner_radius, egui::Stroke::new(1.5, color), egui::StrokeKind::Inside);
        painter.rect_stroke(btn_rect.shrink(1.0), corner_radius - 1.0, egui::Stroke::new(2.0, color.linear_multiply(0.2)), egui::StrokeKind::Inside);
        painter.rect_stroke(btn_rect.expand(1.0), corner_radius + 1.0, egui::Stroke::new(3.0, color.linear_multiply(0.15)), egui::StrokeKind::Outside);
    }
    
    btn_rect
}

impl TunerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Load custom font
        let mut fonts = egui::FontDefinitions::default();
        if let Ok(font_data) = std::fs::read("fonts/digital-7/digital-7.ttf") {
            fonts.font_data.insert(
                "digital7".to_owned(),
                egui::FontData::from_owned(font_data).into(),
            );
            fonts.families.entry(egui::FontFamily::Name("digital7".into()))
                .or_default()
                .insert(0, "digital7".to_owned());
            // Also add as fallback to proportional so it's available
            fonts.families.entry(egui::FontFamily::Proportional)
                .or_default()
                .push("digital7".to_owned());
        }
        if let Ok(font_data) = std::fs::read("fonts/playball/Playball-Regular.ttf") {
            fonts.font_data.insert(
                "playball".to_owned(),
                egui::FontData::from_owned(font_data).into(),
            );
            fonts.families.entry(egui::FontFamily::Name("playball".into()))
                .or_default()
                .insert(0, "playball".to_owned());
        }
        cc.egui_ctx.set_fonts(fonts);

        // Dark theme
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::from_rgb(25, 27, 30);
        visuals.window_fill = egui::Color32::from_rgb(25, 27, 30);
        visuals.extreme_bg_color = egui::Color32::from_rgb(12, 14, 16);
        visuals.faint_bg_color = egui::Color32::from_rgb(20, 22, 24);
        visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(30, 32, 35);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(35, 38, 42);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(45, 48, 55);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(50, 55, 65);
        cc.egui_ctx.set_visuals(visuals);

        let input_devices = audio::list_input_devices();

        let shared_buffer = Arc::new(Mutex::new(AudioBuffer::new(44100)));

        let (audio_engine, audio_error) =
            match AudioEngine::start(Arc::clone(&shared_buffer), None) {
                Ok(engine) => (Some(engine), None),
                Err(e) => (None, Some(format!("Audio error: {e}"))),
            };

        let tone_state = Arc::new(ToneState::new());
        let tone_generator = match ToneGenerator::start(Arc::clone(&tone_state)) {
            Ok(tg) => Some(tg),
            Err(e) => {
                eprintln!("Could not start tone generator: {e}");
                None
            }
        };

        Self {
            shared_buffer,
            _audio_engine: audio_engine,
            audio_error,
            pitch_engine: PitchEngine::new(),

            smoothed_cents: 0.0,
            display_note: String::new(),
            display_octave: 0,
            detected_freq: 0.0,
            clarity: 0.0,
            rms_level: 0.0,
            silence_frames: SILENCE_TIMEOUT,

            needle_angle: PI / 2.0, // start centered

            auto_string_idx: None,
            auto_string_cents: 0.0,

            candidate_note: String::new(),
            candidate_octave: 0,
            candidate_count: 0,

            tunings: tuning::all_tunings(),
            selected_tuning: 0,
            a4_freq: 440.0,
            selected_string: None,

            tone_state,
            _tone_generator: tone_generator,
            tone_volume: 0.25,
            playing_string: None,

            vu_peak: 0.0,
            vu_peak_timer: 0.0,
            waveform_samples: Vec::new(),
            show_waveform: false,

            input_devices,
            selected_device: 0,
            bg_texture: None,

            show_calibration: false,
            zone_screen: [0.156, 0.292, 0.842, 0.500],
            zone_strip: [0.206, 0.517, 0.794, 0.576],
            zone_controls: [0.223, 0.594, 0.769, 0.661],
            zone_foot_center: [0.504, 0.754],
            zone_foot_r: 0.09,
            zone_leds_y: 0.731,
            zone_knob_left: [0.265, 0.195],
            zone_knob_right: [0.710, 0.196],
            zone_branding_y: 0.139,
            zone_labels_y: 0.253,
            calibration_drag: None,
        }
    }
}

impl eframe::App for TunerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(33));

        // Keyboard shortcuts
        ctx.input(|i| {
            use egui::Key;
            let num_strings = self.tunings[self.selected_tuning].strings.len();

            // 1-6: select string
            for (key, idx) in [
                (Key::Num1, 0),
                (Key::Num2, 1),
                (Key::Num3, 2),
                (Key::Num4, 3),
                (Key::Num5, 4),
                (Key::Num6, 5),
            ] {
                if i.key_pressed(key) && idx < num_strings {
                    self.selected_string = Some(idx);
                }
            }

            // 0 or A: auto mode
            if i.key_pressed(Key::Num0) || i.key_pressed(Key::A) {
                self.selected_string = None;
            }

            // Space: toggle play/stop for selected (or first) string
            if i.key_pressed(Key::Space) {
                if self.playing_string.is_some() {
                    self.tone_state.stop();
                    self.playing_string = None;
                } else {
                    let idx = self.selected_string.unwrap_or(0);
                    if idx < num_strings {
                        let freq = self.tunings[self.selected_tuning].strings[idx]
                            .frequency(self.a4_freq) as f32;
                        self.tone_state.set_frequency(freq);
                        self.tone_state.set_volume(self.tone_volume);
                        self.playing_string = Some(idx);
                    }
                }
            }

            // W: toggle waveform
            if i.key_pressed(Key::W) {
                self.show_waveform = !self.show_waveform;
            }

            // D: toggle calibration overlay
            if i.key_pressed(Key::D) {
                self.show_calibration = !self.show_calibration;
            }
        });

        // Grab samples
        let (samples, sample_rate, rms) = {
            if let Ok(buf) = self.shared_buffer.lock() {
                (buf.latest(self.pitch_engine.detection_size()), buf.sample_rate, buf.rms(2048))
            } else {
                return;
            }
        };

        self.rms_level = rms;

        // VU peak hold: track peak, hold for ~1s then decay
        let dt = ctx.input(|i| i.predicted_dt);
        let db_now = if rms > 0.0 { (20.0 * rms.log10()).clamp(-60.0, 0.0) } else { -60.0 };
        let level_now = ((db_now + 60.0) / 60.0).clamp(0.0, 1.0);
        if level_now >= self.vu_peak {
            self.vu_peak = level_now;
            self.vu_peak_timer = 1.0; // hold for 1 second
        } else {
            self.vu_peak_timer -= dt;
            if self.vu_peak_timer <= 0.0 {
                // Decay the peak
                self.vu_peak = (self.vu_peak - dt * 0.8).max(level_now);
            }
        }

        // Grab ~20ms of waveform for display (~880 samples at 44.1kHz)
        let waveform_len = (sample_rate as usize / 50).min(samples.len());
        self.waveform_samples = samples[samples.len() - waveform_len..].to_vec();

        // Pitch detection
        if let Some(result) = self.pitch_engine.detect(&samples, sample_rate) {
            self.detected_freq = result.frequency;
            self.clarity = result.clarity;
            self.silence_frames = 0;

            let note_info = tuning::frequency_to_note(result.frequency, self.a4_freq);

            // Auto-detect nearest string
            let current_tuning = &self.tunings[self.selected_tuning];
            let (auto_idx, auto_cents) =
                tuning::closest_string(result.frequency, current_tuning, self.a4_freq);
            self.auto_string_idx = Some(auto_idx);
            self.auto_string_cents = auto_cents;

            // Compute cents: either from selected string or from nearest note
            let cents = if let Some(string_idx) = self.selected_string {
                let tuning = &self.tunings[self.selected_tuning];
                if string_idx < tuning.strings.len() {
                    let target_freq = tuning.strings[string_idx].frequency(self.a4_freq);
                    1200.0 * (result.frequency / target_freq).log2()
                } else {
                    note_info.cents_offset
                }
            } else {
                note_info.cents_offset
            };

            // Adaptive EMA: faster when far off, slower when close (for stability)
            let alpha = if cents.abs() < 5.0 { 0.12 } else if cents.abs() < 15.0 { 0.20 } else { 0.35 };
            self.smoothed_cents = self.smoothed_cents * (1.0 - alpha) + cents * alpha;

            // Note hysteresis
            if note_info.name != self.candidate_note || note_info.octave != self.candidate_octave {
                self.candidate_note = note_info.name.to_string();
                self.candidate_octave = note_info.octave;
                self.candidate_count = 1;
            } else {
                self.candidate_count += 1;
            }

            if self.candidate_count >= HYSTERESIS_FRAMES {
                self.display_note = self.candidate_note.clone();
                self.display_octave = self.candidate_octave;
            }
        } else {
            self.silence_frames = self.silence_frames.saturating_add(1);

            // Gradual fade-out
            if self.silence_frames > 3 {
                self.clarity *= 0.85;
                self.smoothed_cents *= 0.92;
            }

            if self.silence_frames > SILENCE_TIMEOUT {
                self.clarity = 0.0;
                self.detected_freq = 0.0;
                self.auto_string_idx = None;
            }
        }

        // Animate needle toward target
        let target_angle = if self.clarity > 0.01 {
            let clamped = (self.smoothed_cents as f32).clamp(-50.0, 50.0);
            let t = (clamped + 50.0) / 100.0;
            PI + (0.0 - PI) * t // PI (left) to 0 (right)
        } else {
            PI / 2.0 // centered when no signal
        };
        self.needle_angle += (target_angle - self.needle_angle) * NEEDLE_LERP;

        // Keep playing tone in sync with current tuning/A4
        if let Some(idx) = self.playing_string {
            let tuning = &self.tunings[self.selected_tuning];
            if idx < tuning.strings.len() {
                let freq = tuning.strings[idx].frequency(self.a4_freq) as f32;
                self.tone_state.set_frequency(freq);
            } else {
                self.tone_state.stop();
                self.playing_string = None;
            }
        }

        // Load texture if not already loaded
        let bg_texture = self.bg_texture.get_or_insert_with(|| {
            let path = std::path::PathBuf::from("images/blank-pedal3.png");
            if let Ok(color_image) = load_image_from_path(&path) {
                ctx.load_texture(
                    "blank-pedal",
                    color_image,
                    egui::TextureOptions::LINEAR,
                )
            } else {
                // Fallback texture if loading fails
                ctx.load_texture(
                    "fallback",
                    egui::ColorImage::example(),
                    egui::TextureOptions::default(),
                )
            }
        });

        // ---- UI ----
        let frame = egui::Frame::default()
            .fill(egui::Color32::from_rgb(22, 24, 28))
            .inner_margin(egui::Margin::same(0));

        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let panel = ui.max_rect();
            let time = ctx.input(|i| i.time);

            // Maintain 2:3 aspect ratio, centered in panel
            let pedal_aspect = 1.5;
            let (pw, ph) = if panel.width() * pedal_aspect <= panel.height() {
                (panel.width(), panel.width() * pedal_aspect)
            } else {
                (panel.height() / pedal_aspect, panel.height())
            };
            let pedal_rect = egui::Rect::from_center_size(
                panel.center(),
                egui::Vec2::new(pw, ph),
            );

            // Draw pedal background image
            ui.painter().image(
                bg_texture.id(),
                pedal_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );

            // Helper: convert pedal-relative fractions to screen coords
            let px = |frac_x: f32| pedal_rect.left() + frac_x * pw;
            let py = |frac_y: f32| pedal_rect.top() + frac_y * ph;

            // ============================================================
            // CALIBRATION MODE (press D to toggle)
            // Draws zone rectangles with draggable handles
            // ============================================================
            if self.show_calibration {
                // Screen zone
                let sr = egui::Rect::from_min_max(
                    egui::Pos2::new(px(self.zone_screen[0]), py(self.zone_screen[1])),
                    egui::Pos2::new(px(self.zone_screen[2]), py(self.zone_screen[3])),
                );
                ui.painter().rect_stroke(sr, 0.0, egui::Stroke::new(2.0, egui::Color32::RED), egui::StrokeKind::Outside);
                ui.painter().text(sr.left_top() + egui::vec2(4.0, 4.0), egui::Align2::LEFT_TOP, "SCREEN", egui::FontId::proportional(12.0), egui::Color32::RED);

                // Strip zone
                let st = egui::Rect::from_min_max(
                    egui::Pos2::new(px(self.zone_strip[0]), py(self.zone_strip[1])),
                    egui::Pos2::new(px(self.zone_strip[2]), py(self.zone_strip[3])),
                );
                ui.painter().rect_stroke(st, 0.0, egui::Stroke::new(2.0, egui::Color32::GREEN), egui::StrokeKind::Outside);
                ui.painter().text(st.left_top() + egui::vec2(4.0, 4.0), egui::Align2::LEFT_TOP, "STRIP", egui::FontId::proportional(12.0), egui::Color32::GREEN);

                // Controls zone
                let ct = egui::Rect::from_min_max(
                    egui::Pos2::new(px(self.zone_controls[0]), py(self.zone_controls[1])),
                    egui::Pos2::new(px(self.zone_controls[2]), py(self.zone_controls[3])),
                );
                ui.painter().rect_stroke(ct, 0.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 200, 255)), egui::StrokeKind::Outside);
                ui.painter().text(ct.left_top() + egui::vec2(4.0, 4.0), egui::Align2::LEFT_TOP, "CONTROLS", egui::FontId::proportional(12.0), egui::Color32::from_rgb(0, 200, 255));

                // Footswitch
                let fc = egui::Pos2::new(px(self.zone_foot_center[0]), py(self.zone_foot_center[1]));
                let fr = pw * self.zone_foot_r;
                ui.painter().circle_stroke(fc, fr, egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 0, 255)));
                ui.painter().text(fc + egui::vec2(0.0, -fr - 8.0), egui::Align2::CENTER_BOTTOM, "FOOT", egui::FontId::proportional(12.0), egui::Color32::from_rgb(255, 0, 255));

                // LEDs Y line
                let ly = py(self.zone_leds_y);
                ui.painter().line_segment(
                    [egui::Pos2::new(px(0.3), ly), egui::Pos2::new(px(0.7), ly)],
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                );
                ui.painter().text(egui::Pos2::new(px(0.7) + 4.0, ly), egui::Align2::LEFT_CENTER, "LEDs", egui::FontId::proportional(12.0), egui::Color32::YELLOW);

                // Knobs
                let knob_r_vis = pw * 0.06;
                let lk = egui::Pos2::new(px(self.zone_knob_left[0]), py(self.zone_knob_left[1]));
                let rk = egui::Pos2::new(px(self.zone_knob_right[0]), py(self.zone_knob_right[1]));
                ui.painter().circle_stroke(lk, knob_r_vis, egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 128, 0)));
                ui.painter().circle_stroke(rk, knob_r_vis, egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 128, 0)));

                // Branding Y line
                let by = py(self.zone_branding_y);
                ui.painter().line_segment(
                    [egui::Pos2::new(px(0.3), by), egui::Pos2::new(px(0.7), by)],
                    egui::Stroke::new(1.0, egui::Color32::from_gray(200)),
                );

                // Labels Y line
                let lby = py(self.zone_labels_y);
                ui.painter().line_segment(
                    [egui::Pos2::new(px(0.2), lby), egui::Pos2::new(px(0.8), lby)],
                    egui::Stroke::new(1.0, egui::Color32::from_gray(150)),
                );

                // Draggable handles for screen rect corners
                let handle_size = 8.0;
                let handles: Vec<(&str, usize, egui::Pos2)> = vec![
                    ("screen", 0, egui::Pos2::new(px(self.zone_screen[0]), py((self.zone_screen[1] + self.zone_screen[3]) / 2.0))), // left
                    ("screen", 1, egui::Pos2::new(px((self.zone_screen[0] + self.zone_screen[2]) / 2.0), py(self.zone_screen[1]))), // top
                    ("screen", 2, egui::Pos2::new(px(self.zone_screen[2]), py((self.zone_screen[1] + self.zone_screen[3]) / 2.0))), // right
                    ("screen", 3, egui::Pos2::new(px((self.zone_screen[0] + self.zone_screen[2]) / 2.0), py(self.zone_screen[3]))), // bottom
                    ("strip", 0, egui::Pos2::new(px(self.zone_strip[0]), py((self.zone_strip[1] + self.zone_strip[3]) / 2.0))), // left
                    ("strip", 1, egui::Pos2::new(px((self.zone_strip[0] + self.zone_strip[2]) / 2.0), py(self.zone_strip[1]))), // top
                    ("strip", 2, egui::Pos2::new(px(self.zone_strip[2]), py((self.zone_strip[1] + self.zone_strip[3]) / 2.0))), // right
                    ("strip", 3, egui::Pos2::new(px((self.zone_strip[0] + self.zone_strip[2]) / 2.0), py(self.zone_strip[3]))), // bottom
                    ("controls", 0, egui::Pos2::new(px(self.zone_controls[0]), py((self.zone_controls[1] + self.zone_controls[3]) / 2.0))), // left
                    ("controls", 1, egui::Pos2::new(px((self.zone_controls[0] + self.zone_controls[2]) / 2.0), py(self.zone_controls[1]))),
                    ("controls", 2, egui::Pos2::new(px(self.zone_controls[2]), py((self.zone_controls[1] + self.zone_controls[3]) / 2.0))), // right
                    ("controls", 3, egui::Pos2::new(px((self.zone_controls[0] + self.zone_controls[2]) / 2.0), py(self.zone_controls[3]))),
                    ("foot", 1, egui::Pos2::new(px(self.zone_foot_center[0]), py(self.zone_foot_center[1]))), // center
                    ("leds", 1, egui::Pos2::new(px(0.5), py(self.zone_leds_y))),
                    ("knob_l", 0, egui::Pos2::new(px(self.zone_knob_left[0]), py(self.zone_knob_left[1]))),
                    ("knob_r", 0, egui::Pos2::new(px(self.zone_knob_right[0]), py(self.zone_knob_right[1]))),
                    ("branding", 1, egui::Pos2::new(px(0.5), py(self.zone_branding_y))),
                    ("labels", 1, egui::Pos2::new(px(0.5), py(self.zone_labels_y))),
                ];

                for (zone_name, idx, pos) in &handles {
                    let handle_rect = egui::Rect::from_center_size(*pos, egui::Vec2::splat(handle_size * 2.0));
                    let resp = ui.interact(handle_rect, ui.id().with((*zone_name, *idx, "handle")), egui::Sense::click_and_drag());
                    let color = if resp.dragged() || resp.hovered() { egui::Color32::WHITE } else { egui::Color32::from_gray(200) };
                    ui.painter().rect_filled(
                        egui::Rect::from_center_size(*pos, egui::Vec2::splat(handle_size)),
                        2.0, color,
                    );

                    if resp.dragged() {
                        let delta = resp.drag_delta();
                        let dx = delta.x / pw;
                        let dy = delta.y / ph;
                        match (*zone_name, *idx) {
                            ("screen", 0) => self.zone_screen[0] = (self.zone_screen[0] + dx).clamp(0.0, 0.5),
                            ("screen", 1) => self.zone_screen[1] = (self.zone_screen[1] + dy).clamp(0.0, 0.5),
                            ("screen", 2) => self.zone_screen[2] = (self.zone_screen[2] + dx).clamp(0.5, 1.0),
                            ("screen", 3) => self.zone_screen[3] = (self.zone_screen[3] + dy).clamp(0.2, 0.8),
                            ("strip", 0) => self.zone_strip[0] = (self.zone_strip[0] + dx).clamp(0.0, 0.5),
                            ("strip", 1) => self.zone_strip[1] = (self.zone_strip[1] + dy).clamp(0.3, 0.8),
                            ("strip", 2) => self.zone_strip[2] = (self.zone_strip[2] + dx).clamp(0.5, 1.0),
                            ("strip", 3) => self.zone_strip[3] = (self.zone_strip[3] + dy).clamp(0.4, 0.9),
                            ("controls", 0) => self.zone_controls[0] = (self.zone_controls[0] + dx).clamp(0.0, 0.5),
                            ("controls", 1) => self.zone_controls[1] = (self.zone_controls[1] + dy).clamp(0.4, 0.9),
                            ("controls", 2) => self.zone_controls[2] = (self.zone_controls[2] + dx).clamp(0.5, 1.0),
                            ("controls", 3) => self.zone_controls[3] = (self.zone_controls[3] + dy).clamp(0.4, 0.9),
                            ("foot", 1) => {
                                self.zone_foot_center[0] = (self.zone_foot_center[0] + dx).clamp(0.2, 0.8);
                                self.zone_foot_center[1] = (self.zone_foot_center[1] + dy).clamp(0.5, 0.95);
                            }
                            ("leds", _) => self.zone_leds_y = (self.zone_leds_y + dy).clamp(0.4, 0.9),
                            ("knob_l", _) => {
                                self.zone_knob_left[0] = (self.zone_knob_left[0] + dx).clamp(0.1, 0.5);
                                self.zone_knob_left[1] = (self.zone_knob_left[1] + dy).clamp(0.0, 0.3);
                            }
                            ("knob_r", _) => {
                                self.zone_knob_right[0] = (self.zone_knob_right[0] + dx).clamp(0.5, 0.9);
                                self.zone_knob_right[1] = (self.zone_knob_right[1] + dy).clamp(0.0, 0.3);
                            }
                            ("branding", _) => self.zone_branding_y = (self.zone_branding_y + dy).clamp(0.0, 0.2),
                            ("labels", _) => self.zone_labels_y = (self.zone_labels_y + dy).clamp(0.05, 0.3),
                            _ => {}
                        }
                    }
                }

                // Print current values for copy-paste
                ui.painter().text(
                    egui::Pos2::new(pedal_rect.left() + 4.0, pedal_rect.bottom() - 60.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!(
                        "screen: [{:.3}, {:.3}, {:.3}, {:.3}]\nstrip: [{:.3}, {:.3}, {:.3}, {:.3}]\ncontrols: [{:.3}, {:.3}, {:.3}, {:.3}]\nfoot: [{:.3}, {:.3}] leds: {:.3}\nknob_l: [{:.3}, {:.3}] knob_r: [{:.3}, {:.3}]\nbranding: {:.3} labels: {:.3}",
                        self.zone_screen[0], self.zone_screen[1], self.zone_screen[2], self.zone_screen[3],
                        self.zone_strip[0], self.zone_strip[1], self.zone_strip[2], self.zone_strip[3],
                        self.zone_controls[0], self.zone_controls[1], self.zone_controls[2], self.zone_controls[3],
                        self.zone_foot_center[0], self.zone_foot_center[1], self.zone_leds_y,
                        self.zone_knob_left[0], self.zone_knob_left[1], self.zone_knob_right[0], self.zone_knob_right[1],
                        self.zone_branding_y, self.zone_labels_y,
                    ),
                    egui::FontId::monospace(10.0),
                    egui::Color32::WHITE,
                );
            }

            // ============================================================
            // ZONE: Knob labels + interactive knobs
            // ============================================================

            // Branding
            ui.painter().text(
                egui::Pos2::new(px(0.50), py(self.zone_branding_y)),
                egui::Align2::CENTER_CENTER,
                "Rusty Tuner",
                egui::FontId::new(pw * 0.06, egui::FontFamily::Name("playball".into())),
                egui::Color32::from_rgba_unmultiplied(200, 190, 170, 180),
            );

            // Interactive knobs (disabled during calibration so handles can be dragged)
            let knob_r = pw * 0.07;
            let left_knob_center = egui::Pos2::new(px(self.zone_knob_left[0]), py(self.zone_knob_left[1]));
            let left_knob_rect = egui::Rect::from_center_size(left_knob_center, egui::Vec2::splat(knob_r * 2.0));
            let knob_sense = if self.show_calibration { egui::Sense::hover() } else { egui::Sense::click_and_drag() };
            let left_knob_resp = ui.interact(left_knob_rect, ui.id().with("knob_tuning"), knob_sense);
            if left_knob_resp.dragged() {
                let dy = left_knob_resp.drag_delta().y;
                if dy < -4.0 {
                    self.selected_tuning = (self.selected_tuning + 1).min(self.tunings.len() - 1);
                    self.selected_string = None;
                } else if dy > 4.0 {
                    self.selected_tuning = self.selected_tuning.saturating_sub(1);
                    self.selected_string = None;
                }
            }
            if left_knob_resp.hovered() {
                left_knob_resp.clone().on_hover_text("Drag to change tuning");
            }
            let scroll_tuning = ctx.input(|i| if left_knob_resp.hovered() { i.raw_scroll_delta.y } else { 0.0 });
            if scroll_tuning > 1.0 { self.selected_tuning = self.selected_tuning.saturating_sub(1); self.selected_string = None; }
            else if scroll_tuning < -1.0 { self.selected_tuning = (self.selected_tuning + 1).min(self.tunings.len() - 1); self.selected_string = None; }

            let right_knob_center = egui::Pos2::new(px(self.zone_knob_right[0]), py(self.zone_knob_right[1]));
            let right_knob_rect = egui::Rect::from_center_size(right_knob_center, egui::Vec2::splat(knob_r * 2.0));
            let right_knob_resp = ui.interact(right_knob_rect, ui.id().with("knob_a4"), knob_sense);
            if right_knob_resp.dragged() {
                self.a4_freq = (self.a4_freq - right_knob_resp.drag_delta().y as f64 * 0.3).clamp(420.0, 460.0);
            }
            if right_knob_resp.hovered() {
                right_knob_resp.clone().on_hover_text(format!("A4: {:.1} Hz — drag to adjust", self.a4_freq));
            }
            let scroll_a4 = ctx.input(|i| if right_knob_resp.hovered() { i.raw_scroll_delta.y } else { 0.0 });
            if scroll_a4.abs() > 0.1 { self.a4_freq = (self.a4_freq + scroll_a4 as f64 * 0.1).clamp(420.0, 460.0); }

            // Knob indicators
            let indicator_r = knob_r * 0.50;
            let tuning_frac = self.selected_tuning as f32 / (self.tunings.len() - 1).max(1) as f32;
            let tuning_angle = PI * 0.75 + tuning_frac * PI * 1.5;
            let tip = egui::Pos2::new(left_knob_center.x + indicator_r * tuning_angle.cos(), left_knob_center.y + indicator_r * tuning_angle.sin());
            ui.painter().line_segment([left_knob_center, tip], egui::Stroke::new(2.0, egui::Color32::from_gray(200)));

            let a4_frac = ((self.a4_freq - 420.0) / 40.0) as f32;
            let a4_angle = PI * 0.75 + a4_frac * PI * 1.5;
            let a4_tip = egui::Pos2::new(right_knob_center.x + indicator_r * a4_angle.cos(), right_knob_center.y + indicator_r * a4_angle.sin());
            ui.painter().line_segment([right_knob_center, a4_tip], egui::Stroke::new(2.0, egui::Color32::from_gray(200)));

            // ============================================================
            // ZONE: Screen — uses self.zone_screen
            // ============================================================
            let screen_rect = egui::Rect::from_min_max(
                egui::Pos2::new(px(self.zone_screen[0]), py(self.zone_screen[1])),
                egui::Pos2::new(px(self.zone_screen[2]), py(self.zone_screen[3])),
            );
            ui.painter().rect_filled(screen_rect, 4.0, egui::Color32::from_rgba_unmultiplied(4, 6, 10, 240));

            let mut screen_ui = ui.new_child(egui::UiBuilder::new().max_rect(screen_rect));
            screen_ui.set_clip_rect(screen_rect);
            screen_ui.vertical_centered(|ui| {
                let sh = screen_rect.height();
                let cx = screen_rect.center().x;

                // Fixed layout: text zone = top 22%, gauge+strobe = rest
                let text_zone_h = sh * 0.22;
                let note_size = text_zone_h * 0.55;
                let small_size = (text_zone_h * 0.14).max(8.0);
                let digi_font = egui::FontId::new(note_size, egui::FontFamily::Name("digital7".into()));
                let digi_small = egui::FontId::new(small_size, egui::FontFamily::Name("digital7".into()));

                // Allocate fixed space for text — paint directly so font metrics don't shift layout
                let text_rect = ui.allocate_space(egui::Vec2::new(ui.available_width(), text_zone_h)).1;

                ui.painter().text(
                    text_rect.left_top() + egui::vec2(8.0, 6.0),
                    egui::Align2::LEFT_TOP,
                    self.tunings[self.selected_tuning].name,
                    digi_small.clone(),
                    egui::Color32::from_gray(140),
                );

                ui.painter().text(
                    text_rect.right_top() + egui::vec2(-8.0, 6.0),
                    egui::Align2::RIGHT_TOP,
                    format!("{:.1} Hz", self.a4_freq),
                    digi_small.clone(),
                    egui::Color32::from_gray(140),
                );

                if let Some(ref err) = self.audio_error {
                    ui.painter().text(text_rect.center_top() + egui::vec2(0.0, 4.0), egui::Align2::CENTER_TOP, err,
                        egui::FontId::proportional(small_size), egui::Color32::from_rgb(230, 60, 60));
                }

                let active = self.clarity > 0.01;
                if active {
                    let abs_cents = self.smoothed_cents.abs();
                    let note_color = gauge::cents_color(abs_cents as f32);

                    // Note name + octave
                    let note_y = text_rect.top() + text_zone_h * 0.1;
                    let note_text = format!("{}{}", self.display_note, self.display_octave);
                    ui.painter().text(egui::Pos2::new(cx, note_y), egui::Align2::CENTER_TOP, &note_text, digi_font, note_color);

                    // Frequency + cents on one line
                    let info_y = text_rect.bottom() - small_size * 1.2;
                    let info_text = if abs_cents < 1.0 {
                        format!("{:.1} Hz  IN TUNE", self.detected_freq)
                    } else {
                        let arrow = if self.smoothed_cents > 0.0 { "+" } else { "" };
                        format!("{:.1} Hz  {arrow}{:.1}c", self.detected_freq, self.smoothed_cents)
                    };
                    let info_color = if abs_cents < 3.0 { egui::Color32::from_rgb(0, 220, 100) } else { note_color.gamma_multiply(0.9) };
                    ui.painter().text(egui::Pos2::new(cx, info_y), egui::Align2::CENTER_TOP, &info_text, digi_small, info_color);
                } else {
                    let note_y = text_rect.top() + text_zone_h * 0.1;
                    ui.painter().text(egui::Pos2::new(cx, note_y), egui::Align2::CENTER_TOP, "--", digi_font, egui::Color32::from_gray(40));
                    let info_y = text_rect.bottom() - small_size * 1.2;
                    ui.painter().text(egui::Pos2::new(cx, info_y), egui::Align2::CENTER_TOP, "Play a note", digi_small, egui::Color32::from_gray(55));
                }

                gauge::draw_gauge(ui, self.smoothed_cents, self.clarity, self.needle_angle, time);
                gauge::draw_strobe(ui, self.smoothed_cents, self.clarity, time);
            });

            ui.painter().rect_stroke(screen_rect, 4.0, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(12)), egui::StrokeKind::Inside);

            // ============================================================
            // ZONE: Strip — string buttons
            // ============================================================
            let strip_rect = egui::Rect::from_min_max(
                egui::Pos2::new(px(self.zone_strip[0]), py(self.zone_strip[1])),
                egui::Pos2::new(px(self.zone_strip[2]), py(self.zone_strip[3])),
            );
            let tuning = self.tunings[self.selected_tuning].clone();
            let num_strings = tuning.strings.len();
            let strip_w = strip_rect.width();
            let auto_w = strip_w * 0.12;
            let btn_gap = 3.0;
            let sbtn_w = ((strip_w - auto_w - btn_gap * num_strings as f32) / num_strings as f32).max(30.0);
            let btn_h = strip_rect.height();
            let mut bx = strip_rect.left();

            // Auto button
            let auto_active = self.selected_string.is_none();
            let auto_rect = egui::Rect::from_min_size(egui::Pos2::new(bx, strip_rect.top()), egui::Vec2::new(auto_w, btn_h));
            let auto_resp = ui.interact(auto_rect, ui.id().with("auto_btn"), egui::Sense::click());
            let glow_color = if auto_active { Some(egui::Color32::from_rgb(220, 110, 40)) } else { None };
            let dr = draw_3d_button(ui.painter(), auto_rect, auto_resp.is_pointer_button_down_on(), auto_resp.hovered(), glow_color, 6.0);
            ui.painter().text(dr.center(), egui::Align2::CENTER_CENTER, "AUTO", egui::FontId::proportional(pw * 0.018),
                if auto_active { egui::Color32::from_rgb(255, 180, 100) } else { egui::Color32::from_gray(140) });
            if auto_resp.clicked() { self.selected_string = None; }
            bx += auto_w + btn_gap;

            for (i, s) in tuning.strings.iter().enumerate() {
                let is_selected = self.selected_string == Some(i);
                let is_playing = self.playing_string == Some(i);
                let is_auto = self.selected_string.is_none() && self.auto_string_idx == Some(i) && self.clarity > 0.01;
                let btn_rect = egui::Rect::from_min_size(egui::Pos2::new(bx, strip_rect.top()), egui::Vec2::new(sbtn_w, btn_h));
                let note_rect = egui::Rect::from_min_max(btn_rect.min, egui::Pos2::new(btn_rect.right(), btn_rect.bottom() - btn_h * 0.28));
                let play_rect = egui::Rect::from_min_max(egui::Pos2::new(btn_rect.left(), btn_rect.bottom() - btn_h * 0.28), btn_rect.max);
                let note_resp = ui.interact(note_rect, ui.id().with(("str", i)), egui::Sense::click());
                let play_resp = ui.interact(play_rect, ui.id().with(("play", i)), egui::Sense::click());
                let pressed = note_resp.is_pointer_button_down_on() || play_resp.is_pointer_button_down_on();
                let hovered = note_resp.hovered() || play_resp.hovered();
                
                let glow_color = if is_selected {
                    Some(egui::Color32::from_rgb(220, 110, 40))
                } else if is_auto {
                    Some(gauge::cents_color(self.auto_string_cents.abs() as f32))
                } else {
                    None
                };
                
                let dr = draw_3d_button(ui.painter(), btn_rect, pressed, hovered, glow_color, 6.0);
                
                let target_hz = s.frequency(self.a4_freq);
                let nc = if is_selected { egui::Color32::from_rgb(255, 180, 100) } else if is_auto { glow_color.unwrap() } else { egui::Color32::from_gray(160) };
                
                ui.painter().text(egui::Pos2::new(dr.center().x, dr.top() + btn_h * 0.32), egui::Align2::CENTER_CENTER, s.name, egui::FontId::proportional(pw * 0.035), nc);
                
                let pc = if is_playing { egui::Color32::from_rgb(80, 240, 100) } else if play_resp.hovered() { egui::Color32::from_gray(140) } else { egui::Color32::from_gray(80) };
                ui.painter().text(egui::Pos2::new(dr.center().x, dr.bottom() - btn_h * 0.35), egui::Align2::CENTER_CENTER, "\u{25B6}", egui::FontId::proportional(pw * 0.022), pc);
                
                let led_w = sbtn_w * 0.4;
                let led_h = btn_h * 0.06;
                let led_rect = egui::Rect::from_center_size(egui::Pos2::new(dr.center().x, dr.bottom() - btn_h * 0.14), egui::Vec2::new(led_w, led_h));
                let led_color = if is_playing { egui::Color32::from_rgb(0, 255, 100) } else { egui::Color32::from_rgb(20, 25, 20) };
                ui.painter().rect_filled(led_rect, 1.0, led_color);
                if note_resp.hovered() { note_resp.clone().on_hover_text(format!("{} — {:.1} Hz", s.name, target_hz)); }
                if note_resp.clicked() { self.selected_string = Some(i); }
                if play_resp.clicked() {
                    if is_playing { self.tone_state.stop(); self.playing_string = None; }
                    else { self.tone_state.set_frequency(target_hz as f32); self.tone_state.set_volume(self.tone_volume); self.playing_string = Some(i); }
                }
                bx += sbtn_w + btn_gap;
            }

            // ============================================================
            // ZONE: Controls (vol slider + LED meter)
            // ============================================================
            let controls_rect = egui::Rect::from_min_max(
                egui::Pos2::new(px(self.zone_controls[0]), py(self.zone_controls[1])),
                egui::Pos2::new(px(self.zone_controls[2]), py(self.zone_controls[3])),
            );

            // Calculate internal bounds for Volume + Vertical Meter
            let margin = pw * 0.015;
            let meter_w = pw * 0.055;
            let slider_area_r = controls_rect.right() - meter_w - margin * 3.0;

            // --- VOLUME SLIDER ---
            let vol_y = controls_rect.top() + controls_rect.height() * 0.4;
            let tl = controls_rect.left() + pw * 0.12;
            let tr = slider_area_r;

            // "VOL" Label
            let vol_font = egui::FontId::new(pw * 0.024, egui::FontFamily::Name("digital7".into()));
            ui.painter().text(
                egui::Pos2::new(controls_rect.left() + pw * 0.025, vol_y),
                egui::Align2::LEFT_CENTER,
                "VOL",
                vol_font,
                egui::Color32::from_gray(130),
            );

            // Slider interaction
            let hit = egui::Rect::from_min_max(
                egui::Pos2::new(tl, vol_y - pw * 0.04),
                egui::Pos2::new(tr, vol_y + pw * 0.04),
            );
            let sr = ui.interact(hit, ui.id().with("vol_slider"), egui::Sense::click_and_drag());
            if sr.dragged() || sr.clicked() {
                if let Some(pos) = sr.interact_pointer_pos() {
                    self.tone_volume = ((pos.x - tl) / (tr - tl)).clamp(0.0, 1.0);
                    self.tone_state.set_volume(self.tone_volume);
                }
            }

            // Recessed track groove
            let groove_h = pw * 0.01;
            let groove_r = egui::Rect::from_min_size(
                egui::Pos2::new(tl, vol_y - groove_h / 2.0),
                egui::Vec2::new(tr - tl, groove_h),
            );
            ui.painter().rect_filled(groove_r, groove_h / 2.0, egui::Color32::from_rgb(8, 8, 10));
            ui.painter().rect_stroke(groove_r, groove_h / 2.0, egui::Stroke::new(0.5, egui::Color32::from_black_alpha(200)), egui::StrokeKind::Inside);

            // Segmented fill — amber with subtle gradient
            let num_segments: usize = 28;
            let seg_gap = 1.5_f32;
            let seg_w = ((tr - tl) - seg_gap * (num_segments - 1) as f32) / num_segments as f32;
            let fill_idx = (self.tone_volume * num_segments as f32).round() as usize;
            let seg_h = pw * 0.014;

            for i in 0..num_segments {
                let x = tl + i as f32 * (seg_w + seg_gap);
                let seg_r = egui::Rect::from_min_size(
                    egui::Pos2::new(x, vol_y - seg_h / 2.0),
                    egui::Vec2::new(seg_w, seg_h),
                );

                if i < fill_idx {
                    let frac = i as f32 / num_segments as f32;
                    // Color: warm amber gradient, brighter toward the right
                    let r = (200.0 + 55.0 * frac) as u8;
                    let g = (130.0 + 30.0 * frac - 60.0 * frac * frac) as u8;
                    let b = (10.0 + 15.0 * frac) as u8;
                    let c = egui::Color32::from_rgb(r, g, b);
                    ui.painter().rect_filled(seg_r, 1.0, c);

                    // Top gloss line
                    ui.painter().line_segment(
                        [seg_r.left_top() + egui::vec2(0.5, 0.5), seg_r.right_top() + egui::vec2(-0.5, 0.5)],
                        egui::Stroke::new(0.5, egui::Color32::from_white_alpha((40.0 + 30.0 * frac) as u8)),
                    );

                    // Glow halo on the last few lit segments
                    if i >= fill_idx.saturating_sub(3) {
                        let glow_t = 1.0 - (fill_idx - 1 - i) as f32 / 3.0;
                        ui.painter().rect_filled(
                            seg_r.expand(1.5),
                            2.0,
                            c.linear_multiply(0.15 * glow_t),
                        );
                    }
                } else {
                    // Unlit: dark recessed segments
                    ui.painter().rect_filled(seg_r, 1.0, egui::Color32::from_rgb(22, 18, 12));
                    ui.painter().rect_stroke(
                        seg_r, 1.0,
                        egui::Stroke::new(0.5, egui::Color32::from_black_alpha(180)),
                        egui::StrokeKind::Inside,
                    );
                }
            }

            // Tick marks + number labels
            let tick_positions: &[(f32, &str)] = &[
                (0.0, "0"), (0.143, ""), (0.286, "3"), (0.429, ""),
                (0.571, "5"), (0.714, ""), (0.857, "8"), (1.0, "10"),
            ];
            for &(frac, label) in tick_positions {
                let tx = tl + frac * (tr - tl);
                let is_major = !label.is_empty();
                let tick_len = if is_major { pw * 0.025 } else { pw * 0.015 };
                let tick_color = if is_major { egui::Color32::from_gray(90) } else { egui::Color32::from_gray(55) };

                ui.painter().line_segment(
                    [
                        egui::Pos2::new(tx, vol_y + seg_h / 2.0 + 2.0),
                        egui::Pos2::new(tx, vol_y + seg_h / 2.0 + 2.0 + tick_len),
                    ],
                    egui::Stroke::new(if is_major { 1.0 } else { 0.5 }, tick_color),
                );

                if is_major {
                    ui.painter().text(
                        egui::Pos2::new(tx, vol_y + seg_h / 2.0 + 3.0 + tick_len),
                        egui::Align2::CENTER_TOP,
                        label,
                        egui::FontId::proportional(pw * 0.018),
                        egui::Color32::from_gray(100),
                    );
                }
            }

            // --- Knob thumb ---
            let knob_x = tl + self.tone_volume * (tr - tl);
            let k_center = egui::Pos2::new(knob_x, vol_y);
            let k_radius = pw * 0.026;

            // Drop shadow
            ui.painter().circle_filled(
                k_center + egui::vec2(0.0, 2.5),
                k_radius + 1.5,
                egui::Color32::from_black_alpha(140),
            );

            // Outer dark ring
            ui.painter().circle_filled(k_center, k_radius, egui::Color32::from_rgb(42, 44, 48));
            ui.painter().circle_stroke(
                k_center, k_radius,
                egui::Stroke::new(0.8, egui::Color32::from_rgb(18, 20, 22)),
            );

            // Inner metallic disc with gradient simulation
            let k_inner = k_radius * 0.72;
            ui.painter().circle_filled(k_center, k_inner, egui::Color32::from_rgb(95, 100, 108));

            // Top-half highlight arc (simulate lighting from above)
            let highlight_segs = 16;
            for i in 0..highlight_segs {
                let a0 = PI + (i as f32 / highlight_segs as f32) * PI;
                let a1 = PI + ((i + 1) as f32 / highlight_segs as f32) * PI;
                let p0 = k_center + egui::vec2(a0.cos() * k_inner, a0.sin() * k_inner);
                let p1 = k_center + egui::vec2(a1.cos() * k_inner, a1.sin() * k_inner);
                ui.painter().line_segment(
                    [p0, p1],
                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70)),
                );
            }

            // Knurling (radial grooves)
            let knurl_count = 20;
            for i in 0..knurl_count {
                let a = i as f32 * 2.0 * PI / knurl_count as f32;
                let p1 = k_center + egui::vec2(a.cos() * (k_inner + 0.5), a.sin() * (k_inner + 0.5));
                let p2 = k_center + egui::vec2(a.cos() * (k_radius - 0.5), a.sin() * (k_radius - 0.5));
                ui.painter().line_segment(
                    [p1, p2],
                    egui::Stroke::new(0.8, egui::Color32::from_gray(28)),
                );
            }

            // Center pointer line (points up)
            let ptr_inner = k_center + egui::vec2(0.0, -k_inner * 0.3);
            let ptr_outer = k_center + egui::vec2(0.0, -k_radius + 1.0);
            ui.painter().line_segment(
                [ptr_inner, ptr_outer],
                egui::Stroke::new(1.5, egui::Color32::from_rgb(220, 140, 30)),
            );

            // --- VERTICAL LED VU METER ---
            let meter_rect = egui::Rect::from_min_max(
                egui::Pos2::new(controls_rect.right() - meter_w - margin, controls_rect.top() + margin),
                egui::Pos2::new(controls_rect.right() - margin, controls_rect.bottom() - margin),
            );

            // Recessed bezel
            ui.painter().rect_filled(meter_rect, 3.0, egui::Color32::from_rgb(6, 7, 9));
            ui.painter().rect_stroke(
                meter_rect, 3.0,
                egui::Stroke::new(1.0, egui::Color32::from_black_alpha(220)),
                egui::StrokeKind::Inside,
            );
            ui.painter().rect_stroke(
                meter_rect, 3.0,
                egui::Stroke::new(0.5, egui::Color32::from_white_alpha(20)),
                egui::StrokeKind::Outside,
            );

            // LED blocks (bottom to top)
            let vu_leds: usize = 18;
            let gap_y = 1.5_f32;
            let pad_y = 3.0_f32;
            let pad_x = 3.0_f32;
            let led_h = (meter_rect.height() - pad_y * 2.0 - gap_y * (vu_leds - 1) as f32) / vu_leds as f32;
            let led_w = meter_rect.width() - pad_x * 2.0;

            let db = if self.rms_level > 0.0 {
                (20.0 * self.rms_level.log10()).clamp(-60.0, 0.0)
            } else {
                -60.0
            };
            let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            let active_vu = (level * vu_leds as f32).ceil() as usize;
            let peak_led = (self.vu_peak * vu_leds as f32).round() as usize;

            for i in 0..vu_leds {
                let frac = i as f32 / (vu_leds - 1) as f32;

                // Smooth color gradient: green → yellow → orange → red
                let base_c = if frac < 0.55 {
                    let t = frac / 0.55;
                    let r = (t * 180.0) as u8;
                    let g = (180.0 + t * 40.0) as u8;
                    let b = (80.0 - t * 60.0) as u8;
                    egui::Color32::from_rgb(r, g, b)
                } else if frac < 0.78 {
                    let t = (frac - 0.55) / 0.23;
                    let r = (180.0 + t * 60.0) as u8;
                    let g = (220.0 - t * 40.0) as u8;
                    egui::Color32::from_rgb(r, g, 20)
                } else {
                    let t = (frac - 0.78) / 0.22;
                    let r = (240.0 + t * 15.0) as u8;
                    let g = (180.0 - t * 130.0) as u8;
                    egui::Color32::from_rgb(r, g, (20.0 + t * 20.0) as u8)
                };

                let is_on = i < active_vu;
                let is_peak = i == peak_led.saturating_sub(1) && peak_led > active_vu && self.vu_peak > 0.01;

                let y = meter_rect.bottom() - pad_y - (i as f32 + 1.0) * led_h - i as f32 * gap_y;
                let led_r = egui::Rect::from_min_size(
                    egui::Pos2::new(meter_rect.left() + pad_x, y),
                    egui::Vec2::new(led_w, led_h),
                );

                if is_on {
                    // Lit LED
                    ui.painter().rect_filled(led_r, 1.0, base_c);

                    // Hot center highlight
                    let inner = led_r.shrink2(egui::vec2(led_w * 0.15, 0.5));
                    let bright = egui::Color32::from_rgb(
                        (base_c.r() as f32 * 0.5 + 128.0) as u8,
                        (base_c.g() as f32 * 0.5 + 128.0) as u8,
                        (base_c.b() as f32 * 0.3 + 80.0).min(255.0) as u8,
                    );
                    ui.painter().rect_filled(inner, 0.5, bright.linear_multiply(0.5));

                    // Glow emission
                    ui.painter().rect_filled(
                        led_r.expand(1.5),
                        2.0,
                        base_c.linear_multiply(0.15),
                    );

                    // Top gloss
                    ui.painter().line_segment(
                        [led_r.left_top() + egui::vec2(1.0, 0.5), led_r.right_top() + egui::vec2(-1.0, 0.5)],
                        egui::Stroke::new(0.5, egui::Color32::from_white_alpha(60)),
                    );
                } else if is_peak {
                    // Peak hold indicator — bright outline with dim fill
                    ui.painter().rect_filled(led_r, 1.0, base_c.linear_multiply(0.6));
                    ui.painter().rect_filled(
                        led_r.expand(1.0),
                        2.0,
                        base_c.linear_multiply(0.1),
                    );
                } else {
                    // Off LED — dim with inner shadow
                    ui.painter().rect_filled(led_r, 1.0, base_c.linear_multiply(0.07));
                    ui.painter().rect_stroke(
                        led_r, 1.0,
                        egui::Stroke::new(0.5, egui::Color32::from_black_alpha(180)),
                        egui::StrokeKind::Inside,
                    );
                }
            }

            // dB labels alongside meter
            let label_x = meter_rect.left() - 2.0;
            let db_labels: &[(f32, &str)] = &[
                (0.0, "-60"), (0.33, "-40"), (0.55, "-20"),
                (0.78, "-10"), (0.92, "-3"), (1.0, "0"),
            ];
            for &(frac, label) in db_labels {
                let led_i = (frac * (vu_leds - 1) as f32).round();
                let y = meter_rect.bottom() - pad_y - (led_i + 0.5) * led_h - led_i * gap_y;
                ui.painter().text(
                    egui::Pos2::new(label_x, y),
                    egui::Align2::RIGHT_CENTER,
                    label,
                    egui::FontId::proportional(pw * 0.014),
                    egui::Color32::from_gray(75),
                );
            }

            // ============================================================
            // ZONE: Footswitch
            // ============================================================
            let fc = egui::Pos2::new(px(self.zone_foot_center[0]), py(self.zone_foot_center[1]));
            let fr = pw * self.zone_foot_r;
            let foot_resp = ui.interact(egui::Rect::from_center_size(fc, egui::Vec2::splat(fr * 2.0)), ui.id().with("footswitch"), egui::Sense::click());
            if foot_resp.clicked() {
                if self.playing_string.is_some() { self.tone_state.stop(); self.playing_string = None; }
                else {
                    let idx = self.selected_string.unwrap_or(0);
                    let t = &self.tunings[self.selected_tuning];
                    if idx < t.strings.len() { self.tone_state.set_frequency(t.strings[idx].frequency(self.a4_freq) as f32); self.tone_state.set_volume(self.tone_volume); self.playing_string = Some(idx); }
                }
            }

            // Status LEDs
            let led_base_y = py(self.zone_leds_y);
            let ls = pw * 0.025;
            let lbx = px(0.50) - ls;
            let green_on = self.clarity > 0.01;
            ui.painter().circle_filled(egui::Pos2::new(lbx, led_base_y), 3.0, if green_on { egui::Color32::from_rgb(0, 255, 80) } else { egui::Color32::from_rgb(0, 40, 15) });
            if green_on { ui.painter().circle_filled(egui::Pos2::new(lbx, led_base_y), 7.0, egui::Color32::from_rgba_unmultiplied(0, 255, 80, 25)); }
            let amber_on = self.playing_string.is_some();
            ui.painter().circle_filled(egui::Pos2::new(lbx + ls * 2.0, led_base_y), 3.0, if amber_on { egui::Color32::from_rgb(255, 180, 0) } else { egui::Color32::from_rgb(50, 35, 0) });
            if amber_on { ui.painter().circle_filled(egui::Pos2::new(lbx + ls * 2.0, led_base_y), 7.0, egui::Color32::from_rgba_unmultiplied(255, 180, 0, 25)); }

            // Input device selector
            if !self.input_devices.is_empty() {
                let iy = py(0.92);
                let cn = if self.selected_device == 0 { "Default" } else { self.input_devices.get(self.selected_device - 1).map(|s| s.as_str()).unwrap_or("Default") };
                let ir = egui::Rect::from_center_size(egui::Pos2::new(px(0.5), iy), egui::Vec2::new(pw * 0.6, pw * 0.04));
                let irsp = ui.interact(ir, ui.id().with("input_sel"), egui::Sense::click());
                ui.painter().text(ir.center(), egui::Align2::CENTER_CENTER, format!("Input: {}", cn), egui::FontId::proportional(pw * 0.024),
                    if irsp.hovered() { egui::Color32::from_gray(180) } else { egui::Color32::from_gray(100) });
                if irsp.clicked() {
                    self.selected_device = (self.selected_device + 1) % (self.input_devices.len() + 1);
                    let dn = if self.selected_device == 0 { None } else { self.input_devices.get(self.selected_device - 1).map(|s| s.as_str()) };
                    match AudioEngine::start(Arc::clone(&self.shared_buffer), dn) {
                        Ok(e) => { self._audio_engine = Some(e); self.audio_error = None; }
                        Err(e) => { self._audio_engine = None; self.audio_error = Some(format!("Audio error: {e}")); }
                    }
                }
            }
        });
    }
}
