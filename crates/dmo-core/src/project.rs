use std::fmt;

/// A complete editable song.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub sample_rate: u32,
    pub tempo_bpm: f64,
    pub time_signature: TimeSignature,
    /// Optional persistent cycle/loop range used by playback and punch recording.
    pub cycle_range: Option<CycleRange>,
    /// Timeline markers used for navigation and arrangement notes.
    pub markers: Vec<Marker>,
    /// Named song sections shown on the arranger lane.
    pub arranger_sections: Vec<ArrangerSection>,
    /// Linear gain applied at the master output after all track mixing.
    pub master_gain: f32,
    /// Insert processing applied after all buses and before the master gain.
    pub master_inserts: Vec<ChannelInsert>,
    /// Auxiliary/group channels fed by track outputs and sends.
    pub buses: Vec<Bus>,
    pub tracks: Vec<Track>,
}

impl Project {
    /// Creates an empty project after validating its audio and musical settings.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] when the sample rate or tempo is outside the
    /// supported range.
    pub fn new(
        name: impl Into<String>,
        sample_rate: u32,
        tempo_bpm: f64,
    ) -> Result<Self, ProjectError> {
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err(ProjectError::InvalidSampleRate(sample_rate));
        }
        if !tempo_bpm.is_finite() || !(20.0..=400.0).contains(&tempo_bpm) {
            return Err(ProjectError::InvalidTempo(tempo_bpm));
        }

        Ok(Self {
            name: name.into(),
            sample_rate,
            tempo_bpm,
            time_signature: TimeSignature::default(),
            cycle_range: None,
            markers: Vec::new(),
            arranger_sections: Vec::new(),
            master_gain: 1.0,
            master_inserts: Vec::new(),
            buses: Vec::new(),
            tracks: Vec::new(),
        })
    }

    #[must_use]
    pub fn duration_frames(&self) -> u64 {
        self.tracks
            .iter()
            .flat_map(|track| &track.clips)
            .map(Clip::end_frame)
            .max()
            .unwrap_or(0)
    }

    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn seconds_to_frames(&self, seconds: f64) -> u64 {
        (seconds.max(0.0) * f64::from(self.sample_rate)).round() as u64
    }
}

/// A project-wide musical meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl TimeSignature {
    pub const COMMON_DENOMINATORS: [u8; 5] = [2, 4, 8, 16, 32];

    #[must_use]
    pub const fn new(numerator: u8, denominator: u8) -> Option<Self> {
        if numerator == 0 || numerator > 32 || !matches!(denominator, 2 | 4 | 8 | 16 | 32) {
            return None;
        }
        Some(Self {
            numerator,
            denominator,
        })
    }

    #[must_use]
    pub const fn beats_per_bar(self) -> u8 {
        self.numerator
    }
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

/// A saved song cycle range spanning a half-open timeline range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleRange {
    pub start_frame: u64,
    pub end_frame: u64,
}

impl CycleRange {
    #[must_use]
    pub const fn new(start_frame: u64, end_frame: u64) -> Option<Self> {
        if end_frame <= start_frame {
            return None;
        }
        Some(Self {
            start_frame,
            end_frame,
        })
    }

    #[must_use]
    pub const fn length_frames(self) -> u64 {
        self.end_frame - self.start_frame
    }
}

/// A named song section spanning a half-open timeline range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrangerSection {
    pub name: String,
    pub start_frame: u64,
    pub length_frames: u64,
    pub color_index: u8,
}

impl ArrangerSection {
    #[must_use]
    pub fn new(name: impl Into<String>, start_frame: u64, length_frames: u64) -> Self {
        Self {
            name: name.into(),
            start_frame,
            length_frames: length_frames.max(1),
            color_index: 0,
        }
    }

    #[must_use]
    pub const fn end_frame(&self) -> u64 {
        self.start_frame.saturating_add(self.length_frames)
    }
}

/// A named timeline location in absolute project frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub name: String,
    pub frame: u64,
}

impl Marker {
    #[must_use]
    pub fn new(name: impl Into<String>, frame: u64) -> Self {
        Self {
            name: name.into(),
            frame,
        }
    }
}

