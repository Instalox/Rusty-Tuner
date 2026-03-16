use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const BUFFER_SIZE: usize = 8192;

pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub write_pos: usize,
    pub sample_rate: u32,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            samples: vec![0.0; BUFFER_SIZE],
            write_pos: 0,
            sample_rate,
        }
    }

    fn push_samples(&mut self, data: &[f32], channels: usize) {
        for chunk in data.chunks(channels) {
            self.samples[self.write_pos] = chunk[0]; // mono: take first channel
            self.write_pos = (self.write_pos + 1) % BUFFER_SIZE;
        }
    }

    /// Copy the most recent `n` samples in order.
    pub fn latest(&self, n: usize) -> Vec<f32> {
        let n = n.min(BUFFER_SIZE);
        let mut out = Vec::with_capacity(n);
        let start = (self.write_pos + BUFFER_SIZE - n) % BUFFER_SIZE;
        for i in 0..n {
            out.push(self.samples[(start + i) % BUFFER_SIZE]);
        }
        out
    }

    /// RMS level of the most recent `n` samples.
    pub fn rms(&self, n: usize) -> f32 {
        let samples = self.latest(n);
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }
}

/// List available input device names.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

pub struct AudioEngine {
    _stream: Stream,
}

impl AudioEngine {
    /// Start capturing from a named device, or the default if `device_name` is None.
    pub fn start(
        shared: Arc<Mutex<AudioBuffer>>,
        device_name: Option<&str>,
    ) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = if let Some(name) = device_name {
            host.input_devices()?
                .find(|d| {
                    d.description()
                        .map(|desc| desc.name() == name)
                        .unwrap_or(false)
                })
                .ok_or_else(|| anyhow::anyhow!("Input device '{name}' not found"))?
        } else {
            host.default_input_device()
                .ok_or_else(|| anyhow::anyhow!("No input device found"))?
        };

        let config = device.default_input_config()?;
        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate();

        // Update shared buffer with actual sample rate
        if let Ok(mut buf) = shared.lock() {
            buf.sample_rate = sample_rate;
        }

        let sample_format = config.sample_format();
        let stream_config = config.config();

        let shared_clone = Arc::clone(&shared);
        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| {
                    if let Ok(mut buf) = shared_clone.try_lock() {
                        buf.push_samples(data, channels);
                    }
                },
                |err| eprintln!("Audio stream error: {err}"),
                None,
            )?,
            SampleFormat::I16 => {
                let shared_clone2 = Arc::clone(&shared);
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| {
                        if let Ok(mut buf) = shared_clone2.try_lock() {
                            let floats: Vec<f32> =
                                data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                            buf.push_samples(&floats, channels);
                        }
                    },
                    |err| eprintln!("Audio stream error: {err}"),
                    None,
                )?
            }
            format => return Err(anyhow::anyhow!("Unsupported sample format: {format:?}")),
        };

        stream.play()?;

        Ok(Self { _stream: stream })
    }
}

// ---- Reference tone generator ----

/// Shared state for the tone generator. Frequency stored as f32 bits in an AtomicU32
/// so we can update it lock-free from the UI thread.
pub struct ToneState {
    /// Target frequency as f32 bits (0.0 = silent)
    freq_bits: AtomicU32,
    /// Volume 0.0–1.0 as f32 bits
    volume_bits: AtomicU32,
}

impl ToneState {
    pub fn new() -> Self {
        Self {
            freq_bits: AtomicU32::new(0.0_f32.to_bits()),
            volume_bits: AtomicU32::new(0.25_f32.to_bits()),
        }
    }

    pub fn set_frequency(&self, freq: f32) {
        self.freq_bits.store(freq.to_bits(), Ordering::Relaxed);
    }

    pub fn frequency(&self) -> f32 {
        f32::from_bits(self.freq_bits.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, vol: f32) {
        self.volume_bits.store(vol.to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }

    pub fn stop(&self) {
        self.set_frequency(0.0);
    }

    pub fn is_playing(&self) -> bool {
        self.frequency() > 0.0
    }
}

pub struct ToneGenerator {
    _stream: Stream,
}

impl ToneGenerator {
    pub fn start(state: Arc<ToneState>) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No output device found"))?;

        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate() as f32;
        let channels = config.channels() as usize;
        let stream_config = config.config();

        // Karplus-Strong state
        let mut delay_line: Vec<f32> = Vec::new();
        let mut read_pos: usize = 0;
        let mut last_freq: f32 = 0.0;
        let mut phase: f32 = 0.0;
        let mut envelope: f32 = 0.0;
        let feedback: f32 = 0.996;

        let stream = device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| {
                let freq = state.frequency();
                let volume = state.volume();
                let target_env = if freq > 0.0 { 1.0 } else { 0.0 };

                // Reset last_freq on stop so re-play triggers a fresh pluck
                if freq <= 0.0 {
                    last_freq = 0.0;
                }

                // New note or frequency change → fresh pluck
                if freq > 0.0 && (freq - last_freq).abs() > 0.5 {
                    last_freq = freq;
                    let delay_len = (sample_rate / freq).max(20.0) as usize;
                    delay_line.resize(delay_len, 0.0);

                    // Deterministic noise burst for the pluck attack
                    let mut seed = 0x5bd1e995u32;
                    for sample in delay_line.iter_mut() {
                        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                        let r = ((seed >> 16) & 0x7FFF) as f32 / 32767.0;
                        *sample = (r * 2.0 - 1.0) * 0.85;
                    }
                    read_pos = 0;
                    phase = 0.0;
                    envelope = 1.0;
                }

                for frame in data.chunks_mut(channels) {
                    // Smooth envelope for stop (avoids clicks)
                    envelope += (target_env - envelope) * 0.0005;

                    let sample = if envelope > 0.001 && !delay_line.is_empty() && freq > 0.0 {
                        let current = delay_line[read_pos];
                        let next_idx = (read_pos + 1) % delay_line.len();
                        let next = delay_line[next_idx];

                        // KS low-pass averaging + feedback decay
                        let filtered = (current + next) * 0.5 * feedback;

                        // Small continuous excitation to sustain the tone
                        // (sine at fundamental, just enough energy to prevent decay to silence)
                        let excitation = (phase * 2.0 * std::f32::consts::PI).sin() * 0.006;
                        delay_line[read_pos] = filtered + excitation;

                        phase += freq / sample_rate;
                        if phase >= 1.0 {
                            phase -= 1.0;
                        }

                        read_pos = next_idx;
                        current * volume * envelope
                    } else {
                        // Stopped — let the delay line ring out naturally
                        if !delay_line.is_empty() && envelope > 0.0001 {
                            let current = delay_line[read_pos];
                            let next_idx = (read_pos + 1) % delay_line.len();
                            let next = delay_line[next_idx];
                            delay_line[read_pos] = (current + next) * 0.5 * feedback;
                            read_pos = next_idx;
                            current * volume * envelope
                        } else {
                            0.0
                        }
                    };

                    for ch in frame.iter_mut() {
                        *ch = sample;
                    }
                }
            },
            |err| eprintln!("Output stream error: {err}"),
            None,
        )?;

        stream.play()?;

        Ok(Self { _stream: stream })
    }
}
