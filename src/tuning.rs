const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

#[derive(Debug, Clone)]
pub struct NoteInfo {
    pub name: &'static str,
    pub octave: i32,
    pub exact_frequency: f64,
    pub cents_offset: f64,
}

/// A target string defined by semitone offset from A4.
#[derive(Debug, Clone)]
pub struct StringTarget {
    pub name: &'static str,
    pub semitones_from_a4: f64,
}

impl StringTarget {
    pub fn frequency(&self, a4: f64) -> f64 {
        a4 * 2.0_f64.powf(self.semitones_from_a4 / 12.0)
    }
}

#[derive(Debug, Clone)]
pub struct Tuning {
    pub name: &'static str,
    pub strings: Vec<StringTarget>,
}

/// Convert a frequency to the nearest note info.
pub fn frequency_to_note(freq: f64, a4: f64) -> NoteInfo {
    // Semitones from A4: 12 * log2(freq / a4)
    let semitones = 12.0 * (freq / a4).log2();
    let nearest_semitone = semitones.round() as i32;
    let cents_offset = (semitones - nearest_semitone as f64) * 100.0;

    // A4 is MIDI 69 → note index 9 (A), octave 4
    let midi = 69 + nearest_semitone;
    let note_index = ((midi % 12) + 12) % 12;
    let octave = (midi / 12) - 1;

    NoteInfo {
        name: NOTE_NAMES[note_index as usize],
        octave,
        exact_frequency: a4 * 2.0_f64.powf(nearest_semitone as f64 / 12.0),
        cents_offset,
    }
}

/// Find the string in the tuning closest to the given frequency. Returns (index, cents_deviation).
pub fn closest_string(freq: f64, tuning: &Tuning, a4: f64) -> (usize, f64) {
    tuning
        .strings
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let target = s.frequency(a4);
            let cents = 1200.0 * (freq / target).log2();
            (i, cents)
        })
        .min_by(|(_, a), (_, b)| a.abs().partial_cmp(&b.abs()).unwrap())
        .unwrap()
}

/// Semitone offset from A4 for a given note name and octave.
fn semitones(note: &str, octave: i32) -> f64 {
    let note_index = NOTE_NAMES.iter().position(|&n| n == note).unwrap() as i32;
    // A4 = MIDI 69 = note index 9, octave 4
    let midi = (octave + 1) * 12 + note_index;
    (midi - 69) as f64
}

pub fn standard_tuning() -> Tuning {
    Tuning {
        name: "Standard",
        strings: vec![
            StringTarget { name: "E2", semitones_from_a4: semitones("E", 2) },
            StringTarget { name: "A2", semitones_from_a4: semitones("A", 2) },
            StringTarget { name: "D3", semitones_from_a4: semitones("D", 3) },
            StringTarget { name: "G3", semitones_from_a4: semitones("G", 3) },
            StringTarget { name: "B3", semitones_from_a4: semitones("B", 3) },
            StringTarget { name: "E4", semitones_from_a4: semitones("E", 4) },
        ],
    }
}

pub fn drop_d_tuning() -> Tuning {
    Tuning {
        name: "Drop D",
        strings: vec![
            StringTarget { name: "D2", semitones_from_a4: semitones("D", 2) },
            StringTarget { name: "A2", semitones_from_a4: semitones("A", 2) },
            StringTarget { name: "D3", semitones_from_a4: semitones("D", 3) },
            StringTarget { name: "G3", semitones_from_a4: semitones("G", 3) },
            StringTarget { name: "B3", semitones_from_a4: semitones("B", 3) },
            StringTarget { name: "E4", semitones_from_a4: semitones("E", 4) },
        ],
    }
}

pub fn open_g_tuning() -> Tuning {
    Tuning {
        name: "Open G",
        strings: vec![
            StringTarget { name: "D2", semitones_from_a4: semitones("D", 2) },
            StringTarget { name: "G2", semitones_from_a4: semitones("G", 2) },
            StringTarget { name: "D3", semitones_from_a4: semitones("D", 3) },
            StringTarget { name: "G3", semitones_from_a4: semitones("G", 3) },
            StringTarget { name: "B3", semitones_from_a4: semitones("B", 3) },
            StringTarget { name: "D4", semitones_from_a4: semitones("D", 4) },
        ],
    }
}

pub fn open_d_tuning() -> Tuning {
    Tuning {
        name: "Open D",
        strings: vec![
            StringTarget { name: "D2", semitones_from_a4: semitones("D", 2) },
            StringTarget { name: "A2", semitones_from_a4: semitones("A", 2) },
            StringTarget { name: "D3", semitones_from_a4: semitones("D", 3) },
            StringTarget { name: "F#3", semitones_from_a4: semitones("F#", 3) },
            StringTarget { name: "A3", semitones_from_a4: semitones("A", 3) },
            StringTarget { name: "D4", semitones_from_a4: semitones("D", 4) },
        ],
    }
}

