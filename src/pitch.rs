use pitch_detection::detector::mcleod::McLeodDetector;
use pitch_detection::detector::PitchDetector;

const DETECTION_SIZE: usize = 4096;
const PADDING: usize = 2048;
const POWER_THRESHOLD: f64 = 5.0;
const CLARITY_THRESHOLD: f64 = 0.8;

/// Minimum RMS level to even attempt detection — noise gate.
const NOISE_GATE_RMS: f32 = 0.005;

#[derive(Debug, Clone, Copy)]
pub struct PitchResult {
    pub frequency: f64,
    pub clarity: f64,
}

pub struct PitchEngine {
    detector: McLeodDetector<f64>,
}

impl PitchEngine {
    pub fn new() -> Self {
        Self {
            detector: McLeodDetector::new(DETECTION_SIZE, PADDING),
        }
    }

    /// Detect pitch from audio samples. Expects at least DETECTION_SIZE samples.
    pub fn detect(&mut self, samples: &[f32], sample_rate: u32) -> Option<PitchResult> {
        if samples.len() < DETECTION_SIZE {
            return None;
        }

        let window = &samples[samples.len() - DETECTION_SIZE..];

        // Noise gate: skip detection if signal is too quiet
        let rms = (window.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>()
            / window.len() as f64)
            .sqrt() as f32;
        if rms < NOISE_GATE_RMS {
            return None;
        }

        let buf: Vec<f64> = window.iter().map(|&s| s as f64).collect();

        let pitch = self.detector.get_pitch(
            &buf,
            sample_rate as usize,
            POWER_THRESHOLD,
            CLARITY_THRESHOLD,
        )?;

        // Reject unreasonable frequencies for guitar (20 Hz – 2000 Hz)
        if pitch.frequency < 20.0 || pitch.frequency > 2000.0 {
            return None;
        }

        Some(PitchResult {
            frequency: pitch.frequency,
            clarity: pitch.clarity,
        })
    }

    pub fn detection_size(&self) -> usize {
        DETECTION_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine_wave(freq: f32, sample_rate: u32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| (2.0 * PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn test_a440() {
        let mut engine = PitchEngine::new();
        let samples = sine_wave(440.0, 44100, 8192);
        let result = engine.detect(&samples, 44100).unwrap();
        assert!((result.frequency - 440.0).abs() < 2.0);
        assert!(result.clarity > 0.7);
    }

    #[test]
    fn test_e2() {
        let mut engine = PitchEngine::new();
        let samples = sine_wave(82.41, 44100, 8192);
        let result = engine.detect(&samples, 44100).unwrap();
        assert!((result.frequency - 82.41).abs() < 2.0);
    }

    #[test]
    fn test_silence() {
        let mut engine = PitchEngine::new();
        let samples = vec![0.0_f32; 8192];
        let result = engine.detect(&samples, 44100);
        assert!(result.is_none());
    }

    #[test]
    fn test_too_few_samples() {
        let mut engine = PitchEngine::new();
        let samples = vec![0.0_f32; 100];
        assert!(engine.detect(&samples, 44100).is_none());
    }

    #[test]
    fn test_quiet_noise_rejected() {
        let mut engine = PitchEngine::new();
        // Very quiet sine — below noise gate
        let samples: Vec<f32> = sine_wave(440.0, 44100, 8192)
            .iter()
            .map(|s| s * 0.001)
            .collect();
        assert!(engine.detect(&samples, 44100).is_none());
    }
}