/// A stereo mixer channel containing timeline clips.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub name: String,
    pub gain: f32,
    pub pan: f32,
    pub muted: bool,
    pub soloed: bool,
    pub recording: TrackRecording,
    /// Hardware source captured when this track is record-armed.
    pub input: TrackInput,
    /// Main destination for this channel after its fader and inserts.
    pub output: ChannelOutput,
    /// Parallel pre/post-fader feeds into auxiliary or group buses.
    pub sends: Vec<TrackSend>,
    /// Ordered channel-strip processors before the channel fader.
    pub inserts: Vec<ChannelInsert>,
    /// Built-in instrument used by MIDI clips on this track.
    pub instrument: Instrument,
    /// Optional `SoundFont` preset used instead of the built-in oscillator.
    pub soundfont: Option<SoundFontPreset>,
    /// Logical MIDI channel in the conventional 1..=16 range.
    pub midi_channel: u8,
    /// Sample-accurate MIDI continuous-controller events on this track.
    pub midi_cc: Vec<MidiControlPoint>,
    /// Sample-accurate 14-bit pitch-wheel events in the MIDI -8192..=8191 range.
    pub midi_pitch_bend: Vec<MidiPitchBendPoint>,
    /// Channel-wide pressure (aftertouch) events.
    pub midi_channel_pressure: Vec<MidiChannelPressurePoint>,
    /// Per-note polyphonic pressure (aftertouch) events.
    pub midi_poly_pressure: Vec<MidiPolyPressurePoint>,
    /// MIDI program-change events selecting external patches by channel.
    pub midi_program_changes: Vec<MidiProgramChangePoint>,
    /// Linear gain multipliers interpolated across the project timeline.
    pub volume_automation: Vec<AutomationPoint>,
    /// Alternate audio recordings retained below the main comp lane.
    ///
    /// The clips in `clips` are the audible comp. Matching entries here can
    /// be promoted for a whole clip or for a split section without deleting
    /// earlier performances.
    pub take_lanes: Vec<AudioTake>,
    pub clips: Vec<Clip>,
}

impl Track {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            gain: 1.0,
            pan: 0.0,
            muted: false,
            soloed: false,
            recording: TrackRecording::default(),
            input: TrackInput::Audio,
            output: ChannelOutput::Master,
            sends: Vec::new(),
            inserts: Vec::new(),
            instrument: Instrument::Sine,
            soundfont: None,
            midi_channel: 1,
            midi_cc: Vec::new(),
            midi_pitch_bend: Vec::new(),
            midi_channel_pressure: Vec::new(),
            midi_poly_pressure: Vec::new(),
            midi_program_changes: Vec::new(),
            volume_automation: Vec::new(),
            take_lanes: Vec::new(),
            clips: Vec::new(),
        }
    }

    /// Returns the linearly interpolated volume automation multiplier.
    #[must_use]
    pub fn automation_gain_at(&self, frame: u64) -> f32 {
        automation_value_at(&self.volume_automation, frame)
    }
}

/// A selectable preset contained in an SF2 `SoundFont` file.
///
/// The sample data stays in the external `SoundFont` file; projects only keep
/// the path and stable bank/program address so they remain compact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundFontPreset {
    pub path: String,
    pub bank: u16,
    pub program: u16,
    pub name: String,
}

impl SoundFontPreset {
    #[must_use]
    pub fn new(path: impl Into<String>, bank: u16, program: u16, name: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            bank,
            program,
            name: name.into(),
        }
    }

    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "{} · Bank {} Program {}",
            self.name,
            self.bank,
            self.program + 1
        )
    }
}

/// Source accepted by a record-armed track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackInput {
    /// Capture the default system audio input.
    #[default]
    Audio,
    /// Capture channel voice messages from every MIDI channel.
    MidiOmni,
    /// Capture only the conventional 1..=16 MIDI channel.
    MidiChannel(u8),
}

impl TrackInput {
    #[must_use]
    pub fn accepts_midi_channel(self, channel: u8) -> bool {
        match self {
            Self::Audio => false,
            Self::MidiOmni => (1..=16).contains(&channel),
            Self::MidiChannel(expected) => expected == channel,
        }
    }

    #[must_use]
    pub const fn is_midi(self) -> bool {
        !matches!(self, Self::Audio)
    }
}

/// A MIDI continuous-controller event at an absolute project frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiControlPoint {
    pub frame: u64,
    pub controller: u8,
    pub value: u8,
}