pub fn dadgad_tuning() -> Tuning {
    Tuning {
        name: "DADGAD",
        strings: vec![
            StringTarget { name: "D2", semitones_from_a4: semitones("D", 2) },
            StringTarget { name: "A2", semitones_from_a4: semitones("A", 2) },
            StringTarget { name: "D3", semitones_from_a4: semitones("D", 3) },
            StringTarget { name: "G3", semitones_from_a4: semitones("G", 3) },
            StringTarget { name: "A3", semitones_from_a4: semitones("A", 3) },
            StringTarget { name: "D4", semitones_from_a4: semitones("D", 4) },
        ],
    }
}

// Flat note names for display (indexed by semitone offset from C)
const FLAT_NOTE_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

/// Build a tuning by shifting every string of a base tuning down by `half_steps` semitones.
fn shift_tuning(base: &Tuning, half_steps: i32, name: &'static str) -> Tuning {
    Tuning {
        name,
        strings: base
            .strings
            .iter()
            .map(|s| {
                let new_semitones = s.semitones_from_a4 + half_steps as f64;
                // Compute display name: MIDI note from A4
                let midi = 69 + new_semitones as i32;
                let note_idx = ((midi % 12) + 12) % 12;
                let octave = (midi / 12) - 1;
                // Use flat names for downtuned notes (more conventional for guitar)
                let display = FLAT_NOTE_NAMES[note_idx as usize];
                // Leak a string for the static name — these are created once at startup
                let name: &'static str =
                    Box::leak(format!("{display}{octave}").into_boxed_str());
                StringTarget {
                    name,
                    semitones_from_a4: new_semitones,
                }
            })
            .collect(),
    }
}

pub fn half_step_down_tuning() -> Tuning {
    shift_tuning(&standard_tuning(), -1, "Eb Standard")
}

pub fn whole_step_down_tuning() -> Tuning {
    shift_tuning(&standard_tuning(), -2, "D Standard")
}

pub fn drop_db_tuning() -> Tuning {
    // Half step down then drop the 6th string another whole step
    let mut t = shift_tuning(&standard_tuning(), -1, "Drop Db");
    t.strings[0] = StringTarget {
        name: "Db2",
        semitones_from_a4: semitones("C#", 2),
    };
    t
}

pub fn drop_c_tuning() -> Tuning {
    // Whole step down then drop the 6th string another whole step (C-G-C-F-A-D)
    let mut t = shift_tuning(&standard_tuning(), -2, "Drop C");
    t.strings[0] = StringTarget {
        name: "C2",
        semitones_from_a4: semitones("C", 2),
    };
    t
}

pub fn all_tunings() -> Vec<Tuning> {
    vec![
        standard_tuning(),
        half_step_down_tuning(),
        whole_step_down_tuning(),
        drop_d_tuning(),
        drop_db_tuning(),
        drop_c_tuning(),
        open_g_tuning(),
        open_d_tuning(),
        dadgad_tuning(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a440() {
        let note = frequency_to_note(440.0, 440.0);
        assert_eq!(note.name, "A");
        assert_eq!(note.octave, 4);
        assert!(note.cents_offset.abs() < 0.01);
    }

    #[test]
    fn test_e2() {
        let note = frequency_to_note(82.41, 440.0);
        assert_eq!(note.name, "E");
        assert_eq!(note.octave, 2);
        assert!(note.cents_offset.abs() < 1.0);
    }

    #[test]
    fn test_b3() {
        let note = frequency_to_note(246.94, 440.0);
        assert_eq!(note.name, "B");
        assert_eq!(note.octave, 3);
        assert!(note.cents_offset.abs() < 1.0);
    }

    #[test]
    fn test_sharp_a440() {
        // 10 cents sharp of A4
        let freq = 440.0 * 2.0_f64.powf(10.0 / 1200.0);
        let note = frequency_to_note(freq, 440.0);
        assert_eq!(note.name, "A");
        assert!((note.cents_offset - 10.0).abs() < 0.1);
    }

    #[test]
    fn test_closest_string_standard() {
        let tuning = standard_tuning();
        let (idx, cents) = closest_string(82.41, &tuning, 440.0);
        assert_eq!(idx, 0); // E2
        assert!(cents.abs() < 1.0);
    }

    #[test]
    fn test_closest_string_a2() {
        let tuning = standard_tuning();
        let (idx, cents) = closest_string(110.0, &tuning, 440.0);
        assert_eq!(idx, 1); // A2
        assert!(cents.abs() < 0.01);
    }

    #[test]
    fn test_string_frequencies() {
        let tuning = standard_tuning();
        let e2 = tuning.strings[0].frequency(440.0);
        assert!((e2 - 82.41).abs() < 0.01);
        let a2 = tuning.strings[1].frequency(440.0);
        assert!((a2 - 110.0).abs() < 0.01);
    }

    #[test]
    fn test_a4_recalibration() {
        let note = frequency_to_note(442.0, 442.0);
        assert_eq!(note.name, "A");
        assert_eq!(note.octave, 4);
        assert!(note.cents_offset.abs() < 0.01);
    }
}
