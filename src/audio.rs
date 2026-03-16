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

pub struct AudioEngine {
    _stream: Stream,
}

impl AudioEngine {
    pub fn start(shared: Arc<Mutex<AudioBuffer>>) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device found"))?;

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

        let mut phase: f32 = 0.0;
        // Smooth envelope to avoid clicks on start/stop
        let mut envelope: f32 = 0.0;

        let stream = device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| {
                let freq = state.frequency();
                let volume = state.volume();
                let target_env = if freq > 0.0 { 1.0 } else { 0.0 };

                for frame in data.chunks_mut(channels) {
                    // Smooth envelope (attack/release ~5ms at 44.1kHz)
                    envelope += (target_env - envelope) * 0.005;

                    let sample = if envelope > 0.001 {
                        let tau = 2.0 * std::f32::consts::PI;
                        // Richer harmonic series — more harmonics for low notes
                        // so they're audible on small speakers (missing fundamental)
                        let harmonic_weight = |n: f32| -> f32 {
                            // Below 150 Hz, boost upper harmonics significantly
                            let base = 1.0 / n;
                            if freq < 150.0 {
                                base * (1.0 + (150.0 - freq) / 100.0)
                            } else {
                                base
                            }
                        };
                        let mut s = (phase * tau).sin(); // fundamental
                        s += (phase * 2.0 * tau).sin() * harmonic_weight(2.0);
                        s += (phase * 3.0 * tau).sin() * harmonic_weight(3.0);
                        s += (phase * 4.0 * tau).sin() * harmonic_weight(4.0);
                        s += (phase * 5.0 * tau).sin() * harmonic_weight(5.0);
                        s += (phase * 6.0 * tau).sin() * harmonic_weight(6.0);
                        // Normalize so we don't clip
                        let norm = 1.0
                            + harmonic_weight(2.0)
                            + harmonic_weight(3.0)
                            + harmonic_weight(4.0)
                            + harmonic_weight(5.0)
                            + harmonic_weight(6.0);
                        s / norm * volume * envelope
                    } else {
                        0.0
                    };

                    for ch in frame.iter_mut() {
                        *ch = sample;
                    }

                    if freq > 0.0 {
                        phase += freq / sample_rate;
                        if phase >= 1.0 {
                            phase -= 1.0;
                        }
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