/// A MIDI pitch-wheel event at an absolute project frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiPitchBendPoint {
    pub frame: u64,
    pub value: i16,
}

/// A MIDI channel-pressure event at an absolute project frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiChannelPressurePoint {
    pub frame: u64,
    pub value: u8,
}

/// A MIDI polyphonic-pressure event at an absolute project frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiPolyPressurePoint {
    pub frame: u64,
    pub note: u8,
    pub value: u8,
}

/// A MIDI program-change event at an absolute project frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiProgramChangePoint {
    pub frame: u64,
    /// Conventional 1..=128 program number shown in the UI.
    pub program: u8,
}

/// Returns the last controller value at or before `frame`.
#[must_use]
pub fn midi_controller_value_at(
    points: &[MidiControlPoint],
    controller: u8,
    frame: u64,
    default: u8,
) -> u8 {
    points
        .iter()
        .filter(|point| point.controller == controller && point.frame <= frame)
        .max_by_key(|point| point.frame)
        .map_or(default, |point| point.value)
}

/// The main output destination of a track channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelOutput {
    #[default]
    Master,
    Bus(usize),
}

/// A parallel track feed into a bus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSend {
    pub bus_index: usize,
    /// Linear send gain.
    pub gain: f32,
    pub enabled: bool,
    /// Pre-fader sends tap after inserts but before track gain, pan, and
    /// automation. Post-fader sends tap the final track channel signal.
    pub pre_fader: bool,
}

impl TrackSend {
    #[must_use]
    pub const fn new(bus_index: usize) -> Self {
        Self {
            bus_index,
            gain: 1.0,
            enabled: true,
            pre_fader: false,
        }
    }
}

/// A stereo auxiliary or group channel routed to the master.
#[derive(Debug, Clone, PartialEq)]
pub struct Bus {
    pub name: String,
    pub gain: f32,
    pub pan: f32,
    pub muted: bool,
    pub soloed: bool,
    pub inserts: Vec<ChannelInsert>,
}

impl Bus {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            gain: 1.0,
            pan: 0.0,
            muted: false,
            soloed: false,
            inserts: Vec::new(),
        }
    }
}

/// One bypassable insert slot in a track, bus, or master channel.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelInsert {
    pub enabled: bool,
    pub effect: InsertEffect,
}

impl ChannelInsert {
    #[must_use]
    pub const fn new(effect: InsertEffect) -> Self {
        Self {
            enabled: true,
            effect,
        }
    }
}

/// Built-in real-time-safe channel processors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InsertEffect {
    Gain {
        gain_db: f32,
    },
    ThreeBandEq {
        low_db: f32,
        mid_db: f32,
        high_db: f32,
    },
    Compressor {
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
    },
    Saturation {
        drive: f32,
        mix: f32,
    },
}

impl InsertEffect {
    pub const ALL_DEFAULTS: [Self; 4] = [
        Self::Gain { gain_db: 0.0 },
        Self::ThreeBandEq {
            low_db: 0.0,
            mid_db: 0.0,
            high_db: 0.0,
        },
        Self::Compressor {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
        },
        Self::Saturation {
            drive: 1.0,
            mix: 1.0,
        },
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gain { .. } => "Gain",
            Self::ThreeBandEq { .. } => "3-Band EQ",
            Self::Compressor { .. } => "Compressor",
            Self::Saturation { .. } => "Saturation",
        }
    }
}

/// A non-destructive alternate audio performance on a track.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioTake {
    pub name: String,
    pub start_frame: u64,
    pub length_frames: u64,
    pub gain: f32,
    pub fade_in_frames: u64,
    pub fade_out_frames: u64,
    pub fade_curve: FadeCurve,
    pub path: String,
    pub source_offset_frames: u64,
    pub source_sample_rate: u32,
    pub channels: u16,
    pub reversed: bool,
}

impl AudioTake {
    /// Captures an audio-file clip as an alternate take.
    #[must_use]
    pub fn from_clip(clip: &Clip) -> Option<Self> {
        let ClipSource::AudioFile {
            path,
            source_offset_frames,
            source_sample_rate,
            channels,
            reversed,
        } = &clip.source
        else {
            return None;
        };
        Some(Self {
            name: clip.name.clone(),
            start_frame: clip.start_frame,
            length_frames: clip.length_frames,
            gain: clip.gain,
            fade_in_frames: clip.fade_in_frames,
            fade_out_frames: clip.fade_out_frames,
            fade_curve: clip.fade_curve,
            path: path.clone(),
            source_offset_frames: *source_offset_frames,
            source_sample_rate: *source_sample_rate,
            channels: *channels,
            reversed: *reversed,
        })
    }

