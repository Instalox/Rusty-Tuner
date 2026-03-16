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

    // Waveform display
    waveform_samples: Vec<f32>,
    show_waveform: bool,

    // Input device
    input_devices: Vec<String>,
    selected_device: usize, // 0 = "Default", 1+ = named devices
}

impl TunerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Dark theme
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::from_rgb(22, 24, 30);
        visuals.window_fill = egui::Color32::from_rgb(22, 24, 30);
        visuals.extreme_bg_color = egui::Color32::from_rgb(16, 18, 22);
        visuals.faint_bg_color = egui::Color32::from_rgb(30, 33, 40);
        visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(35, 38, 45);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(40, 44, 52);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(50, 55, 65);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(55, 60, 72);
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
            Ok(gen) => Some(gen),
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

            waveform_samples: Vec::new(),
            show_waveform: false,

            input_devices,
            selected_device: 0,
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

        // ---- UI ----
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(6.0);

                // Error banner
                if let Some(ref err) = self.audio_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 60, 60), err);
                    ui.add_space(4.0);
                }

                // Header bar — styled settings row
                let header_frame = egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(26, 28, 35))
                    .corner_radius(8.0)
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(38)))
                    .inner_margin(egui::Margin::symmetric(10, 7));

                header_frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;

                        // Tuning label
                        ui.label(
                            egui::RichText::new("TUNING")
                                .size(9.0)
                                .color(egui::Color32::from_gray(80)),
                        );

                        egui::ComboBox::from_id_salt("tuning_select")
                            .selected_text(
                                egui::RichText::new(self.tunings[self.selected_tuning].name)
                                    .size(12.0),
                            )
                            .width(125.0)
                            .show_ui(ui, |ui| {
                                for (i, t) in self.tunings.iter().enumerate() {
                                    let r = ui.selectable_value(&mut self.selected_tuning, i, t.name);
                                    if r.changed() {
                                        self.selected_string = None;
                                    }
                                }
                            });

                        ui.add_space(4.0);

                        // Subtle vertical divider
                        let (div_rect, _) = ui.allocate_exact_size(
                            egui::Vec2::new(1.0, 18.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().line_segment(
                            [div_rect.left_top(), div_rect.left_bottom()],
                            egui::Stroke::new(1.0, egui::Color32::from_gray(45)),
                        );

                        ui.add_space(4.0);

                        ui.label(
                            egui::RichText::new("A4")
                                .size(10.0)
                                .color(egui::Color32::from_gray(90)),
                        );
                        ui.add(
                            egui::Slider::new(&mut self.a4_freq, 420.0..=460.0)
                                .suffix(" Hz")
                                .fixed_decimals(1)
                                .text(""),
                        );
                    });
                });

                // Input device selector
                if !self.input_devices.is_empty() {
                    ui.add_space(3.0);
                    let device_frame = egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(26, 28, 35))
                        .corner_radius(6.0)
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(38)))
                        .inner_margin(egui::Margin::symmetric(10, 5));

                    device_frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;

                            ui.label(
                                egui::RichText::new("INPUT")
                                    .size(9.0)
                                    .color(egui::Color32::from_gray(80)),
                            );

                            let current_name = if self.selected_device == 0 {
                                "Default".to_string()
                            } else {
                                self.input_devices
                                    .get(self.selected_device - 1)
                                    .cloned()
                                    .unwrap_or("Default".to_string())
                            };

                            let mut changed = false;
                            egui::ComboBox::from_id_salt("input_device")
                                .selected_text(
                                    egui::RichText::new(&current_name).size(12.0),
                                )
                                .width(ui.available_width().min(280.0))
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_value(&mut self.selected_device, 0, "Default")
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                    for (i, name) in self.input_devices.iter().enumerate() {
                                        if ui
                                            .selectable_value(
                                                &mut self.selected_device,
                                                i + 1,
                                                name,
                                            )
                                            .changed()
                                        {
                                            changed = true;
                                        }
                                    }
                                });

                            if changed {
                                let device_name = if self.selected_device == 0 {
                                    None
                                } else {
                                    self.input_devices.get(self.selected_device - 1).map(|s| s.as_str())
                                };
                                match AudioEngine::start(
                                    Arc::clone(&self.shared_buffer),
                                    device_name,
                                ) {
                                    Ok(engine) => {
                                        self._audio_engine = Some(engine);
                                        self.audio_error = None;
                                    }
                                    Err(e) => {
                                        self._audio_engine = None;
                                        self.audio_error = Some(format!("Audio error: {e}"));
                                    }
                                }
                            }
                        });
                    });
                }

                ui.add_space(8.0);

                // ---- Note display ----
                let active = self.clarity > 0.01;

                if active {
                    let abs_cents = self.smoothed_cents.abs();
                    let note_color = gauge::cents_color(abs_cents as f32);

                    // Note name (big) + octave (small subscript)
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 120.0) / 2.0); // center roughly
                        ui.label(
                            egui::RichText::new(&self.display_note)
                                .size(80.0)
                                .color(note_color)
                                .strong(),
                        );
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::BOTTOM), |ui| {
                            ui.add_space(0.0);
                            ui.label(
                                egui::RichText::new(format!("{}", self.display_octave))
                                    .size(32.0)
                                    .color(note_color.gamma_multiply(0.7)),
                            );
                        });
                    });

                    // Frequency + cents
                    ui.label(
                        egui::RichText::new(format!("{:.1} Hz", self.detected_freq))
                            .size(16.0)
                            .color(egui::Color32::from_gray(140)),
                    );

                    // Cents with direction indicator
                    let cents_text = if abs_cents < 1.0 {
                        "IN TUNE".to_string()
                    } else {
                        let arrow = if self.smoothed_cents > 0.0 { "+" } else { "" };
                        format!("{arrow}{:.1} cents", self.smoothed_cents)
                    };
                    ui.label(
                        egui::RichText::new(cents_text)
                            .size(14.0)
                            .color(if abs_cents < 3.0 {
                                egui::Color32::from_rgb(0, 220, 100)
                            } else {
                                note_color.gamma_multiply(0.9)
                            }),
                    );
                } else {
                    // Idle state
                    ui.label(
                        egui::RichText::new("—")
                            .size(80.0)
                            .color(egui::Color32::from_gray(55))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Play a note")
                            .size(14.0)
                            .color(egui::Color32::from_gray(70)),
                    );
                    ui.add_space(17.0); // keep layout stable
                }

                ui.add_space(8.0);

                // Gauge
                let time = ctx.input(|i| i.time);
                gauge::draw_gauge(ui, self.smoothed_cents, self.clarity, self.needle_angle, time);

                ui.add_space(6.0);

                // Strobe tuner
                gauge::draw_strobe(ui, self.smoothed_cents, self.clarity, time);

                ui.add_space(8.0);

                // String buttons — custom painted
                let tuning = self.tunings[self.selected_tuning].clone();
                let num_strings = tuning.strings.len();
                let total_width = ui.available_width().min(370.0);
                let auto_width = 44.0;
                let sep_width = 12.0;
                let btn_gap = 5.0;
                let string_area = total_width - auto_width - sep_width - btn_gap * (num_strings as f32 - 1.0);
                let string_btn_width = (string_area / num_strings as f32).max(40.0);
                let btn_height = 42.0;

                let (row_rect, _) = ui.allocate_exact_size(
                    egui::Vec2::new(total_width, btn_height),
                    egui::Sense::hover(),
                );

                // Center the row
                let row_left = row_rect.center().x - total_width / 2.0;
                let mut x = row_left;

                // Auto button
                let auto_active = self.selected_string.is_none();
                let auto_rect = egui::Rect::from_min_size(
                    egui::Pos2::new(x, row_rect.top()),
                    egui::Vec2::new(auto_width, btn_height),
                );
                let auto_response = ui.interact(auto_rect, ui.id().with("auto_btn"), egui::Sense::click());
                let auto_hovered = auto_response.hovered();

                let auto_fill = if auto_active {
                    egui::Color32::from_rgb(40, 55, 70)
                } else if auto_hovered {
                    egui::Color32::from_rgb(40, 43, 52)
                } else {
                    egui::Color32::from_rgb(30, 33, 40)
                };

                ui.painter().rect_filled(auto_rect, 8.0, auto_fill);
                if auto_active {
                    ui.painter().rect_stroke(auto_rect, 8.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(70, 130, 200).gamma_multiply(0.6)), egui::StrokeKind::Outside);
                } else {
                    ui.painter().rect_stroke(auto_rect, 8.0, egui::Stroke::new(1.0, egui::Color32::from_gray(45)), egui::StrokeKind::Outside);
                }

                ui.painter().text(
                    auto_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "AUTO",
                    egui::FontId::proportional(11.0),
                    if auto_active {
                        egui::Color32::from_rgb(130, 180, 240)
                    } else {
                        egui::Color32::from_gray(120)
                    },
                );

                if auto_response.clicked() {
                    self.selected_string = None;
                }

                x += auto_width + sep_width;

                // String buttons
                for (i, s) in tuning.strings.iter().enumerate() {
                    let is_selected = self.selected_string == Some(i);
                    let is_playing = self.playing_string == Some(i);
                    let is_auto_detected = self.selected_string.is_none()
                        && self.auto_string_idx == Some(i)
                        && self.clarity > 0.01;

                    let btn_rect = egui::Rect::from_min_size(
                        egui::Pos2::new(x, row_rect.top()),
                        egui::Vec2::new(string_btn_width, btn_height),
                    );

                    // Split into note area (top) and play area (bottom strip)
                    let note_rect = egui::Rect::from_min_max(
                        btn_rect.min,
                        egui::Pos2::new(btn_rect.right(), btn_rect.bottom() - 13.0),
                    );
                    let play_rect = egui::Rect::from_min_max(
                        egui::Pos2::new(btn_rect.left(), btn_rect.bottom() - 13.0),
                        btn_rect.max,
                    );

                    let note_response = ui.interact(note_rect, ui.id().with(("string", i)), egui::Sense::click());
                    let play_response = ui.interact(play_rect, ui.id().with(("play", i)), egui::Sense::click());
                    let hovered = note_response.hovered() || play_response.hovered();

                    let target_hz = s.frequency(self.a4_freq);

                    // Background fill
                    let fill = if is_selected {
                        egui::Color32::from_rgb(40, 55, 70)
                    } else if is_auto_detected {
                        let c = gauge::cents_color(self.auto_string_cents.abs() as f32);
                        egui::Color32::from_rgb(
                            (c.r() as f32 * 0.15) as u8 + 22,
                            (c.g() as f32 * 0.15) as u8 + 24,
                            (c.b() as f32 * 0.15) as u8 + 28,
                        )
                    } else if hovered {
                        egui::Color32::from_rgb(40, 43, 52)
                    } else {
                        egui::Color32::from_rgb(30, 33, 40)
                    };

                    ui.painter().rect_filled(btn_rect, 8.0, fill);

                    // Border
                    let border_color = if is_selected {
                        egui::Color32::from_rgb(70, 130, 200).gamma_multiply(0.6)
                    } else if is_auto_detected {
                        gauge::cents_color(self.auto_string_cents.abs() as f32).gamma_multiply(0.4)
                    } else {
                        egui::Color32::from_gray(45)
                    };
                    let border_width = if is_selected || is_auto_detected { 1.5 } else { 1.0 };
                    ui.painter().rect_stroke(btn_rect, 8.0, egui::Stroke::new(border_width, border_color), egui::StrokeKind::Outside);

                    // Note name
                    let note_color = if is_selected {
                        egui::Color32::WHITE
                    } else if is_auto_detected {
                        gauge::cents_color(self.auto_string_cents.abs() as f32)
                    } else {
                        egui::Color32::from_gray(150)
                    };

                    ui.painter().text(
                        egui::Pos2::new(note_rect.center().x, note_rect.center().y - 1.0),
                        egui::Align2::CENTER_CENTER,
                        s.name,
                        egui::FontId::proportional(15.0),
                        note_color,
                    );

                    // Divider line between note and play area
                    ui.painter().line_segment(
                        [
                            egui::Pos2::new(btn_rect.left() + 4.0, play_rect.top()),
                            egui::Pos2::new(btn_rect.right() - 4.0, play_rect.top()),
                        ],
                        egui::Stroke::new(0.5, egui::Color32::from_gray(50)),
                    );

                    // Play indicator
                    let play_color = if is_playing {
                        egui::Color32::from_rgb(0, 200, 90)
                    } else if play_response.hovered() {
                        egui::Color32::from_gray(120)
                    } else {
                        egui::Color32::from_gray(70)
                    };

                    let play_icon = if is_playing { "\u{25A0}" } else { "\u{25B6}" };
                    ui.painter().text(
                        egui::Pos2::new(play_rect.center().x, play_rect.center().y + 0.5),
                        egui::Align2::CENTER_CENTER,
                        play_icon,
                        egui::FontId::proportional(8.0),
                        play_color,
                    );

                    // Hover tooltip
                    if note_response.hovered() {
                        note_response.clone().on_hover_text(format!("{} — {:.1} Hz", s.name, target_hz));
                    }

                    // Click handling
                    if note_response.clicked() {
                        self.selected_string = Some(i);
                    }

                    if play_response.clicked() {
                        if is_playing {
                            self.tone_state.stop();
                            self.playing_string = None;
                        } else {
                            self.tone_state.set_frequency(target_hz as f32);
                            self.tone_state.set_volume(self.tone_volume);
                            self.playing_string = Some(i);
                        }
                    }

                    x += string_btn_width + btn_gap;
                }

                // Tone volume slider (only visible when playing)
                if self.playing_string.is_some() {
                    ui.add_space(8.0);
                    let vol_width = total_width;
                    let vol_height = 20.0;
                    let (vol_row, _) = ui.allocate_exact_size(
                        egui::Vec2::new(vol_width, vol_height),
                        egui::Sense::hover(),
                    );
                    let vol_left = vol_row.center().x - vol_width / 2.0;

                    // Volume label
                    ui.painter().text(
                        egui::Pos2::new(vol_left + 16.0, vol_row.center().y),
                        egui::Align2::LEFT_CENTER,
                        "VOL",
                        egui::FontId::proportional(10.0),
                        egui::Color32::from_gray(90),
                    );

                    // Volume slider area
                    let slider_left = vol_left + 42.0;
                    let slider_right = vol_row.right() - 50.0;
                    let slider_rect = egui::Rect::from_min_max(
                        egui::Pos2::new(slider_left, vol_row.center().y - 3.0),
                        egui::Pos2::new(slider_right, vol_row.center().y + 3.0),
                    );
                    let slider_response = ui.interact(
                        egui::Rect::from_min_max(
                            egui::Pos2::new(slider_left, vol_row.top()),
                            egui::Pos2::new(slider_right, vol_row.bottom()),
                        ),
                        ui.id().with("vol_slider"),
                        egui::Sense::click_and_drag(),
                    );

                    if slider_response.dragged() || slider_response.clicked() {
                        if let Some(pos) = slider_response.interact_pointer_pos() {
                            let t = ((pos.x - slider_left) / (slider_right - slider_left)).clamp(0.0, 1.0);
                            self.tone_volume = 0.05 + t * 0.45;
                            self.tone_state.set_volume(self.tone_volume);
                        }
                    }

                    // Slider track
                    ui.painter().rect_filled(slider_rect, 3.0, egui::Color32::from_gray(35));
                    let fill_frac = (self.tone_volume - 0.05) / 0.45;
                    let fill_rect = egui::Rect::from_min_max(
                        slider_rect.min,
                        egui::Pos2::new(
                            slider_left + fill_frac * (slider_right - slider_left),
                            slider_rect.max.y,
                        ),
                    );
                    ui.painter().rect_filled(fill_rect, 3.0, egui::Color32::from_rgb(60, 90, 140));

                    // Slider thumb
                    let thumb_x = slider_left + fill_frac * (slider_right - slider_left);
                    ui.painter().circle_filled(
                        egui::Pos2::new(thumb_x, vol_row.center().y),
                        5.0,
                        egui::Color32::from_rgb(100, 150, 220),
                    );

                    // Stop button
                    let stop_rect = egui::Rect::from_min_max(
                        egui::Pos2::new(slider_right + 8.0, vol_row.top() + 1.0),
                        egui::Pos2::new(vol_row.right() - 4.0, vol_row.bottom() - 1.0),
                    );
                    let stop_response = ui.interact(stop_rect, ui.id().with("stop_btn"), egui::Sense::click());
                    let stop_fill = if stop_response.hovered() {
                        egui::Color32::from_rgb(65, 35, 35)
                    } else {
                        egui::Color32::from_rgb(45, 30, 30)
                    };
                    ui.painter().rect_filled(stop_rect, 4.0, stop_fill);
                    ui.painter().rect_stroke(stop_rect, 4.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 45, 45)), egui::StrokeKind::Outside);
                    ui.painter().text(
                        stop_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "STOP",
                        egui::FontId::proportional(9.0),
                        egui::Color32::from_rgb(200, 100, 100),
                    );

                    if stop_response.clicked() {
                        self.tone_state.stop();
                        self.playing_string = None;
                    }
                }

                ui.add_space(10.0);

                // Volume meter — segmented LED style with label
                let meter_total_width = ui.available_width().min(370.0);
                let meter_label_width = 28.0;
                let meter_width = meter_total_width - meter_label_width;
                let meter_height = 8.0;
                let (meter_row, _) = ui.allocate_exact_size(
                    egui::Vec2::new(meter_total_width, meter_height),
                    egui::Sense::hover(),
                );

                // "IN" label
                let meter_left = meter_row.center().x - meter_total_width / 2.0;
                ui.painter().text(
                    egui::Pos2::new(meter_left + 10.0, meter_row.center().y),
                    egui::Align2::LEFT_CENTER,
                    "IN",
                    egui::FontId::proportional(8.0),
                    egui::Color32::from_gray(65),
                );

                let leds_left = meter_left + meter_label_width;
                let num_leds: usize = 36;
                let led_gap = 1.5;
                let led_width = (meter_width - led_gap * (num_leds - 1) as f32) / num_leds as f32;

                // Map RMS to dB-ish scale: -60dB to 0dB
                let db = if self.rms_level > 0.0 {
                    (20.0 * self.rms_level.log10()).clamp(-60.0, 0.0)
                } else {
                    -60.0
                };
                let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                let active_leds = (level * num_leds as f32).ceil() as usize;

                for led_i in 0..num_leds {
                    let led_x = leds_left + led_i as f32 * (led_width + led_gap);
                    let led_rect = egui::Rect::from_min_size(
                        egui::Pos2::new(led_x, meter_row.center().y - meter_height / 2.0),
                        egui::Vec2::new(led_width, meter_height),
                    );

                    let frac = led_i as f32 / num_leds as f32;
                    let led_color = if frac < 0.65 {
                        egui::Color32::from_rgb(0, 180, 80)
                    } else if frac < 0.85 {
                        egui::Color32::from_rgb(220, 180, 0)
                    } else {
                        egui::Color32::from_rgb(210, 50, 50)
                    };

                    if led_i < active_leds {
                        ui.painter().rect_filled(led_rect, 1.5, led_color);
                    } else {
                        ui.painter().rect_filled(led_rect, 1.5, led_color.gamma_multiply(0.1));
                    }
                }

                ui.add_space(10.0);

                // Waveform toggle — custom painted pill button
                let wf_width = 82.0;
                let wf_height = 20.0;
                let (wf_rect, _) = ui.allocate_exact_size(
                    egui::Vec2::new(wf_width, wf_height),
                    egui::Sense::hover(),
                );
                let wf_btn_rect = egui::Rect::from_center_size(wf_rect.center(), egui::Vec2::new(wf_width, wf_height));
                let wf_response = ui.interact(wf_btn_rect, ui.id().with("wf_toggle"), egui::Sense::click());

                if wf_response.clicked() {
                    self.show_waveform = !self.show_waveform;
                }

                let wf_fill = if self.show_waveform {
                    egui::Color32::from_rgb(30, 40, 52)
                } else if wf_response.hovered() {
                    egui::Color32::from_rgb(32, 35, 42)
                } else {
                    egui::Color32::from_rgb(26, 28, 35)
                };
                let wf_border = if self.show_waveform {
                    egui::Color32::from_rgb(60, 90, 140).gamma_multiply(0.5)
                } else {
                    egui::Color32::from_gray(40)
                };
                let wf_text_color = if self.show_waveform {
                    egui::Color32::from_rgb(130, 170, 220)
                } else {
                    egui::Color32::from_gray(90)
                };

                ui.painter().rect_filled(wf_btn_rect, 10.0, wf_fill);
                ui.painter().rect_stroke(wf_btn_rect, 10.0, egui::Stroke::new(1.0, wf_border), egui::StrokeKind::Outside);
                ui.painter().text(
                    wf_btn_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "WAVEFORM",
                    egui::FontId::proportional(9.0),
                    wf_text_color,
                );

                if self.show_waveform && !self.waveform_samples.is_empty() {
                    use egui_plot::{Line, Plot, PlotPoints};

                    let waveform_color = if self.clarity > 0.01 {
                        gauge::cents_color(self.smoothed_cents.abs() as f32)
                    } else {
                        egui::Color32::from_gray(80)
                    };

                    let points: PlotPoints = self.waveform_samples
                        .iter()
                        .enumerate()
                        .map(|(i, &s)| [i as f64, s as f64])
                        .collect();

                    Plot::new("waveform")
                        .height(80.0)
                        .show_axes(false)
                        .show_grid(false)
                        .allow_zoom(false)
                        .allow_drag(false)
                        .allow_scroll(false)
                        .allow_boxed_zoom(false)
                        .show_x(false)
                        .show_y(false)
                        .show(ui, |plot_ui| {
                            plot_ui.line(
                                Line::new("wave", points)
                                    .color(waveform_color)
                                    .width(1.5),
                            );
                        });
                }
            });
        });
    }
}
