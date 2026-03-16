mod gauge;

use crate::audio::{AudioBuffer, AudioEngine, ToneGenerator, ToneState};
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

        let shared_buffer = Arc::new(Mutex::new(AudioBuffer::new(44100)));

        let (audio_engine, audio_error) = match AudioEngine::start(Arc::clone(&shared_buffer)) {
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
        }
    }
}

impl eframe::App for TunerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(33));

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

                // Header bar
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("tuning_select")
                        .selected_text(self.tunings[self.selected_tuning].name)
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for (i, t) in self.tunings.iter().enumerate() {
                                if ui.selectable_value(&mut self.selected_tuning, i, t.name).changed() {
                                    self.selected_string = None;
                                }
                            }
                        });

                    ui.add_space(8.0);

                    ui.label(
                        egui::RichText::new("A4")
                            .size(12.0)
                            .color(egui::Color32::from_gray(130)),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.a4_freq, 420.0..=460.0)
                            .suffix(" Hz")
                            .fixed_decimals(1)
                            .text(""),
                    );
                });

                ui.add_space(12.0);

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

                ui.add_space(10.0);

                // String buttons
                let tuning = self.tunings[self.selected_tuning].clone();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    // Auto button
                    let auto_active = self.selected_string.is_none();
                    let auto_btn = egui::Button::new(
                        egui::RichText::new("Auto")
                            .size(13.0)
                            .color(if auto_active {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_gray(160)
                            }),
                    )
                    .fill(if auto_active {
                        egui::Color32::from_rgb(50, 55, 68)
                    } else {
                        egui::Color32::from_rgb(35, 38, 45)
                    })
                    .corner_radius(6.0)
                    .min_size(egui::Vec2::new(46.0, 30.0));

                    if ui.add(auto_btn).clicked() {
                        self.selected_string = None;
                    }

                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);

                    for (i, s) in tuning.strings.iter().enumerate() {
                        let is_selected = self.selected_string == Some(i);
                        let is_playing = self.playing_string == Some(i);
                        let is_auto_detected = self.selected_string.is_none()
                            && self.auto_string_idx == Some(i)
                            && self.clarity > 0.01;

                        let (fill, text_color) = if is_selected {
                            (
                                egui::Color32::from_rgb(50, 55, 68),
                                egui::Color32::WHITE,
                            )
                        } else if is_auto_detected {
                            (
                                egui::Color32::from_rgb(40, 48, 55),
                                gauge::cents_color(self.auto_string_cents.abs() as f32).gamma_multiply(0.9),
                            )
                        } else {
                            (
                                egui::Color32::from_rgb(35, 38, 45),
                                egui::Color32::from_gray(130),
                            )
                        };

                        let target_hz = s.frequency(self.a4_freq);

                        // String select button
                        let btn = egui::Button::new(
                            egui::RichText::new(s.name)
                                .size(13.0)
                                .color(text_color),
                        )
                        .fill(fill)
                        .corner_radius(6.0)
                        .min_size(egui::Vec2::new(42.0, 30.0));

                        let btn_response = ui.add(btn);
                        btn_response.clone().on_hover_text(format!("{:.1} Hz", target_hz));

                        if btn_response.clicked() {
                            self.selected_string = Some(i);
                        }

                        // Play/stop toggle — small speaker button
                        let play_label = if is_playing { "\u{23F9}" } else { "\u{25B6}" };
                        let play_btn = egui::Button::new(
                            egui::RichText::new(play_label)
                                .size(10.0)
                                .color(if is_playing {
                                    egui::Color32::from_rgb(0, 200, 90)
                                } else {
                                    egui::Color32::from_gray(110)
                                }),
                        )
                        .fill(if is_playing {
                            egui::Color32::from_rgb(30, 50, 40)
                        } else {
                            egui::Color32::from_rgb(30, 33, 38)
                        })
                        .corner_radius(4.0)
                        .min_size(egui::Vec2::new(22.0, 30.0));

                        if ui.add(play_btn).clicked() {
                            if is_playing {
                                self.tone_state.stop();
                                self.playing_string = None;
                            } else {
                                self.tone_state.set_frequency(target_hz as f32);
                                self.tone_state.set_volume(self.tone_volume);
                                self.playing_string = Some(i);
                            }
                        }

                        if i < tuning.strings.len() - 1 {
                            ui.add_space(2.0);
                        }
                    }
                });

                // Tone volume slider (only visible when playing)
                if self.playing_string.is_some() {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.label(
                            egui::RichText::new("Vol")
                                .size(11.0)
                                .color(egui::Color32::from_gray(110)),
                        );
                        let slider = egui::Slider::new(&mut self.tone_volume, 0.05..=0.5)
                            .show_value(false)
                            .text("");
                        if ui.add(slider).changed() {
                            self.tone_state.set_volume(self.tone_volume);
                        }

                        // Stop all button
                        if ui.add(
                            egui::Button::new(
                                egui::RichText::new("Stop")
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(180)),
                            )
                            .fill(egui::Color32::from_rgb(50, 35, 35))
                            .corner_radius(4.0),
                        ).clicked() {
                            self.tone_state.stop();
                            self.playing_string = None;
                        }
                    });
                }

                ui.add_space(10.0);

                // Volume meter — segmented LED style
                let meter_width = ui.available_width().min(320.0);
                let meter_height = 6.0;
                let (meter_rect, _) = ui.allocate_exact_size(
                    egui::Vec2::new(meter_width, meter_height),
                    egui::Sense::hover(),
                );

                let painter = ui.painter();
                let num_leds = 32;
                let led_gap = 2.0;
                let led_width = (meter_width - led_gap * (num_leds - 1) as f32) / num_leds as f32;

                // Map RMS to dB-ish scale: -60dB to 0dB
                let db = if self.rms_level > 0.0 {
                    (20.0 * self.rms_level.log10()).clamp(-60.0, 0.0)
                } else {
                    -60.0
                };
                let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                let active_leds = (level * num_leds as f32).ceil() as usize;

                for i in 0..num_leds {
                    let x = meter_rect.min.x + i as f32 * (led_width + led_gap);
                    let led_rect = egui::Rect::from_min_size(
                        egui::Pos2::new(x, meter_rect.min.y),
                        egui::Vec2::new(led_width, meter_height),
                    );

                    let frac = i as f32 / num_leds as f32;
                    let led_color = if frac < 0.65 {
                        egui::Color32::from_rgb(0, 180, 80)
                    } else if frac < 0.85 {
                        egui::Color32::from_rgb(220, 180, 0)
                    } else {
                        egui::Color32::from_rgb(210, 50, 50)
                    };

                    if i < active_leds {
                        painter.rect_filled(led_rect, 1.0, led_color);
                    } else {
                        painter.rect_filled(led_rect, 1.0, led_color.gamma_multiply(0.12));
                    }
                }

                ui.add_space(8.0);

                // Waveform toggle + plot
                let wf_color = if self.show_waveform { 180 } else { 100 };
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.toggle_value(
                        &mut self.show_waveform,
                        egui::RichText::new("Waveform")
                            .size(12.0)
                            .color(egui::Color32::from_gray(wf_color)),
                    );
                });

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