    /// Builds an audible comp-lane clip from this take.
    #[must_use]
    pub fn to_clip(&self) -> Clip {
        Clip {
            name: self.name.clone(),
            start_frame: self.start_frame,
            length_frames: self.length_frames,
            gain: self.gain,
            fade_in_frames: self.fade_in_frames,
            fade_out_frames: self.fade_out_frames,
            fade_curve: self.fade_curve,
            source: ClipSource::AudioFile {
                path: self.path.clone(),
                source_offset_frames: self.source_offset_frames,
                source_sample_rate: self.source_sample_rate,
                channels: self.channels,
                reversed: self.reversed,
            },
        }
    }

    #[must_use]
    pub const fn end_frame(&self) -> u64 {
        self.start_frame.saturating_add(self.length_frames)
    }
}

/// Recording and live-input state for a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrackRecording {
    /// Whether this track receives newly recorded input clips.
    pub armed: bool,
    /// Whether live input is routed to the output while recording.
    pub input_monitoring: bool,
}

/// A point on a sample-accurate linear automation envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutomationPoint {
    pub frame: u64,
    /// Linear multiplier. A value of 1.0 is unity gain.
    pub value: f32,
}

/// Evaluates a linear automation envelope. Values before/after the point range
/// hold the first/last point. An empty envelope evaluates to unity.
#[must_use]
pub fn automation_value_at(points: &[AutomationPoint], frame: u64) -> f32 {
    let before = points
        .iter()
        .filter(|point| point.frame <= frame)
        .max_by_key(|point| point.frame);
    let after = points
        .iter()
        .filter(|point| point.frame >= frame)
        .min_by_key(|point| point.frame);
    match (before, after) {
        (None, None) => 1.0,
        (Some(point), None) | (None, Some(point)) => point.value,
        (Some(before), Some(after)) if before.frame == after.frame => before.value,
        (Some(before), Some(after)) => {
            #[allow(clippy::cast_precision_loss)]
            let position = (frame - before.frame) as f32 / (after.frame - before.frame) as f32;
            before.value + (after.value - before.value) * position
        }
    }
}

/// Built-in oscillator instruments available to MIDI tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Instrument {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
}

impl Instrument {
    pub const ALL: [Self; 4] = [Self::Sine, Self::Triangle, Self::Saw, Self::Square];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sine => "Sine",
            Self::Triangle => "Triangle",
            Self::Saw => "Saw",
            Self::Square => "Square",
        }
    }
}

/// A source placed at a sample-accurate position on a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub name: String,
    pub start_frame: u64,
    pub length_frames: u64,
    /// Linear clip gain applied before the track mixer.
    pub gain: f32,
    /// Non-destructive fade-in duration in project frames.
    pub fade_in_frames: u64,
    /// Non-destructive fade-out duration in project frames.
    pub fade_out_frames: u64,
    /// Shape used by both fade-in and fade-out envelopes.
    pub fade_curve: FadeCurve,
    pub source: ClipSource,
}

impl Clip {
    #[must_use]
    pub const fn end_frame(&self) -> u64 {
        self.start_frame.saturating_add(self.length_frames)
    }
}

/// Per-clip non-destructive fade curve shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FadeCurve {
    #[default]
    Linear,
    EqualPower,
    Slow,
    Fast,
}

impl FadeCurve {
    pub const ALL: [Self; 4] = [Self::Linear, Self::EqualPower, Self::Slow, Self::Fast];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::EqualPower => "Equal Power",
            Self::Slow => "Slow",
            Self::Fast => "Fast",
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::EqualPower => "equal_power",
            Self::Slow => "slow",
            Self::Fast => "fast",
        }
    }

    #[must_use]
    pub fn gain(self, normalized: f32) -> f32 {
        let t = normalized.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
            Self::Slow => t * t,
            Self::Fast => 1.0 - (1.0 - t) * (1.0 - t),
        }
    }
}

