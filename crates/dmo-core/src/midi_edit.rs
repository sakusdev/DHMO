use crate::MidiNote;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantizeOptions {
    pub grid_frames: u64,
    /// Percentage from the original position toward the nearest grid line.
    pub strength: u8,
    pub quantize_ends: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwingQuantizeOptions {
    pub grid_frames: u64,
    /// Percentage from the original position toward the nearest swung grid line.
    pub strength: u8,
    /// 50 is straight timing. Higher values delay every second grid point.
    pub swing_percent: u8,
    pub quantize_ends: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrooveTemplate {
    Straight,
    Mpc16,
    LaidBack,
    PushPull,
}

impl GrooveTemplate {
    pub const ALL: [Self; 4] = [Self::Straight, Self::Mpc16, Self::LaidBack, Self::PushPull];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Straight => "Straight",
            Self::Mpc16 => "MPC 16",
            Self::LaidBack => "Laid-back",
            Self::PushPull => "Push/pull",
        }
    }

    const fn offsets_percent(self) -> &'static [i16] {
        match self {
            Self::Straight => &[0],
            Self::Mpc16 => &[0, 6, -2, 10],
            Self::LaidBack => &[0, 8, 6, 12],
            Self::PushPull => &[0, -5, 4, -2],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrooveQuantizeOptions {
    pub grid_frames: u64,
    /// Percentage from the original position toward the groove-adjusted grid.
    pub strength: u8,
    /// Percentage of the selected groove template timing offset to apply.
    pub amount: u8,
    pub template: GrooveTemplate,
    pub quantize_ends: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiScale {
    Major,
    NaturalMinor,
    MajorPentatonic,
    MinorPentatonic,
    Chromatic,
}

impl MidiScale {
    pub const ALL: [Self; 5] = [
        Self::Major,
        Self::NaturalMinor,
        Self::MajorPentatonic,
        Self::MinorPentatonic,
        Self::Chromatic,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::NaturalMinor => "Natural minor",
            Self::MajorPentatonic => "Major pentatonic",
            Self::MinorPentatonic => "Minor pentatonic",
            Self::Chromatic => "Chromatic",
        }
    }

    const fn pitch_classes(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            Self::MajorPentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }
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

/// Quantizes notes to a swung grid. A 50% swing is straight timing; common
/// values around 57–66% delay every second subdivision.
pub fn swing_quantize_notes(notes: &mut [MidiNote], options: SwingQuantizeOptions) {
    if options.grid_frames == 0 || options.strength == 0 {
        return;
    }
    let strength = options.strength.min(100);
    let swing_percent = options.swing_percent.clamp(50, 75);
    for note in notes.iter_mut() {
        let old_start = note.start_frame;
        let old_end = note.end_frame();
        note.start_frame = move_toward(
            old_start,
            nearest_swing_grid(old_start, options.grid_frames, swing_percent),
            strength,
        );
        if options.quantize_ends {
            let end = move_toward(
                old_end,
                nearest_swing_grid(old_end, options.grid_frames, swing_percent),
                strength,
            );
            note.length_frames = end.saturating_sub(note.start_frame).max(1);
        }
    }
    notes.sort_by_key(|note| (note.start_frame, note.midi_note));
}

/// Quantizes notes to a named timing feel template. The groove offset is applied
/// as a percentage of the active grid, then blended by strength.
pub fn groove_quantize_notes(notes: &mut [MidiNote], options: GrooveQuantizeOptions) {
    if options.grid_frames == 0 || options.strength == 0 {
        return;
    }
    let strength = options.strength.min(100);
    for note in notes.iter_mut() {
        let old_start = note.start_frame;
        let old_end = note.end_frame();
        note.start_frame = move_toward(
            old_start,
            groove_grid_position(old_start, options),
            strength,
        );
        if options.quantize_ends {
            let end = move_toward(old_end, groove_grid_position(old_end, options), strength);
            note.length_frames = end.saturating_sub(note.start_frame).max(1);
        }
    }
    notes.sort_by_key(|note| (note.start_frame, note.midi_note));
}

/// Moves notes to the nearest pitch in the selected key/scale.
pub fn conform_notes_to_scale(notes: &mut [MidiNote], root: u8, scale: MidiScale) {
    let root = root % 12;
    let allowed = scale.pitch_classes();
    for note in notes.iter_mut() {
        let pitch = i16::from(note.midi_note);
        let nearest = (-12..=12)
            .map(|offset| pitch + offset)
            .filter(|candidate| (0..=127).contains(candidate))
            .filter(|candidate| {
                let class = (u8::try_from(*candidate).unwrap_or(0) + 12 - root) % 12;
                allowed.contains(&class)
            })
            .min_by_key(|candidate| {
                let distance = (candidate - pitch).abs();
                let direction_bias = i16::from(*candidate < pitch);
                (distance, direction_bias)
            })
            .unwrap_or(pitch);
        note.midi_note = u8::try_from(nearest).unwrap_or(note.midi_note);
    }
    notes.sort_by_key(|note| (note.start_frame, note.midi_note));
}

/// Sets all selected note durations to a fixed musical/grid length in frames.
pub fn set_note_lengths(notes: &mut [MidiNote], length_frames: u64) {
    let length_frames = length_frames.max(1);
    for note in notes {
        note.length_frames = length_frames;
    }
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

fn nearest_swing_grid(frame: u64, grid: u64, swing_percent: u8) -> u64 {
    if swing_percent == 50 {
        return nearest_grid(frame, grid);
    }
    let index = frame / grid;
    (index.saturating_sub(2)..=index.saturating_add(2))
        .map(|candidate| swing_grid_position(candidate, grid, swing_percent))
        .min_by_key(|candidate| candidate.abs_diff(frame))
        .unwrap_or(frame)
}

fn swing_grid_position(index: u64, grid: u64, swing_percent: u8) -> u64 {
    if index.is_multiple_of(2) {
        return index.saturating_mul(grid);
    }
    let pair_start = index.saturating_sub(1).saturating_mul(grid);
    let swung_offset = u128::from(grid) * u128::from(swing_percent) / 50;
    pair_start.saturating_add(u64::try_from(swung_offset).unwrap_or(u64::MAX))
}

fn groove_grid_position(frame: u64, options: GrooveQuantizeOptions) -> u64 {
    let grid_index = nearest_grid(frame, options.grid_frames) / options.grid_frames;
    let base = grid_index.saturating_mul(options.grid_frames);
    let offset = groove_offset_frames(
        grid_index,
        options.grid_frames,
        options.template,
        options.amount,
    );
    saturating_add_signed(base, offset)
}

fn groove_offset_frames(
    grid_index: u64,
    grid_frames: u64,
    template: GrooveTemplate,
    amount: u8,
) -> i64 {
    let offsets = template.offsets_percent();
    let pattern_index = usize::try_from(grid_index % offsets.len() as u64).unwrap_or(0);
    let offset_percent = i128::from(offsets[pattern_index]);
    let frames = i128::from(grid_frames) * offset_percent * i128::from(amount.min(100)) / 10_000;
    i64::try_from(frames).unwrap_or(if frames.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
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
    fn swing_quantize_delays_every_second_grid_point() {
        let mut notes = vec![note(95, 80, 60, 100), note(205, 80, 62, 100)];
        swing_quantize_notes(
            &mut notes,
            SwingQuantizeOptions {
                grid_frames: 100,
                strength: 100,
                swing_percent: 66,
                quantize_ends: false,
            },
        );

        assert_eq!(notes[0].start_frame, 132);
        assert_eq!(notes[1].start_frame, 200);
        assert_eq!(notes[0].length_frames, 80);
    }

    #[test]
    fn groove_quantize_applies_template_offsets() {
        let mut notes = vec![note(96, 80, 60, 100), note(198, 80, 62, 100)];
        groove_quantize_notes(
            &mut notes,
            GrooveQuantizeOptions {
                grid_frames: 100,
                strength: 100,
                amount: 50,
                template: GrooveTemplate::Mpc16,
                quantize_ends: false,
            },
        );

        assert_eq!(notes[0].start_frame, 103);
        assert_eq!(notes[1].start_frame, 199);
        assert_eq!(notes[0].length_frames, 80);
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

    #[test]
    fn scale_conform_and_fixed_lengths_make_midi_parts_more_musical() {
        let mut notes = vec![
            note(0, 10, 61, 80),
            note(10, 20, 63, 80),
            note(20, 30, 66, 80),
        ];
        conform_notes_to_scale(&mut notes, 0, MidiScale::Major);
        assert_eq!(
            notes.iter().map(|note| note.midi_note).collect::<Vec<_>>(),
            vec![62, 64, 67]
        );

        set_note_lengths(&mut notes, 120);
        assert!(notes.iter().all(|note| note.length_frames == 120));
    }
}
