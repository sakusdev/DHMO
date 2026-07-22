use crate::MidiNote;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantizeOptions {
    pub grid_frames: u64,
    /// Percentage from the original position toward the nearest grid line.
    pub strength: u8,
    pub quantize_ends: bool,
}

/// Quantizes note starts and, optionally, ends toward the nearest grid line.
pub fn quantize_notes(notes: &mut [MidiNote], options: QuantizeOptions) {
    if options.grid_frames == 0 || options.strength == 0 {
        return;
    }
    let strength = options.strength.min(100);
    for note in notes.iter_mut() {
        let old_start = note.start_frame;
        let old_end = note.end_frame();
        note.start_frame = move_toward(
            old_start,
            nearest_grid(old_start, options.grid_frames),
            strength,
        );
        if options.quantize_ends {
            let end = move_toward(
                old_end,
                nearest_grid(old_end, options.grid_frames),
                strength,
            );
            note.length_frames = end.saturating_sub(note.start_frame).max(1);
        }
    }
    notes.sort_by_key(|note| (note.start_frame, note.midi_note));
}

/// Transposes notes while clamping to the MIDI 0..=127 pitch range.
pub fn transpose_notes(notes: &mut [MidiNote], semitones: i16) {
    for note in notes {
        note.midi_note =
            u8::try_from((i16::from(note.midi_note) + semitones).clamp(0, 127)).unwrap_or(127);
    }
}

/// Adds a signed amount to note velocities.
pub fn adjust_note_velocities(notes: &mut [MidiNote], amount: i16) {
    for note in notes {
        note.velocity =
            u8::try_from((i16::from(note.velocity) + amount).clamp(1, 127)).unwrap_or(127);
    }
}

/// Scales velocities around zero by `percent`.
pub fn scale_note_velocities(notes: &mut [MidiNote], percent: u16) {
    for note in notes {
        let scaled = u32::from(note.velocity) * u32::from(percent) / 100;
        note.velocity = u8::try_from(scaled.clamp(1, 127)).unwrap_or(127);
    }
}

/// Extends every note/chord to the next distinct note start.
pub fn make_notes_legato(notes: &mut [MidiNote]) {
    notes.sort_by_key(|note| (note.start_frame, note.midi_note));
    let starts = notes
        .iter()
        .map(|note| note.start_frame)
        .collect::<Vec<_>>();
    for (index, note) in notes.iter_mut().enumerate() {
        if let Some(next_start) = starts[index + 1..]
            .iter()
            .copied()
            .find(|start| *start > note.start_frame)
        {
            note.length_frames = next_start.saturating_sub(note.start_frame).max(1);
        }
    }
}

/// Applies deterministic timing and velocity variation. The seed is explicit
/// so undoable edits and tests remain reproducible.
pub fn humanize_notes(notes: &mut [MidiNote], timing_frames: u64, velocity_amount: u8, seed: u64) {
    let mut random = XorShift64::new(seed);
    for note in notes.iter_mut() {
        let timing = random.signed_range(timing_frames);
        note.start_frame = saturating_add_signed(note.start_frame, timing);
        let velocity = random.signed_range(u64::from(velocity_amount.min(126)));
        let velocity = i128::from(note.velocity) + i128::from(velocity);
        note.velocity = u8::try_from(velocity.clamp(1, 127)).unwrap_or(127);
    }
    notes.sort_by_key(|note| (note.start_frame, note.midi_note));
}

/// Merges exact duplicates and trims overlapping notes of the same pitch.
pub fn remove_note_overlaps(notes: &mut Vec<MidiNote>) {
    notes.sort_by_key(|note| (note.start_frame, note.midi_note));
    let mut cleaned: Vec<MidiNote> = Vec::with_capacity(notes.len());
    let mut last_for_pitch: [Option<usize>; 128] = [None; 128];
    for note in notes.drain(..) {
        let pitch = usize::from(note.midi_note);
        if let Some(index) = last_for_pitch[pitch] {
            let previous = &mut cleaned[index];
            if previous.start_frame == note.start_frame {
                previous.length_frames = previous.length_frames.max(note.length_frames);
                previous.velocity = previous.velocity.max(note.velocity);
                continue;
            }
            if previous.end_frame() > note.start_frame {
                previous.length_frames = note.start_frame.saturating_sub(previous.start_frame);
            }
        }
        last_for_pitch[pitch] = Some(cleaned.len());
        cleaned.push(note);
    }
    cleaned.retain(|note| note.length_frames > 0);
    *notes = cleaned;
}