/// Audio data produced by a clip.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipSource {
    /// A generated instrument part containing notes relative to the clip start.
    Midi {
        notes: Vec<MidiNote>,
        amplitude: f32,
    },
    /// Legacy generated tone clips are retained for project-file compatibility.
    Sine { frequency_hz: f32, amplitude: f32 },
    /// A WAV file decoded outside the real-time audio callback.
    ///
    /// `source_offset_frames` is measured in frames at `source_sample_rate`,
    /// not in project timeline frames. The sample rate and channel count are
    /// persisted so the UI can describe a clip without probing the file.
    AudioFile {
        path: String,
        source_offset_frames: u64,
        source_sample_rate: u32,
        channels: u16,
        reversed: bool,
    },
}

/// A note stored inside a MIDI clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNote {
    /// Position relative to the owning clip's start, in project frames.
    pub start_frame: u64,
    pub length_frames: u64,
    pub midi_note: u8,
    /// MIDI-style note velocity in the inclusive range 1..=127.
    pub velocity: u8,
}

impl MidiNote {
    #[must_use]
    pub const fn end_frame(self) -> u64 {
        self.start_frame.saturating_add(self.length_frames)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectError {
    InvalidSampleRate(u32),
    InvalidTempo(f64),
    InvalidTimeSignature { numerator: u8, denominator: u8 },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate(rate) => write!(formatter, "invalid sample rate: {rate}"),
            Self::InvalidTempo(tempo) => write!(formatter, "invalid tempo: {tempo}"),
            Self::InvalidTimeSignature {
                numerator,
                denominator,
            } => write!(
                formatter,
                "invalid time signature: {numerator}/{denominator}"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_is_the_latest_clip_end() {
        let mut project = Project::new("Test", 48_000, 120.0).unwrap();
        let mut track = Track::new("Tone");
        track.clips.push(Clip {
            name: "A".into(),
            start_frame: 200,
            length_frames: 300,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::Sine {
                frequency_hz: 440.0,
                amplitude: 0.5,
            },
        });
        project.tracks.push(track);
        assert_eq!(project.duration_frames(), 500);
    }

    #[test]
    fn rejects_invalid_project_settings() {
        assert!(Project::new("Test", 100, 120.0).is_err());
        assert!(Project::new("Test", 48_000, f64::NAN).is_err());
    }

    #[test]
    fn automation_interpolates_and_holds_edge_values() {
        let points = [
            AutomationPoint {
                frame: 100,
                value: 0.5,
            },
            AutomationPoint {
                frame: 300,
                value: 1.5,
            },
        ];

        assert_eq!(automation_value_at(&[], 200).to_bits(), 1.0_f32.to_bits());
        assert_eq!(automation_value_at(&points, 0).to_bits(), 0.5_f32.to_bits());
        assert_eq!(
            automation_value_at(&points, 200).to_bits(),
            1.0_f32.to_bits()
        );
        assert_eq!(
            automation_value_at(&points, 400).to_bits(),
            1.5_f32.to_bits()
        );
    }

    #[test]
    fn midi_controller_uses_the_latest_matching_point() {
        let points = [
            MidiControlPoint {
                frame: 10,
                controller: 11,
                value: 64,
            },
            MidiControlPoint {
                frame: 20,
                controller: 1,
                value: 90,
            },
            MidiControlPoint {
                frame: 30,
                controller: 11,
                value: 100,
            },
        ];
        assert_eq!(midi_controller_value_at(&points, 11, 0, 127), 127);
        assert_eq!(midi_controller_value_at(&points, 11, 25, 127), 64);
        assert_eq!(midi_controller_value_at(&points, 11, 30, 127), 100);
    }

    #[test]
    fn fade_curves_have_distinct_shapes() {
        let midpoint = 0.5;

        assert!((FadeCurve::Linear.gain(midpoint) - 0.5).abs() < f32::EPSILON);
        assert!(FadeCurve::Slow.gain(midpoint) < FadeCurve::Linear.gain(midpoint));
        assert!(FadeCurve::Fast.gain(midpoint) > FadeCurve::Linear.gain(midpoint));
        assert!(FadeCurve::EqualPower.gain(midpoint) > FadeCurve::Linear.gain(midpoint));
        assert_eq!(FadeCurve::Linear.gain(-1.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(FadeCurve::Linear.gain(2.0).to_bits(), 1.0_f32.to_bits());
    }
}
