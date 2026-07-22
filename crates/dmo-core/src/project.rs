use std::fmt;

/// A complete editable song.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub sample_rate: u32,
    pub tempo_bpm: f64,
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
            midi_channel: 1,
            midi_cc: Vec::new(),
            midi_pitch_bend: Vec::new(),
            midi_channel_pressure: Vec::new(),
            midi_poly_pressure: Vec::new(),
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
    pub path: String,
    pub source_offset_frames: u64,
    pub source_sample_rate: u32,
    pub channels: u16,
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
            path: path.clone(),
            source_offset_frames: *source_offset_frames,
            source_sample_rate: *source_sample_rate,
            channels: *channels,
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
            source: ClipSource::AudioFile {
                path: self.path.clone(),
                source_offset_frames: self.source_offset_frames,
                source_sample_rate: self.source_sample_rate,
                channels: self.channels,
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
    pub source: ClipSource,
}

impl Clip {
    #[must_use]
    pub const fn end_frame(&self) -> u64 {
        self.start_frame.saturating_add(self.length_frames)
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
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate(rate) => write!(formatter, "invalid sample rate: {rate}"),
            Self::InvalidTempo(tempo) => write!(formatter, "invalid tempo: {tempo}"),
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
}
