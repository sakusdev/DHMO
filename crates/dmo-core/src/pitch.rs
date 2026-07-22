//! Musical pitch conversion helpers used by the editor UI.

/// MIDI note number for A4 concert pitch.
pub const A4_MIDI_NOTE: u8 = 69;

/// Converts a MIDI note number to equal-tempered frequency using A4 = 440 Hz.
#[must_use]
pub fn midi_note_frequency(midi_note: u8) -> f32 {
    let semitones_from_a4 = f32::from(midi_note) - f32::from(A4_MIDI_NOTE);
    440.0 * 2.0_f32.powf(semitones_from_a4 / 12.0)
}

/// Finds the nearest MIDI note for a frequency.
///
/// Non-positive and non-finite inputs fall back to A4. The result is clamped
/// to the standard MIDI range 0..=127.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn frequency_to_midi_note(frequency_hz: f32) -> u8 {
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 {
        return A4_MIDI_NOTE;
    }
    (f32::from(A4_MIDI_NOTE) + 12.0 * (frequency_hz / 440.0).log2())
        .round()
        .clamp(0.0, 127.0) as u8
}

/// Formats a MIDI note as a familiar pitch name such as `C4` or `F#3`.
#[must_use]
pub fn midi_note_name(midi_note: u8) -> String {
    const NOTE_NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let pitch_class = usize::from(midi_note % 12);
    let octave = i16::from(midi_note) / 12 - 1;
    format!("{}{octave}", NOTE_NAMES[pitch_class])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concert_pitch_round_trips() {
        assert!((midi_note_frequency(A4_MIDI_NOTE) - 440.0).abs() < f32::EPSILON);
        assert_eq!(frequency_to_midi_note(440.0), A4_MIDI_NOTE);
        assert_eq!(midi_note_name(A4_MIDI_NOTE), "A4");
    }

    #[test]
    fn common_note_names_and_frequencies_are_correct() {
        assert_eq!(midi_note_name(60), "C4");
        assert_eq!(midi_note_name(61), "C#4");
        assert!((midi_note_frequency(60) - 261.625_55).abs() < 0.001);
        assert_eq!(frequency_to_midi_note(261.63), 60);
    }

    #[test]
    fn invalid_frequencies_fall_back_to_a4() {
        assert_eq!(frequency_to_midi_note(0.0), A4_MIDI_NOTE);
        assert_eq!(frequency_to_midi_note(f32::NAN), A4_MIDI_NOTE);
    }
}