fn nearest_grid(frame: u64, grid: u64) -> u64 {
    let lower = frame / grid * grid;
    let upper = lower.saturating_add(grid);
    if frame - lower < upper - frame {
        lower
    } else {
        upper
    }
}

fn move_toward(value: u64, target: u64, strength: u8) -> u64 {
    let difference = i128::from(target) - i128::from(value);
    let movement = difference * i128::from(strength) / 100;
    let result = i128::from(value) + movement;
    u64::try_from(result.max(0)).unwrap_or(u64::MAX)
}

fn saturating_add_signed(value: u64, amount: i64) -> u64 {
    if amount >= 0 {
        value.saturating_add(amount.unsigned_abs())
    } else {
        value.saturating_sub(amount.unsigned_abs())
    }
}

struct XorShift64(u64);

impl XorShift64 {
    const fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn signed_range(&mut self, maximum: u64) -> i64 {
        let maximum = maximum.min(i64::MAX.unsigned_abs());
        if maximum == 0 {
            return 0;
        }
        let span = maximum.saturating_mul(2).saturating_add(1);
        let value = self.next() % span;
        i64::try_from(value).unwrap_or(i64::MAX) - i64::try_from(maximum).unwrap_or(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: u64, length: u64, pitch: u8, velocity: u8) -> MidiNote {
        MidiNote {
            start_frame: start,
            length_frames: length,
            midi_note: pitch,
            velocity,
        }
    }

    #[test]
    fn quantize_strength_and_ends_are_applied() {
        let mut notes = [note(70, 70, 60, 100)];
        quantize_notes(
            &mut notes,
            QuantizeOptions {
                grid_frames: 100,
                strength: 50,
                quantize_ends: true,
            },
        );
        assert_eq!(notes[0].start_frame, 85);
        assert_eq!(notes[0].length_frames, 35);
    }

    #[test]
    fn transpose_and_velocity_tools_clamp_to_midi_ranges() {
        let mut notes = [note(0, 10, 125, 120)];
        transpose_notes(&mut notes, 12);
        adjust_note_velocities(&mut notes, 20);
        assert_eq!(notes[0].midi_note, 127);
        assert_eq!(notes[0].velocity, 127);
        scale_note_velocities(&mut notes, 50);
        assert_eq!(notes[0].velocity, 63);
    }

    #[test]
    fn legato_and_overlap_cleanup_preserve_chords_and_remove_duplicates() {
        let mut notes = vec![
            note(0, 150, 60, 80),
            note(0, 100, 64, 90),
            note(100, 100, 60, 100),
            note(100, 120, 60, 110),
        ];
        make_notes_legato(&mut notes);
        assert_eq!(notes[0].length_frames, 100);
        assert_eq!(notes[1].length_frames, 100);
        remove_note_overlaps(&mut notes);
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[2].velocity, 110);
        assert_eq!(notes[2].length_frames, 120);
    }

    #[test]
    fn humanize_is_deterministic_and_bounded() {
        let original = vec![note(1_000, 100, 60, 64), note(2_000, 100, 62, 80)];
        let mut first = original.clone();
        let mut second = original;
        humanize_notes(&mut first, 50, 10, 42);
        humanize_notes(&mut second, 50, 10, 42);
        assert_eq!(first, second);
        assert!(
            first
                .iter()
                .all(|note| (950..=2_050).contains(&note.start_frame))
        );
        assert!(first.iter().all(|note| (1..=127).contains(&note.velocity)));
    }
}
