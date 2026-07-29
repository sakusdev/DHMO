use std::{
    fmt, fs,
    path::Path,
    str::{FromStr, Lines},
};

use crate::{
    ArrangerSection, AudioTake, AutomationPoint, Bus, ChannelInsert, ChannelOutput, Clip,
    ClipSource, CycleRange, FadeCurve, InsertEffect, Instrument, Marker, MidiChannelPressurePoint,
    MidiControlPoint, MidiNote, MidiPitchBendPoint, MidiPolyPressurePoint, MidiProgramChangePoint,
    Project, ProjectError, SoundFontPreset, TimeSignature, Track, TrackInput, TrackRecording,
    TrackSend,
};

/// The project-file format version written by this release.
pub const PROJECT_FILE_VERSION: u32 = 21;

const LEGACY_PROJECT_FILE_VERSION: u32 = 1;
const AUDIO_PROJECT_FILE_VERSION: u32 = 2;
const MIDI_PROJECT_FILE_VERSION: u32 = 3;
const ROUTING_PROJECT_FILE_VERSION: u32 = 4;
const EDITING_PROJECT_FILE_VERSION: u32 = 5;
const VELOCITY_PROJECT_FILE_VERSION: u32 = 6;
const CLIP_MIX_PROJECT_FILE_VERSION: u32 = 7;
const AUTOMATION_PROJECT_FILE_VERSION: u32 = 8;
const RECORDING_PROJECT_FILE_VERSION: u32 = 9;
const TAKE_LANE_PROJECT_FILE_VERSION: u32 = 10;
const MIXER_PROJECT_FILE_VERSION: u32 = 11;
const LIVE_MIDI_PROJECT_FILE_VERSION: u32 = 12;
const MARKER_PROJECT_FILE_VERSION: u32 = 13;
const ARRANGER_PROJECT_FILE_VERSION: u32 = 14;
const CYCLE_PROJECT_FILE_VERSION: u32 = 15;
const FADE_CURVE_PROJECT_FILE_VERSION: u32 = 16;
const PROGRAM_CHANGE_PROJECT_FILE_VERSION: u32 = 17;
const AUDIO_REVERSE_PROJECT_FILE_VERSION: u32 = 18;
const TIME_SIGNATURE_PROJECT_FILE_VERSION: u32 = 19;
const SOUNDFONT_PROJECT_FILE_VERSION: u32 = 20;
const AUDIO_INPUT_ROUTING_PROJECT_FILE_VERSION: u32 = 21;

const FILE_HEADER: &str = "DMO_PROJECT";

/// An error produced while encoding, decoding, saving, or loading a DMO project.
#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub enum ProjectFileError {
    /// The project model contains a value that cannot be persisted safely.
    InvalidProject(ProjectError),
    /// A field in the in-memory project is invalid.
    InvalidField {
        field: &'static str,
        message: String,
    },
    /// The file is not valid DMO project syntax.
    InvalidData { line: usize, message: String },
    /// The file uses a newer or otherwise unsupported format version.
    UnsupportedVersion(u32),
    /// Reading or writing the file failed.
    Io(std::io::Error),
}

impl fmt::Display for ProjectFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProject(error) => write!(formatter, "invalid project: {error}"),
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid project field `{field}`: {message}")
            }
            Self::InvalidData { line, message } => {
                write!(formatter, "invalid project file at line {line}: {message}")
            }
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported DMO project version: {version}")
            }
            Self::Io(error) => write!(formatter, "project file I/O failed: {error}"),
        }
    }
}

impl std::error::Error for ProjectFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidProject(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InvalidField { .. } | Self::InvalidData { .. } | Self::UnsupportedVersion(_) => {
                None
            }
        }
    }
}

impl From<std::io::Error> for ProjectFileError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Encodes a project in the current human-readable DMO project format.
///
/// # Errors
///
/// Returns [`ProjectFileError::InvalidProject`] when core project settings are
/// invalid, or [`ProjectFileError::InvalidField`] when a mixer or source value
/// is not finite.
#[allow(clippy::too_many_lines)]
pub fn encode_project(project: &Project) -> Result<String, ProjectFileError> {
    validate_project(project)?;

    let mut output = String::new();
    push_value(&mut output, FILE_HEADER, &PROJECT_FILE_VERSION);
    push_string(&mut output, "name", &project.name);
    push_value(&mut output, "sample_rate", &project.sample_rate);
    push_value(&mut output, "tempo_bpm", &project.tempo_bpm);
    push_value(
        &mut output,
        "time_signature_numerator",
        &project.time_signature.numerator,
    );
    push_value(
        &mut output,
        "time_signature_denominator",
        &project.time_signature.denominator,
    );
    match project.cycle_range {
        Some(range) => {
            push_value(&mut output, "cycle_enabled", &true);
            push_value(&mut output, "cycle_start_frame", &range.start_frame);
            push_value(&mut output, "cycle_end_frame", &range.end_frame);
        }
        None => push_value(&mut output, "cycle_enabled", &false),
    }
    push_value(&mut output, "markers", &project.markers.len());
    for marker in &project.markers {
        output.push_str("marker\n");
        push_string(&mut output, "name", &marker.name);
        push_value(&mut output, "frame", &marker.frame);
        output.push_str("end_marker\n");
    }
    push_value(
        &mut output,
        "arranger_sections",
        &project.arranger_sections.len(),
    );
    for section in &project.arranger_sections {
        output.push_str("arranger_section\n");
        push_string(&mut output, "name", &section.name);
        push_value(&mut output, "start_frame", &section.start_frame);
        push_value(&mut output, "length_frames", &section.length_frames);
        push_value(&mut output, "color_index", &section.color_index);
        output.push_str("end_arranger_section\n");
    }
    push_value(&mut output, "master_gain", &project.master_gain);
    encode_insert_chain(&mut output, "master_inserts", &project.master_inserts);
    push_value(&mut output, "buses", &project.buses.len());
    for bus in &project.buses {
        output.push_str("bus\n");
        push_string(&mut output, "name", &bus.name);
        push_value(&mut output, "gain", &bus.gain);
        push_value(&mut output, "pan", &bus.pan);
        push_value(&mut output, "muted", &bus.muted);
        push_value(&mut output, "soloed", &bus.soloed);
        encode_insert_chain(&mut output, "inserts", &bus.inserts);
        output.push_str("end_bus\n");
    }
    push_value(&mut output, "tracks", &project.tracks.len());

    for track in &project.tracks {
        output.push_str("track\n");
        push_string(&mut output, "name", &track.name);
        push_value(&mut output, "gain", &track.gain);
        push_value(&mut output, "pan", &track.pan);
        push_value(&mut output, "muted", &track.muted);
        push_value(&mut output, "soloed", &track.soloed);
        push_value(&mut output, "record_armed", &track.recording.armed);
        push_value(
            &mut output,
            "input_monitoring",
            &track.recording.input_monitoring,
        );
        match track.input {
            TrackInput::Audio => push_value(&mut output, "record_input", &"audio"),
            TrackInput::AudioMonoLeft => {
                push_value(&mut output, "record_input", &"audio_mono_left");
            }
            TrackInput::AudioMonoRight => {
                push_value(&mut output, "record_input", &"audio_mono_right");
            }
            TrackInput::MidiOmni => push_value(&mut output, "record_input", &"midi_omni"),
            TrackInput::MidiChannel(channel) => {
                push_value(&mut output, "record_input", &"midi_channel");
                push_value(&mut output, "record_input_channel", &channel);
            }
        }
        match track.output {
            ChannelOutput::Master => push_value(&mut output, "output", &"master"),
            ChannelOutput::Bus(index) => {
                push_value(&mut output, "output", &"bus");
                push_value(&mut output, "output_bus_index", &index);
            }
        }
        push_value(&mut output, "sends", &track.sends.len());
        for send in &track.sends {
            output.push_str("send\n");
            push_value(&mut output, "bus_index", &send.bus_index);
            push_value(&mut output, "gain", &send.gain);
            push_value(&mut output, "enabled", &send.enabled);
            push_value(&mut output, "pre_fader", &send.pre_fader);
            output.push_str("end_send\n");
        }
        encode_insert_chain(&mut output, "inserts", &track.inserts);
        push_value(
            &mut output,
            "instrument",
            &track.instrument.label().to_ascii_lowercase(),
        );
        push_value(&mut output, "midi_channel", &track.midi_channel);
        push_value(&mut output, "soundfont_enabled", &track.soundfont.is_some());
        if let Some(soundfont) = &track.soundfont {
            push_string(&mut output, "soundfont_path", &soundfont.path);
            push_value(&mut output, "soundfont_bank", &soundfont.bank);
            push_value(&mut output, "soundfont_program", &soundfont.program);
            push_string(&mut output, "soundfont_name", &soundfont.name);
        }
        push_value(&mut output, "midi_cc_points", &track.midi_cc.len());
        for point in &track.midi_cc {
            output.push_str("midi_cc\n");
            push_value(&mut output, "frame", &point.frame);
            push_value(&mut output, "controller", &point.controller);
            push_value(&mut output, "value", &point.value);
            output.push_str("end_midi_cc\n");
        }
        push_value(
            &mut output,
            "midi_pitch_bend_points",
            &track.midi_pitch_bend.len(),
        );
        for point in &track.midi_pitch_bend {
            output.push_str("midi_pitch_bend\n");
            push_value(&mut output, "frame", &point.frame);
            push_value(&mut output, "value", &point.value);
            output.push_str("end_midi_pitch_bend\n");
        }
        push_value(
            &mut output,
            "midi_channel_pressure_points",
            &track.midi_channel_pressure.len(),
        );
        for point in &track.midi_channel_pressure {
            output.push_str("midi_channel_pressure\n");
            push_value(&mut output, "frame", &point.frame);
            push_value(&mut output, "value", &point.value);
            output.push_str("end_midi_channel_pressure\n");
        }
        push_value(
            &mut output,
            "midi_poly_pressure_points",
            &track.midi_poly_pressure.len(),
        );
        for point in &track.midi_poly_pressure {
            output.push_str("midi_poly_pressure\n");
            push_value(&mut output, "frame", &point.frame);
            push_value(&mut output, "note", &point.note);
            push_value(&mut output, "value", &point.value);
            output.push_str("end_midi_poly_pressure\n");
        }
        push_value(
            &mut output,
            "midi_program_changes",
            &track.midi_program_changes.len(),
        );
        for point in &track.midi_program_changes {
            output.push_str("midi_program_change\n");
            push_value(&mut output, "frame", &point.frame);
            push_value(&mut output, "program", &point.program);
            output.push_str("end_midi_program_change\n");
        }
        push_value(
            &mut output,
            "volume_automation_points",
            &track.volume_automation.len(),
        );
        for point in &track.volume_automation {
            output.push_str("automation_point\n");
            push_value(&mut output, "frame", &point.frame);
            push_value(&mut output, "value", &point.value);
            output.push_str("end_automation_point\n");
        }
        push_value(&mut output, "clips", &track.clips.len());

        for clip in &track.clips {
            output.push_str("clip\n");
            push_string(&mut output, "name", &clip.name);
            push_value(&mut output, "start_frame", &clip.start_frame);
            push_value(&mut output, "length_frames", &clip.length_frames);
            push_value(&mut output, "clip_gain", &clip.gain);
            push_value(&mut output, "fade_in_frames", &clip.fade_in_frames);
            push_value(&mut output, "fade_out_frames", &clip.fade_out_frames);
            push_value(&mut output, "fade_curve", &clip.fade_curve.as_str());
            match &clip.source {
                ClipSource::Midi { notes, amplitude } => {
                    output.push_str("source midi\n");
                    push_value(&mut output, "amplitude", amplitude);
                    push_value(&mut output, "notes", &notes.len());
                    for note in notes {
                        output.push_str("note\n");
                        push_value(&mut output, "start_frame", &note.start_frame);
                        push_value(&mut output, "length_frames", &note.length_frames);
                        push_value(&mut output, "midi_note", &note.midi_note);
                        push_value(&mut output, "velocity", &note.velocity);
                        output.push_str("end_note\n");
                    }
                }
                ClipSource::Sine {
                    frequency_hz,
                    amplitude,
                } => {
                    output.push_str("source sine\n");
                    push_value(&mut output, "frequency_hz", frequency_hz);
                    push_value(&mut output, "amplitude", amplitude);
                }
                ClipSource::AudioFile {
                    path,
                    source_offset_frames,
                    source_sample_rate,
                    channels,
                    reversed,
                } => {
                    output.push_str("source audio_file\n");
                    push_string(&mut output, "path", path);
                    push_value(&mut output, "source_offset_frames", source_offset_frames);
                    push_value(&mut output, "source_sample_rate", source_sample_rate);
                    push_value(&mut output, "channels", channels);
                    push_value(&mut output, "reversed", reversed);
                }
            }
            output.push_str("end_clip\n");
        }
        push_value(&mut output, "take_lanes", &track.take_lanes.len());
        for take in &track.take_lanes {
            encode_take(&mut output, take);
        }
        output.push_str("end_track\n");
    }

    output.push_str("end_project\n");
    Ok(output)
}

fn encode_take(output: &mut String, take: &AudioTake) {
    output.push_str("take_lane\n");
    push_string(output, "name", &take.name);
    push_value(output, "start_frame", &take.start_frame);
    push_value(output, "length_frames", &take.length_frames);
    push_value(output, "clip_gain", &take.gain);
    push_value(output, "fade_in_frames", &take.fade_in_frames);
    push_value(output, "fade_out_frames", &take.fade_out_frames);
    push_value(output, "fade_curve", &take.fade_curve.as_str());
    push_string(output, "path", &take.path);
    push_value(output, "source_offset_frames", &take.source_offset_frames);
    push_value(output, "source_sample_rate", &take.source_sample_rate);
    push_value(output, "channels", &take.channels);
    push_value(output, "reversed", &take.reversed);
    output.push_str("end_take_lane\n");
}

fn encode_insert_chain(output: &mut String, count_field: &str, inserts: &[ChannelInsert]) {
    push_value(output, count_field, &inserts.len());
    for insert in inserts {
        output.push_str("insert\n");
        push_value(output, "enabled", &insert.enabled);
        match insert.effect {
            InsertEffect::Gain { gain_db } => {
                output.push_str("effect gain\n");
                push_value(output, "gain_db", &gain_db);
            }
            InsertEffect::ThreeBandEq {
                low_db,
                mid_db,
                high_db,
            } => {
                output.push_str("effect three_band_eq\n");
                push_value(output, "low_db", &low_db);
                push_value(output, "mid_db", &mid_db);
                push_value(output, "high_db", &high_db);
            }
            InsertEffect::Compressor {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            } => {
                output.push_str("effect compressor\n");
                push_value(output, "threshold_db", &threshold_db);
                push_value(output, "ratio", &ratio);
                push_value(output, "attack_ms", &attack_ms);
                push_value(output, "release_ms", &release_ms);
                push_value(output, "makeup_db", &makeup_db);
            }
            InsertEffect::Saturation { drive, mix } => {
                output.push_str("effect saturation\n");
                push_value(output, "drive", &drive);
                push_value(output, "mix", &mix);
            }
        }
        output.push_str("end_insert\n");
    }
}

/// Decodes a project from the human-readable DMO project format.
///
/// Empty lines and lines beginning with `#` are ignored so hand-edited files
/// can contain comments.
///
/// # Errors
///
/// Returns a descriptive [`ProjectFileError`] for malformed syntax, unsupported
/// versions, invalid field values, or invalid core project settings.
#[allow(clippy::too_many_lines)]
pub fn decode_project(input: &str) -> Result<Project, ProjectFileError> {
    let mut parser = Parser::new(input);

    let (header_line, version_text) = parser.field(FILE_HEADER)?;
    let version = parse_value::<u32>(header_line, FILE_HEADER, version_text)?;
    if !matches!(
        version,
        LEGACY_PROJECT_FILE_VERSION
            | AUDIO_PROJECT_FILE_VERSION
            | MIDI_PROJECT_FILE_VERSION
            | ROUTING_PROJECT_FILE_VERSION
            | EDITING_PROJECT_FILE_VERSION
            | VELOCITY_PROJECT_FILE_VERSION
            | CLIP_MIX_PROJECT_FILE_VERSION
            | AUTOMATION_PROJECT_FILE_VERSION
            | RECORDING_PROJECT_FILE_VERSION
            | TAKE_LANE_PROJECT_FILE_VERSION
            | MIXER_PROJECT_FILE_VERSION
            | LIVE_MIDI_PROJECT_FILE_VERSION
            | MARKER_PROJECT_FILE_VERSION
            | ARRANGER_PROJECT_FILE_VERSION
            | CYCLE_PROJECT_FILE_VERSION
            | FADE_CURVE_PROJECT_FILE_VERSION
            | PROGRAM_CHANGE_PROJECT_FILE_VERSION
            | AUDIO_REVERSE_PROJECT_FILE_VERSION
            | TIME_SIGNATURE_PROJECT_FILE_VERSION
            | SOUNDFONT_PROJECT_FILE_VERSION
            | PROJECT_FILE_VERSION
    ) {
        return Err(ProjectFileError::UnsupportedVersion(version));
    }

    let (_, name) = parser.string_field("name")?;
    let (_, sample_rate) = parser.value_field::<u32>("sample_rate")?;
    let (_, tempo_bpm) = parser.value_field::<f64>("tempo_bpm")?;
    let time_signature = if version >= TIME_SIGNATURE_PROJECT_FILE_VERSION {
        let (numerator_line, numerator) = parser.value_field::<u8>("time_signature_numerator")?;
        let (_, denominator) = parser.value_field::<u8>("time_signature_denominator")?;
        TimeSignature::new(numerator, denominator).ok_or_else(|| {
            invalid_data(
                numerator_line,
                format!("invalid time signature `{numerator}/{denominator}`"),
            )
        })?
    } else {
        TimeSignature::default()
    };
    let cycle_range = if version >= CYCLE_PROJECT_FILE_VERSION {
        let (_, cycle_enabled) = parser.value_field::<bool>("cycle_enabled")?;
        if cycle_enabled {
            let (_, start_frame) = parser.value_field::<u64>("cycle_start_frame")?;
            let (end_line, end_frame) = parser.value_field::<u64>("cycle_end_frame")?;
            Some(CycleRange::new(start_frame, end_frame).ok_or_else(|| {
                invalid_data(
                    end_line,
                    "`cycle_end_frame` must be greater than `cycle_start_frame`".into(),
                )
            })?)
        } else {
            None
        }
    } else {
        None
    };
    let markers = if version >= MARKER_PROJECT_FILE_VERSION {
        let (_, marker_count) = parser.value_field::<usize>("markers")?;
        let mut markers = Vec::with_capacity(marker_count);
        for _ in 0..marker_count {
            parser.literal("marker")?;
            let (_, name) = parser.string_field("name")?;
            let (_, frame) = parser.value_field::<u64>("frame")?;
            parser.literal("end_marker")?;
            markers.push(Marker { name, frame });
        }
        markers
    } else {
        Vec::new()
    };
    let arranger_sections = if version >= ARRANGER_PROJECT_FILE_VERSION {
        let (_, section_count) = parser.value_field::<usize>("arranger_sections")?;
        let mut sections = Vec::with_capacity(section_count);
        for _ in 0..section_count {
            parser.literal("arranger_section")?;
            let (_, name) = parser.string_field("name")?;
            let (_, start_frame) = parser.value_field::<u64>("start_frame")?;
            let (length_line, length_frames) = parser.value_field::<u64>("length_frames")?;
            if length_frames == 0 {
                return Err(invalid_data(
                    length_line,
                    "`length_frames` must be greater than zero".into(),
                ));
            }
            let (_, color_index) = parser.value_field::<u8>("color_index")?;
            parser.literal("end_arranger_section")?;
            sections.push(ArrangerSection {
                name,
                start_frame,
                length_frames,
                color_index,
            });
        }
        sections
    } else {
        Vec::new()
    };
    let master_gain = if version >= AUTOMATION_PROJECT_FILE_VERSION {
        let (line, gain) = parser.value_field::<f32>("master_gain")?;
        require_finite(gain, line, "master_gain")?;
        if gain < 0.0 {
            return Err(invalid_data(
                line,
                "`master_gain` must be non-negative".into(),
            ));
        }
        gain
    } else {
        1.0
    };
    let mut project =
        Project::new(name, sample_rate, tempo_bpm).map_err(ProjectFileError::InvalidProject)?;
    project.cycle_range = cycle_range;
    project.time_signature = time_signature;
    project.markers = markers;
    project.arranger_sections = arranger_sections;
    project.master_gain = master_gain;
    if version >= MIXER_PROJECT_FILE_VERSION {
        project.master_inserts = decode_insert_chain(&mut parser, "master_inserts")?;
        let (_, bus_count) = parser.value_field::<usize>("buses")?;
        project.buses.reserve(bus_count);
        for _ in 0..bus_count {
            parser.literal("bus")?;
            let (_, name) = parser.string_field("name")?;
            let (gain_line, gain) = parser.value_field::<f32>("gain")?;
            require_non_negative_finite(gain, gain_line, "gain")?;
            let (pan_line, pan) = parser.value_field::<f32>("pan")?;
            require_finite(pan, pan_line, "pan")?;
            let (_, muted) = parser.value_field::<bool>("muted")?;
            let (_, soloed) = parser.value_field::<bool>("soloed")?;
            let inserts = decode_insert_chain(&mut parser, "inserts")?;
            parser.literal("end_bus")?;
            project.buses.push(Bus {
                name,
                gain,
                pan,
                muted,
                soloed,
                inserts,
            });
        }
    }
    let (_, track_count) = parser.value_field::<usize>("tracks")?;

    for _ in 0..track_count {
        parser.literal("track")?;
        let (_, track_name) = parser.string_field("name")?;
        let (gain_line, gain) = parser.value_field::<f32>("gain")?;
        require_finite(gain, gain_line, "gain")?;
        let (pan_line, pan) = parser.value_field::<f32>("pan")?;
        require_finite(pan, pan_line, "pan")?;
        let (_, muted) = parser.value_field::<bool>("muted")?;
        let soloed = if version >= EDITING_PROJECT_FILE_VERSION {
            parser.value_field::<bool>("soloed")?.1
        } else {
            false
        };
        let (record_armed, input_monitoring) = if version >= RECORDING_PROJECT_FILE_VERSION {
            (
                parser.value_field::<bool>("record_armed")?.1,
                parser.value_field::<bool>("input_monitoring")?.1,
            )
        } else {
            (false, false)
        };
        let input = if version >= LIVE_MIDI_PROJECT_FILE_VERSION {
            let (line, input_name) = parser.field("record_input")?;
            match input_name {
                "audio" => TrackInput::Audio,
                "audio_mono_left" if version >= AUDIO_INPUT_ROUTING_PROJECT_FILE_VERSION => {
                    TrackInput::AudioMonoLeft
                }
                "audio_mono_right" if version >= AUDIO_INPUT_ROUTING_PROJECT_FILE_VERSION => {
                    TrackInput::AudioMonoRight
                }
                "midi_omni" => TrackInput::MidiOmni,
                "midi_channel" => {
                    let (channel_line, channel) =
                        parser.value_field::<u8>("record_input_channel")?;
                    if !(1..=16).contains(&channel) {
                        return Err(invalid_data(
                            channel_line,
                            "`record_input_channel` must be between 1 and 16".into(),
                        ));
                    }
                    TrackInput::MidiChannel(channel)
                }
                other => {
                    return Err(invalid_data(
                        line,
                        format!("unknown recording input `{other}`"),
                    ));
                }
            }
        } else {
            TrackInput::Audio
        };
        let (output, sends, inserts) = if version >= MIXER_PROJECT_FILE_VERSION {
            let (output_line, output_name) = parser.field("output")?;
            let output = match output_name {
                "master" => ChannelOutput::Master,
                "bus" => ChannelOutput::Bus(parser.value_field::<usize>("output_bus_index")?.1),
                other => {
                    return Err(invalid_data(
                        output_line,
                        format!("unknown channel output `{other}`"),
                    ));
                }
            };
            let (_, send_count) = parser.value_field::<usize>("sends")?;
            let mut sends = Vec::with_capacity(send_count);
            for _ in 0..send_count {
                parser.literal("send")?;
                let (_, bus_index) = parser.value_field::<usize>("bus_index")?;
                let (gain_line, gain) = parser.value_field::<f32>("gain")?;
                require_non_negative_finite(gain, gain_line, "gain")?;
                let (_, enabled) = parser.value_field::<bool>("enabled")?;
                let (_, pre_fader) = parser.value_field::<bool>("pre_fader")?;
                parser.literal("end_send")?;
                sends.push(TrackSend {
                    bus_index,
                    gain,
                    enabled,
                    pre_fader,
                });
            }
            let inserts = decode_insert_chain(&mut parser, "inserts")?;
            (output, sends, inserts)
        } else {
            (ChannelOutput::Master, Vec::new(), Vec::new())
        };
        let (instrument, midi_channel) = if version >= ROUTING_PROJECT_FILE_VERSION {
            let (instrument_line, instrument_name) = parser.field("instrument")?;
            let instrument = match instrument_name {
                "sine" => Instrument::Sine,
                "triangle" => Instrument::Triangle,
                "saw" => Instrument::Saw,
                "square" => Instrument::Square,
                other => {
                    return Err(invalid_data(
                        instrument_line,
                        format!("unknown built-in instrument `{other}`"),
                    ));
                }
            };
            let (channel_line, midi_channel) = parser.value_field::<u8>("midi_channel")?;
            if !(1..=16).contains(&midi_channel) {
                return Err(invalid_data(
                    channel_line,
                    "`midi_channel` must be between 1 and 16".into(),
                ));
            }
            (instrument, midi_channel)
        } else {
            (Instrument::Sine, 1)
        };
        let soundfont = if version >= SOUNDFONT_PROJECT_FILE_VERSION {
            let (_, enabled) = parser.value_field::<bool>("soundfont_enabled")?;
            if enabled {
                let (_, path) = parser.string_field("soundfont_path")?;
                let (_, bank) = parser.value_field::<u16>("soundfont_bank")?;
                let (program_line, program) = parser.value_field::<u16>("soundfont_program")?;
                if program > 127 {
                    return Err(invalid_data(
                        program_line,
                        "SoundFont program must be between 0 and 127".into(),
                    ));
                }
                let (_, name) = parser.string_field("soundfont_name")?;
                Some(SoundFontPreset::new(path, bank, program, name))
            } else {
                None
            }
        } else {
            None
        };
        let midi_cc = if version >= MIXER_PROJECT_FILE_VERSION {
            let (_, point_count) = parser.value_field::<usize>("midi_cc_points")?;
            let mut points = Vec::with_capacity(point_count);
            for _ in 0..point_count {
                parser.literal("midi_cc")?;
                let (_, frame) = parser.value_field::<u64>("frame")?;
                let (_, controller) = parser.value_field::<u8>("controller")?;
                let (_, value) = parser.value_field::<u8>("value")?;
                if controller > 127 || value > 127 {
                    return Err(invalid_data(
                        parser.next_line.saturating_sub(1),
                        "MIDI controller and value must be between 0 and 127".into(),
                    ));
                }
                parser.literal("end_midi_cc")?;
                points.push(MidiControlPoint {
                    frame,
                    controller,
                    value,
                });
            }
            points.sort_by_key(|point| (point.frame, point.controller));
            points
        } else {
            Vec::new()
        };
        let (midi_pitch_bend, midi_channel_pressure, midi_poly_pressure) = if version
            >= LIVE_MIDI_PROJECT_FILE_VERSION
        {
            let (_, pitch_count) = parser.value_field::<usize>("midi_pitch_bend_points")?;
            let mut pitch = Vec::with_capacity(pitch_count);
            for _ in 0..pitch_count {
                parser.literal("midi_pitch_bend")?;
                let (_, frame) = parser.value_field::<u64>("frame")?;
                let (value_line, value) = parser.value_field::<i16>("value")?;
                if !(-8192..=8191).contains(&value) {
                    return Err(invalid_data(
                        value_line,
                        "MIDI pitch bend must be between -8192 and 8191".into(),
                    ));
                }
                parser.literal("end_midi_pitch_bend")?;
                pitch.push(MidiPitchBendPoint { frame, value });
            }
            pitch.sort_by_key(|point| point.frame);

            let (_, channel_count) = parser.value_field::<usize>("midi_channel_pressure_points")?;
            let mut channel_pressure = Vec::with_capacity(channel_count);
            for _ in 0..channel_count {
                parser.literal("midi_channel_pressure")?;
                let (_, frame) = parser.value_field::<u64>("frame")?;
                let (value_line, value) = parser.value_field::<u8>("value")?;
                if value > 127 {
                    return Err(invalid_data(
                        value_line,
                        "MIDI channel pressure must be between 0 and 127".into(),
                    ));
                }
                parser.literal("end_midi_channel_pressure")?;
                channel_pressure.push(MidiChannelPressurePoint { frame, value });
            }
            channel_pressure.sort_by_key(|point| point.frame);

            let (_, poly_count) = parser.value_field::<usize>("midi_poly_pressure_points")?;
            let mut poly_pressure = Vec::with_capacity(poly_count);
            for _ in 0..poly_count {
                parser.literal("midi_poly_pressure")?;
                let (_, frame) = parser.value_field::<u64>("frame")?;
                let (note_line, note) = parser.value_field::<u8>("note")?;
                let (value_line, value) = parser.value_field::<u8>("value")?;
                if note > 127 || value > 127 {
                    return Err(invalid_data(
                        note_line.max(value_line),
                        "MIDI poly pressure note and value must be between 0 and 127".into(),
                    ));
                }
                parser.literal("end_midi_poly_pressure")?;
                poly_pressure.push(MidiPolyPressurePoint { frame, note, value });
            }
            poly_pressure.sort_by_key(|point| (point.frame, point.note));
            (pitch, channel_pressure, poly_pressure)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        let midi_program_changes = if version >= PROGRAM_CHANGE_PROJECT_FILE_VERSION {
            let (_, point_count) = parser.value_field::<usize>("midi_program_changes")?;
            let mut points = Vec::with_capacity(point_count);
            for _ in 0..point_count {
                parser.literal("midi_program_change")?;
                let (_, frame) = parser.value_field::<u64>("frame")?;
                let (program_line, program) = parser.value_field::<u8>("program")?;
                if !(1..=128).contains(&program) {
                    return Err(invalid_data(
                        program_line,
                        "MIDI program number must be between 1 and 128".into(),
                    ));
                }
                parser.literal("end_midi_program_change")?;
                points.push(MidiProgramChangePoint { frame, program });
            }
            points.sort_by_key(|point| point.frame);
            points
        } else {
            Vec::new()
        };
        let volume_automation = if version >= AUTOMATION_PROJECT_FILE_VERSION {
            let (_, point_count) = parser.value_field::<usize>("volume_automation_points")?;
            let mut points = Vec::with_capacity(point_count);
            for _ in 0..point_count {
                parser.literal("automation_point")?;
                let (_, frame) = parser.value_field::<u64>("frame")?;
                let (value_line, value) = parser.value_field::<f32>("value")?;
                require_finite(value, value_line, "value")?;
                if value < 0.0 {
                    return Err(invalid_data(
                        value_line,
                        "automation `value` must be non-negative".into(),
                    ));
                }
                parser.literal("end_automation_point")?;
                points.push(AutomationPoint { frame, value });
            }
            points.sort_by_key(|point| point.frame);
            points
        } else {
            Vec::new()
        };
        let (_, clip_count) = parser.value_field::<usize>("clips")?;

        let mut track = Track {
            name: track_name,
            gain,
            pan,
            muted,
            soloed,
            recording: TrackRecording {
                armed: record_armed,
                input_monitoring,
            },
            input,
            output,
            sends,
            inserts,
            instrument,
            soundfont,
            midi_channel,
            midi_cc,
            midi_pitch_bend,
            midi_channel_pressure,
            midi_poly_pressure,
            midi_program_changes,
            volume_automation,
            take_lanes: Vec::new(),
            clips: Vec::new(),
        };

        for _ in 0..clip_count {
            parser.literal("clip")?;
            let (_, clip_name) = parser.string_field("name")?;
            let (_, start_frame) = parser.value_field::<u64>("start_frame")?;
            let (_, length_frames) = parser.value_field::<u64>("length_frames")?;
            let (gain, fade_in_frames, fade_out_frames, fade_curve) =
                if version >= CLIP_MIX_PROJECT_FILE_VERSION {
                    let (gain_line, gain) = parser.value_field::<f32>("clip_gain")?;
                    require_finite(gain, gain_line, "clip_gain")?;
                    if gain < 0.0 {
                        return Err(invalid_data(
                            gain_line,
                            "`clip_gain` must be non-negative".into(),
                        ));
                    }
                    let fade_in_frames = parser.value_field::<u64>("fade_in_frames")?.1;
                    let fade_out_frames = parser.value_field::<u64>("fade_out_frames")?.1;
                    let fade_curve = if version >= FADE_CURVE_PROJECT_FILE_VERSION {
                        let (curve_line, curve_name) = parser.field("fade_curve")?;
                        parse_fade_curve(curve_line, curve_name)?
                    } else {
                        FadeCurve::Linear
                    };
                    (gain, fade_in_frames, fade_out_frames, fade_curve)
                } else {
                    (1.0, 0, 0, FadeCurve::Linear)
                };
            let (source_line, source_name) = parser.field("source")?;
            let source = match (version, source_name) {
                (version, "midi") if version >= MIDI_PROJECT_FILE_VERSION => {
                    let (amplitude_line, amplitude) = parser.value_field::<f32>("amplitude")?;
                    require_finite(amplitude, amplitude_line, "amplitude")?;
                    let (_, note_count) = parser.value_field::<usize>("notes")?;
                    let mut notes = Vec::with_capacity(note_count);
                    for _ in 0..note_count {
                        parser.literal("note")?;
                        let (_, start_frame) = parser.value_field::<u64>("start_frame")?;
                        let (_, length_frames) = parser.value_field::<u64>("length_frames")?;
                        let (_, midi_note) = parser.value_field::<u8>("midi_note")?;
                        let (velocity_line, velocity) = if version >= VELOCITY_PROJECT_FILE_VERSION
                        {
                            parser.value_field::<u8>("velocity")?
                        } else {
                            (source_line, 127)
                        };
                        if !(1..=127).contains(&velocity) {
                            return Err(invalid_data(
                                velocity_line,
                                "`velocity` must be between 1 and 127".into(),
                            ));
                        }
                        parser.literal("end_note")?;
                        notes.push(MidiNote {
                            start_frame,
                            length_frames,
                            midi_note,
                            velocity,
                        });
                    }
                    ClipSource::Midi { notes, amplitude }
                }
                (_, "sine") => {
                    let (frequency_line, frequency_hz) =
                        parser.value_field::<f32>("frequency_hz")?;
                    require_finite(frequency_hz, frequency_line, "frequency_hz")?;
                    let (amplitude_line, amplitude) = parser.value_field::<f32>("amplitude")?;
                    require_finite(amplitude, amplitude_line, "amplitude")?;
                    ClipSource::Sine {
                        frequency_hz,
                        amplitude,
                    }
                }
                (version, "audio_file") if version >= AUDIO_PROJECT_FILE_VERSION => {
                    let (_, path) = parser.string_field("path")?;
                    let (_, source_offset_frames) =
                        parser.value_field::<u64>("source_offset_frames")?;
                    let (_, source_sample_rate) =
                        parser.value_field::<u32>("source_sample_rate")?;
                    let (_, channels) = parser.value_field::<u16>("channels")?;
                    let reversed = if version >= AUDIO_REVERSE_PROJECT_FILE_VERSION {
                        let (_, reversed) = parser.value_field::<bool>("reversed")?;
                        reversed
                    } else {
                        false
                    };
                    ClipSource::AudioFile {
                        path,
                        source_offset_frames,
                        source_sample_rate,
                        channels,
                        reversed,
                    }
                }
                (_, other) => {
                    return Err(invalid_data(
                        source_line,
                        format!("unknown clip source `{other}` for DMO project version {version}"),
                    ));
                }
            };
            parser.literal("end_clip")?;

            track.clips.push(Clip {
                name: clip_name,
                start_frame,
                length_frames,
                gain,
                fade_in_frames,
                fade_out_frames,
                fade_curve,
                source,
            });
        }
        if version >= TAKE_LANE_PROJECT_FILE_VERSION {
            let (_, take_count) = parser.value_field::<usize>("take_lanes")?;
            track.take_lanes.reserve(take_count);
            for _ in 0..take_count {
                parser.literal("take_lane")?;
                let (_, name) = parser.string_field("name")?;
                let (_, start_frame) = parser.value_field::<u64>("start_frame")?;
                let (_, length_frames) = parser.value_field::<u64>("length_frames")?;
                let (gain_line, gain) = parser.value_field::<f32>("clip_gain")?;
                require_finite(gain, gain_line, "clip_gain")?;
                if gain < 0.0 {
                    return Err(invalid_data(
                        gain_line,
                        "`clip_gain` must be non-negative".into(),
                    ));
                }
                let (_, fade_in_frames) = parser.value_field::<u64>("fade_in_frames")?;
                let (_, fade_out_frames) = parser.value_field::<u64>("fade_out_frames")?;
                let fade_curve = if version >= FADE_CURVE_PROJECT_FILE_VERSION {
                    let (curve_line, curve_name) = parser.field("fade_curve")?;
                    parse_fade_curve(curve_line, curve_name)?
                } else {
                    FadeCurve::Linear
                };
                let (_, path) = parser.string_field("path")?;
                let (_, source_offset_frames) =
                    parser.value_field::<u64>("source_offset_frames")?;
                let (_, source_sample_rate) = parser.value_field::<u32>("source_sample_rate")?;
                let (_, channels) = parser.value_field::<u16>("channels")?;
                let reversed = if version >= AUDIO_REVERSE_PROJECT_FILE_VERSION {
                    let (_, reversed) = parser.value_field::<bool>("reversed")?;
                    reversed
                } else {
                    false
                };
                parser.literal("end_take_lane")?;
                track.take_lanes.push(AudioTake {
                    name,
                    start_frame,
                    length_frames,
                    gain,
                    fade_in_frames,
                    fade_out_frames,
                    fade_curve,
                    path,
                    source_offset_frames,
                    source_sample_rate,
                    channels,
                    reversed,
                });
            }
        }
        parser.literal("end_track")?;
        project.tracks.push(track);
    }

    parser.literal("end_project")?;
    if let Some((line, content)) = parser.next_significant() {
        return Err(invalid_data(
            line,
            format!("unexpected content after `end_project`: `{content}`"),
        ));
    }

    Ok(project)
}

/// Saves a project to a UTF-8 `.dmo` file.
///
/// # Errors
///
/// Returns [`ProjectFileError`] when the project is invalid or the destination
/// cannot be written.
pub fn save_project(path: impl AsRef<Path>, project: &Project) -> Result<(), ProjectFileError> {
    fs::write(path, encode_project(project)?)?;
    Ok(())
}

/// Loads a project from a UTF-8 `.dmo` file.
///
/// # Errors
///
/// Returns [`ProjectFileError`] when the file cannot be read or decoded.
pub fn load_project(path: impl AsRef<Path>) -> Result<Project, ProjectFileError> {
    decode_project(&fs::read_to_string(path)?)
}

fn decode_insert_chain(
    parser: &mut Parser<'_>,
    count_field: &str,
) -> Result<Vec<ChannelInsert>, ProjectFileError> {
    let (_, count) = parser.value_field::<usize>(count_field)?;
    let mut inserts = Vec::with_capacity(count);
    for _ in 0..count {
        parser.literal("insert")?;
        let (_, enabled) = parser.value_field::<bool>("enabled")?;
        let (effect_line, effect_name) = parser.field("effect")?;
        let effect = match effect_name {
            "gain" => InsertEffect::Gain {
                gain_db: finite_field(parser, "gain_db")?,
            },
            "three_band_eq" => InsertEffect::ThreeBandEq {
                low_db: finite_field(parser, "low_db")?,
                mid_db: finite_field(parser, "mid_db")?,
                high_db: finite_field(parser, "high_db")?,
            },
            "compressor" => InsertEffect::Compressor {
                threshold_db: finite_field(parser, "threshold_db")?,
                ratio: finite_field(parser, "ratio")?,
                attack_ms: finite_field(parser, "attack_ms")?,
                release_ms: finite_field(parser, "release_ms")?,
                makeup_db: finite_field(parser, "makeup_db")?,
            },
            "saturation" => InsertEffect::Saturation {
                drive: finite_field(parser, "drive")?,
                mix: finite_field(parser, "mix")?,
            },
            other => {
                return Err(invalid_data(
                    effect_line,
                    format!("unknown insert effect `{other}`"),
                ));
            }
        };
        parser.literal("end_insert")?;
        inserts.push(ChannelInsert { enabled, effect });
    }
    Ok(inserts)
}

fn finite_field(parser: &mut Parser<'_>, field: &'static str) -> Result<f32, ProjectFileError> {
    let (line, value) = parser.value_field::<f32>(field)?;
    require_finite(value, line, field)?;
    Ok(value)
}

fn parse_fade_curve(line: usize, value: &str) -> Result<FadeCurve, ProjectFileError> {
    match value {
        "linear" => Ok(FadeCurve::Linear),
        "equal_power" => Ok(FadeCurve::EqualPower),
        "slow" => Ok(FadeCurve::Slow),
        "fast" => Ok(FadeCurve::Fast),
        other => Err(invalid_data(line, format!("unknown fade curve `{other}`"))),
    }
}

#[allow(clippy::too_many_lines)]
fn validate_project(project: &Project) -> Result<(), ProjectFileError> {
    Project::new(&project.name, project.sample_rate, project.tempo_bpm)
        .map_err(ProjectFileError::InvalidProject)?;
    if TimeSignature::new(
        project.time_signature.numerator,
        project.time_signature.denominator,
    )
    .is_none()
    {
        return Err(ProjectFileError::InvalidProject(
            ProjectError::InvalidTimeSignature {
                numerator: project.time_signature.numerator,
                denominator: project.time_signature.denominator,
            },
        ));
    }
    if let Some(range) = project.cycle_range
        && range.end_frame <= range.start_frame
    {
        return Err(ProjectFileError::InvalidField {
            field: "cycle_range",
            message: "end frame must be greater than start frame".into(),
        });
    }
    validate_non_negative_finite(project.master_gain, "master_gain")?;
    for insert in &project.master_inserts {
        validate_insert(insert)?;
    }
    for bus in &project.buses {
        validate_non_negative_finite(bus.gain, "bus_gain")?;
        validate_finite_field(bus.pan, "bus_pan")?;
        for insert in &bus.inserts {
            validate_insert(insert)?;
        }
    }

    for track in &project.tracks {
        validate_finite_field(track.gain, "gain")?;
        validate_finite_field(track.pan, "pan")?;
        if !(1..=16).contains(&track.midi_channel) {
            return Err(ProjectFileError::InvalidField {
                field: "midi_channel",
                message: "value must be between 1 and 16".into(),
            });
        }
        if let Some(soundfont) = &track.soundfont {
            if soundfont.path.trim().is_empty() || soundfont.name.trim().is_empty() {
                return Err(ProjectFileError::InvalidField {
                    field: "soundfont",
                    message: "path and preset name must not be empty".into(),
                });
            }
            if soundfont.program > 127 {
                return Err(ProjectFileError::InvalidField {
                    field: "soundfont_program",
                    message: "value must be between 0 and 127".into(),
                });
            }
        }
        if let TrackInput::MidiChannel(channel) = track.input
            && !(1..=16).contains(&channel)
        {
            return Err(ProjectFileError::InvalidField {
                field: "record_input_channel",
                message: "value must be between 1 and 16".into(),
            });
        }
        if let ChannelOutput::Bus(bus_index) = track.output
            && bus_index >= project.buses.len()
        {
            return Err(ProjectFileError::InvalidField {
                field: "output_bus_index",
                message: format!(
                    "bus {bus_index} does not exist ({} buses)",
                    project.buses.len()
                ),
            });
        }
        for send in &track.sends {
            if send.bus_index >= project.buses.len() {
                return Err(ProjectFileError::InvalidField {
                    field: "send_bus_index",
                    message: format!(
                        "bus {} does not exist ({} buses)",
                        send.bus_index,
                        project.buses.len()
                    ),
                });
            }
            validate_non_negative_finite(send.gain, "send_gain")?;
        }
        for insert in &track.inserts {
            validate_insert(insert)?;
        }
        if track
            .midi_cc
            .iter()
            .any(|point| point.controller > 127 || point.value > 127)
        {
            return Err(ProjectFileError::InvalidField {
                field: "midi_cc",
                message: "controller and value must be between 0 and 127".into(),
            });
        }
        if track
            .midi_pitch_bend
            .iter()
            .any(|point| !(-8192..=8191).contains(&point.value))
        {
            return Err(ProjectFileError::InvalidField {
                field: "midi_pitch_bend",
                message: "value must be between -8192 and 8191".into(),
            });
        }
        if track
            .midi_channel_pressure
            .iter()
            .any(|point| point.value > 127)
        {
            return Err(ProjectFileError::InvalidField {
                field: "midi_channel_pressure",
                message: "value must be between 0 and 127".into(),
            });
        }
        if track
            .midi_poly_pressure
            .iter()
            .any(|point| point.note > 127 || point.value > 127)
        {
            return Err(ProjectFileError::InvalidField {
                field: "midi_poly_pressure",
                message: "note and value must be between 0 and 127".into(),
            });
        }
        if track
            .midi_program_changes
            .iter()
            .any(|point| !(1..=128).contains(&point.program))
        {
            return Err(ProjectFileError::InvalidField {
                field: "midi_program_changes",
                message: "program must be between 1 and 128".into(),
            });
        }
        for point in &track.volume_automation {
            validate_non_negative_finite(point.value, "automation_value")?;
        }
        for clip in &track.clips {
            validate_finite_field(clip.gain, "clip_gain")?;
            if clip.gain < 0.0 {
                return Err(ProjectFileError::InvalidField {
                    field: "clip_gain",
                    message: "value must be non-negative".into(),
                });
            }
            match &clip.source {
                ClipSource::Midi { notes, amplitude } => {
                    validate_finite_field(*amplitude, "amplitude")?;
                    if notes.iter().any(|note| !(1..=127).contains(&note.velocity)) {
                        return Err(ProjectFileError::InvalidField {
                            field: "velocity",
                            message: "value must be between 1 and 127".into(),
                        });
                    }
                }
                ClipSource::Sine {
                    frequency_hz,
                    amplitude,
                } => {
                    validate_finite_field(*frequency_hz, "frequency_hz")?;
                    validate_finite_field(*amplitude, "amplitude")?;
                }
                ClipSource::AudioFile { .. } => {}
            }
        }
        for take in &track.take_lanes {
            validate_non_negative_finite(take.gain, "clip_gain")?;
            if take.source_sample_rate == 0 {
                return Err(ProjectFileError::InvalidField {
                    field: "source_sample_rate",
                    message: "value must be greater than zero".into(),
                });
            }
            if take.channels == 0 {
                return Err(ProjectFileError::InvalidField {
                    field: "channels",
                    message: "value must be greater than zero".into(),
                });
            }
        }
    }
    Ok(())
}

fn validate_insert(insert: &ChannelInsert) -> Result<(), ProjectFileError> {
    match insert.effect {
        InsertEffect::Gain { gain_db } => validate_finite_field(gain_db, "gain_db"),
        InsertEffect::ThreeBandEq {
            low_db,
            mid_db,
            high_db,
        } => {
            validate_finite_field(low_db, "low_db")?;
            validate_finite_field(mid_db, "mid_db")?;
            validate_finite_field(high_db, "high_db")
        }
        InsertEffect::Compressor {
            threshold_db,
            ratio,
            attack_ms,
            release_ms,
            makeup_db,
        } => {
            validate_finite_field(threshold_db, "threshold_db")?;
            validate_finite_field(makeup_db, "makeup_db")?;
            validate_non_negative_finite(ratio, "ratio")?;
            validate_non_negative_finite(attack_ms, "attack_ms")?;
            validate_non_negative_finite(release_ms, "release_ms")
        }
        InsertEffect::Saturation { drive, mix } => {
            validate_non_negative_finite(drive, "drive")?;
            validate_non_negative_finite(mix, "mix")?;
            if mix > 1.0 {
                return Err(ProjectFileError::InvalidField {
                    field: "mix",
                    message: "value must be between 0 and 1".into(),
                });
            }
            Ok(())
        }
    }
}

fn require_non_negative_finite(
    value: f32,
    line: usize,
    field: &'static str,
) -> Result<(), ProjectFileError> {
    require_finite(value, line, field)?;
    if value < 0.0 {
        Err(invalid_data(
            line,
            format!("`{field}` must be non-negative"),
        ))
    } else {
        Ok(())
    }
}

fn validate_non_negative_finite(value: f32, field: &'static str) -> Result<(), ProjectFileError> {
    validate_finite_field(value, field)?;
    if value < 0.0 {
        Err(ProjectFileError::InvalidField {
            field,
            message: "value must be non-negative".into(),
        })
    } else {
        Ok(())
    }
}

fn validate_finite_field(value: f32, field: &'static str) -> Result<(), ProjectFileError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ProjectFileError::InvalidField {
            field,
            message: "value must be finite".to_owned(),
        })
    }
}

fn require_finite(value: f32, line: usize, field: &'static str) -> Result<(), ProjectFileError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid_data(
            line,
            format!("`{field}` must be a finite number"),
        ))
    }
}

fn push_value(output: &mut String, key: &str, value: &impl ToString) {
    output.push_str(key);
    output.push(' ');
    output.push_str(&value.to_string());
    output.push('\n');
}

fn push_string(output: &mut String, key: &str, value: &str) {
    output.push_str(key);
    output.push(' ');
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control.is_control() => output.extend(control.escape_unicode()),
            other => output.push(other),
        }
    }
    output.push_str("\"\n");
}

fn parse_value<T>(line: usize, field: &str, value: &str) -> Result<T, ProjectFileError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    value.parse().map_err(|error| {
        invalid_data(
            line,
            format!("invalid value for `{field}` (`{value}`): {error}"),
        )
    })
}

fn parse_quoted(line: usize, field: &str, value: &str) -> Result<String, ProjectFileError> {
    let mut characters = value.chars();
    if characters.next() != Some('"') {
        return Err(invalid_data(
            line,
            format!("`{field}` must be a quoted string"),
        ));
    }

    let mut decoded = String::new();
    loop {
        let Some(character) = characters.next() else {
            return Err(invalid_data(
                line,
                format!("unterminated quoted string for `{field}`"),
            ));
        };

        match character {
            '"' => {
                if characters.next().is_some() {
                    return Err(invalid_data(
                        line,
                        format!("unexpected text after quoted `{field}` value"),
                    ));
                }
                return Ok(decoded);
            }
            '\\' => decode_escape(&mut characters, &mut decoded, line, field)?,
            control if control.is_control() => {
                return Err(invalid_data(
                    line,
                    format!("unescaped control character in `{field}`"),
                ));
            }
            other => decoded.push(other),
        }
    }
}

fn decode_escape(
    characters: &mut impl Iterator<Item = char>,
    output: &mut String,
    line: usize,
    field: &str,
) -> Result<(), ProjectFileError> {
    let Some(escape) = characters.next() else {
        return Err(invalid_data(
            line,
            format!("incomplete escape sequence in `{field}`"),
        ));
    };

    match escape {
        '"' => output.push('"'),
        '\\' => output.push('\\'),
        'n' => output.push('\n'),
        'r' => output.push('\r'),
        't' => output.push('\t'),
        'u' => decode_unicode_escape(characters, output, line, field)?,
        other => {
            return Err(invalid_data(
                line,
                format!("unknown escape sequence `\\{other}` in `{field}`"),
            ));
        }
    }
    Ok(())
}

fn decode_unicode_escape(
    characters: &mut impl Iterator<Item = char>,
    output: &mut String,
    line: usize,
    field: &str,
) -> Result<(), ProjectFileError> {
    if characters.next() != Some('{') {
        return Err(invalid_data(
            line,
            format!("expected `{{` after `\\u` in `{field}`"),
        ));
    }

    let mut digits = String::new();
    loop {
        let Some(character) = characters.next() else {
            return Err(invalid_data(
                line,
                format!("unterminated Unicode escape in `{field}`"),
            ));
        };
        if character == '}' {
            break;
        }
        if !character.is_ascii_hexdigit() || digits.len() == 6 {
            return Err(invalid_data(
                line,
                format!("invalid Unicode escape in `{field}`"),
            ));
        }
        digits.push(character);
    }

    if digits.is_empty() {
        return Err(invalid_data(
            line,
            format!("empty Unicode escape in `{field}`"),
        ));
    }
    let code_point = u32::from_str_radix(&digits, 16).map_err(|error| {
        invalid_data(
            line,
            format!("invalid Unicode escape in `{field}`: {error}"),
        )
    })?;
    let character = char::from_u32(code_point)
        .ok_or_else(|| invalid_data(line, format!("invalid Unicode scalar value in `{field}`")))?;
    output.push(character);
    Ok(())
}

fn invalid_data(line: usize, message: String) -> ProjectFileError {
    ProjectFileError::InvalidData { line, message }
}

struct Parser<'a> {
    lines: Lines<'a>,
    next_line: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            lines: input.lines(),
            next_line: 1,
        }
    }

    fn next_significant(&mut self) -> Option<(usize, &'a str)> {
        for original in self.lines.by_ref() {
            let line = self.next_line;
            self.next_line = self.next_line.saturating_add(1);
            let content = original.trim();
            if !content.is_empty() && !content.starts_with('#') {
                return Some((line, content));
            }
        }
        None
    }

    fn required(&mut self, expected: &str) -> Result<(usize, &'a str), ProjectFileError> {
        self.next_significant().ok_or_else(|| {
            invalid_data(
                self.next_line,
                format!("unexpected end of file; expected `{expected}`"),
            )
        })
    }

    fn literal(&mut self, expected: &str) -> Result<(), ProjectFileError> {
        let (line, content) = self.required(expected)?;
        if content == expected {
            Ok(())
        } else {
            Err(invalid_data(
                line,
                format!("expected `{expected}`, found `{content}`"),
            ))
        }
    }

    fn field(&mut self, expected: &str) -> Result<(usize, &'a str), ProjectFileError> {
        let (line, content) = self.required(expected)?;
        let Some(separator) = content.find(char::is_whitespace) else {
            return Err(invalid_data(
                line,
                format!("expected `{expected}` followed by a value"),
            ));
        };
        let key = &content[..separator];
        let value = content[separator..].trim();
        if key != expected {
            return Err(invalid_data(
                line,
                format!("expected field `{expected}`, found `{key}`"),
            ));
        }
        if value.is_empty() {
            return Err(invalid_data(
                line,
                format!("field `{expected}` has no value"),
            ));
        }
        Ok((line, value))
    }

    fn value_field<T>(&mut self, field: &str) -> Result<(usize, T), ProjectFileError>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        let (line, value) = self.field(field)?;
        Ok((line, parse_value(line, field, value)?))
    }

    fn string_field(&mut self, field: &str) -> Result<(usize, String), ProjectFileError> {
        let (line, value) = self.field(field)?;
        Ok((line, parse_quoted(line, field, value)?))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_FILE_ID: AtomicU64 = AtomicU64::new(0);

    #[allow(clippy::too_many_lines)]
    fn complete_project() -> Project {
        let mut project = Project::new("Song \"A\"\n\u{7}日本語", 96_000, 137.25).unwrap();
        project.time_signature = TimeSignature::new(7, 8).unwrap();
        project.cycle_range = CycleRange::new(12_000, 288_000);
        project.markers = vec![Marker::new("Intro", 0), Marker::new("Drop \"A\"", 96_000)];
        project.arranger_sections = vec![
            ArrangerSection {
                name: "Intro".into(),
                start_frame: 0,
                length_frames: 96_000,
                color_index: 1,
            },
            ArrangerSection {
                name: "Drop".into(),
                start_frame: 96_000,
                length_frames: 192_000,
                color_index: 3,
            },
        ];
        project.master_gain = 0.85;
        project
            .master_inserts
            .push(ChannelInsert::new(InsertEffect::Gain { gain_db: -1.5 }));
        let mut reverb_bus = Bus::new("FX Reverb");
        reverb_bus.gain = 0.7;
        reverb_bus.pan = 0.1;
        reverb_bus
            .inserts
            .push(ChannelInsert::new(InsertEffect::Saturation {
                drive: 0.5,
                mix: 0.25,
            }));
        project.buses.push(reverb_bus);
        let mut lead = Track::new("Lead\\one");
        lead.gain = 0.75;
        lead.pan = -0.25;
        lead.muted = true;
        lead.sends.push(TrackSend {
            bus_index: 0,
            gain: 0.35,
            enabled: true,
            pre_fader: true,
        });
        lead.inserts
            .push(ChannelInsert::new(InsertEffect::ThreeBandEq {
                low_db: -2.0,
                mid_db: 1.5,
                high_db: 3.0,
            }));
        lead.clips.push(Clip {
            name: "Intro\tclip".into(),
            start_frame: 123,
            length_frames: u64::MAX,
            gain: 0.75,
            fade_in_frames: 32,
            fade_out_frames: 64,
            fade_curve: FadeCurve::EqualPower,
            source: ClipSource::Sine {
                frequency_hz: 440.5,
                amplitude: 0.625,
            },
        });
        project.tracks.push(lead);

        let mut bass = Track::new("Bass");
        bass.gain = 1.25;
        bass.pan = 0.5;
        bass.soloed = true;
        bass.recording.armed = true;
        bass.recording.input_monitoring = true;
        bass.input = TrackInput::MidiChannel(10);
        bass.instrument = Instrument::Saw;
        bass.soundfont = Some(SoundFontPreset::new(
            r"C:\SoundFonts\Studio.sf2",
            128,
            5,
            "Finger Bass",
        ));
        bass.midi_channel = 10;
        bass.midi_cc = vec![
            MidiControlPoint {
                frame: 10,
                controller: 1,
                value: 64,
            },
            MidiControlPoint {
                frame: 480,
                controller: 11,
                value: 100,
            },
        ];
        bass.midi_pitch_bend = vec![MidiPitchBendPoint {
            frame: 240,
            value: 4096,
        }];
        bass.midi_channel_pressure = vec![MidiChannelPressurePoint {
            frame: 360,
            value: 88,
        }];
        bass.midi_poly_pressure = vec![MidiPolyPressurePoint {
            frame: 420,
            note: 48,
            value: 72,
        }];
        bass.midi_program_changes = vec![MidiProgramChangePoint {
            frame: 120,
            program: 33,
        }];
        bass.output = ChannelOutput::Bus(0);
        bass.volume_automation = vec![
            AutomationPoint {
                frame: 0,
                value: 0.6,
            },
            AutomationPoint {
                frame: 960,
                value: 1.2,
            },
        ];
        bass.take_lanes.push(AudioTake {
            name: "Bass Take 1".into(),
            start_frame: 10,
            length_frames: 960,
            gain: 0.9,
            fade_in_frames: 12,
            fade_out_frames: 24,
            fade_curve: FadeCurve::Fast,
            path: r"C:\Recordings\bass_take_1.wav".into(),
            source_offset_frames: 64,
            source_sample_rate: 48_000,
            channels: 2,
            reversed: true,
        });
        bass.clips.push(Clip {
            name: "MIDI Part".into(),
            start_frame: 10,
            length_frames: 960,
            gain: 1.25,
            fade_in_frames: 48,
            fade_out_frames: 96,
            fade_curve: FadeCurve::Slow,
            source: ClipSource::Midi {
                notes: vec![
                    MidiNote {
                        start_frame: 0,
                        length_frames: 240,
                        midi_note: 48,
                        velocity: 96,
                    },
                    MidiNote {
                        start_frame: 480,
                        length_frames: 480,
                        midi_note: 55,
                        velocity: 112,
                    },
                ],
                amplitude: 0.8,
            },
        });
        project.tracks.push(bass);
        project
    }

    #[test]
    fn every_project_field_round_trips() {
        let project = complete_project();
        let encoded = encode_project(&project).unwrap();

        assert!(encoded.starts_with("DMO_PROJECT 21\n"));
        assert!(encoded.contains("time_signature_numerator 7\n"));
        assert!(encoded.contains("time_signature_denominator 8\n"));
        assert!(encoded.contains("cycle_enabled true\n"));
        assert!(encoded.contains("cycle_start_frame 12000\n"));
        assert!(encoded.contains("cycle_end_frame 288000\n"));
        assert!(encoded.contains("markers 2\n"));
        assert!(encoded.contains("name \"Drop \\\"A\\\"\"\n"));
        assert!(encoded.contains("arranger_sections 2\n"));
        assert!(encoded.contains("color_index 3\n"));
        assert!(encoded.contains("master_gain 0.85\n"));
        assert!(encoded.contains("soloed true\n"));
        assert!(encoded.contains("record_armed true\n"));
        assert!(encoded.contains("input_monitoring true\n"));
        assert!(encoded.contains("record_input midi_channel\n"));
        assert!(encoded.contains("record_input_channel 10\n"));
        assert!(encoded.contains("instrument saw\n"));
        assert!(encoded.contains("soundfont_enabled true\n"));
        assert!(encoded.contains(r#"soundfont_path "C:\\SoundFonts\\Studio.sf2""#));
        assert!(encoded.contains("midi_channel 10\n"));
        assert!(encoded.contains("midi_cc_points 2\n"));
        assert!(encoded.contains("controller 11\n"));
        assert!(encoded.contains("midi_pitch_bend_points 1\n"));
        assert!(encoded.contains("midi_channel_pressure_points 1\n"));
        assert!(encoded.contains("midi_poly_pressure_points 1\n"));
        assert!(encoded.contains("midi_program_changes 1\n"));
        assert!(encoded.contains("program 33\n"));
        assert!(encoded.contains("source midi\n"));
        assert!(encoded.contains("notes 2\n"));
        assert!(encoded.contains("velocity 96\n"));
        assert!(encoded.contains("velocity 112\n"));
        assert!(encoded.contains("clip_gain 0.75\n"));
        assert!(encoded.contains("fade_in_frames 32\n"));
        assert!(encoded.contains("fade_out_frames 64\n"));
        assert!(encoded.contains("fade_curve equal_power\n"));
        assert!(encoded.contains("fade_curve fast\n"));
        assert!(encoded.contains("fade_curve slow\n"));
        assert!(encoded.contains("volume_automation_points 2\n"));
        assert!(encoded.contains("frame 960\n"));
        assert!(encoded.contains("value 1.2\n"));
        assert!(encoded.contains("take_lanes 1\n"));
        assert!(encoded.contains("name \"Bass Take 1\"\n"));
        assert!(encoded.contains("master_inserts 1\n"));
        assert!(encoded.contains("buses 1\n"));
        assert!(encoded.contains("effect three_band_eq\n"));
        assert!(encoded.contains("pre_fader true\n"));
        assert!(encoded.contains("output_bus_index 0\n"));
        assert!(encoded.contains("reversed true\n"));
        assert_eq!(decode_project(&encoded).unwrap(), project);
    }

    #[test]
    fn mono_audio_input_routes_round_trip() {
        let mut project = Project::new("Input routing", 48_000, 120.0).unwrap();
        let mut left = Track::new("Input 1");
        left.input = TrackInput::AudioMonoLeft;
        let mut right = Track::new("Input 2");
        right.input = TrackInput::AudioMonoRight;
        project.tracks.extend([left, right]);

        let encoded = encode_project(&project).unwrap();
        assert!(encoded.contains("record_input audio_mono_left\n"));
        assert!(encoded.contains("record_input audio_mono_right\n"));
        assert_eq!(decode_project(&encoded).unwrap(), project);
    }

    #[test]
    fn version_twenty_projects_remain_compatible() {
        let project = complete_project();
        let version_twenty =
            encode_project(&project)
                .unwrap()
                .replacen("DMO_PROJECT 21", "DMO_PROJECT 20", 1);
        assert_eq!(decode_project(&version_twenty).unwrap(), project);
    }

    #[test]
    fn audio_file_source_and_windows_path_round_trip() {
        let path = r#"C:\Users\音楽\Drums "Live"\kick.wav"#;
        let mut project = Project::new("Imported audio", 48_000, 120.0).unwrap();
        let mut track = Track::new("Audio");
        track.clips.push(Clip {
            name: "Kick".into(),
            start_frame: 512,
            length_frames: 96_000,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::AudioFile {
                path: path.into(),
                source_offset_frames: 1_024,
                source_sample_rate: 44_100,
                channels: 2,
                reversed: false,
            },
        });
        project.tracks.push(track);

        let encoded = encode_project(&project).unwrap();

        assert!(encoded.contains("source audio_file\n"));
        assert!(encoded.contains(r#"path "C:\\Users\\音楽\\Drums \"Live\"\\kick.wav""#));
        assert!(encoded.contains("source_offset_frames 1024\n"));
        assert!(encoded.contains("source_sample_rate 44100\n"));
        assert!(encoded.contains("channels 2\n"));
        assert!(encoded.contains("reversed false\n"));
        assert_eq!(decode_project(&encoded).unwrap(), project);
    }

    #[test]
    fn version_one_sine_project_is_still_supported() {
        let legacy = concat!(
            "DMO_PROJECT 1\n",
            "name \"Legacy\"\n",
            "sample_rate 48000\n",
            "tempo_bpm 120\n",
            "tracks 1\n",
            "track\n",
            "name \"Synth\"\n",
            "gain 0.75\n",
            "pan -0.25\n",
            "muted false\n",
            "clips 1\n",
            "clip\n",
            "name \"Tone\"\n",
            "start_frame 64\n",
            "length_frames 128\n",
            "source sine\n",
            "frequency_hz 220\n",
            "amplitude 0.5\n",
            "end_clip\n",
            "end_track\n",
            "end_project\n",
        );

        let project = decode_project(legacy).unwrap();

        assert_eq!(project.name, "Legacy");
        assert_eq!(project.tracks[0].clips[0].name, "Tone");
        assert!(matches!(
            &project.tracks[0].clips[0].source,
            ClipSource::Sine {
                frequency_hz: 220.0,
                amplitude: 0.5
            }
        ));
        assert!(
            encode_project(&project)
                .unwrap()
                .starts_with("DMO_PROJECT 21\n")
        );
    }

    fn without_time_signature_fields(encoded: &str, target_version: u32) -> String {
        encoded
            .replacen(
                "DMO_PROJECT 21",
                &format!("DMO_PROJECT {target_version}"),
                1,
            )
            .lines()
            .filter(|line| {
                !line.starts_with("time_signature_numerator ")
                    && !line.starts_with("time_signature_denominator ")
                    && !line.starts_with("soundfont_enabled ")
                    && !line.starts_with("soundfont_path ")
                    && !line.starts_with("soundfont_bank ")
                    && !line.starts_with("soundfont_program ")
                    && !line.starts_with("soundfont_name ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn without_reverse_fields(encoded: &str, target_version: u32) -> String {
        without_time_signature_fields(encoded, target_version)
            .lines()
            .filter(|line| !line.starts_with("reversed "))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn without_program_change_fields(encoded: &str, target_version: u32) -> String {
        let mut inside_program = false;
        without_reverse_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                if *line == "midi_program_change" {
                    inside_program = true;
                    return false;
                }
                if *line == "end_midi_program_change" {
                    inside_program = false;
                    return false;
                }
                !inside_program && !line.starts_with("midi_program_changes ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn without_fade_curve_fields(encoded: &str, target_version: u32) -> String {
        without_program_change_fields(encoded, target_version)
            .lines()
            .filter(|line| !line.starts_with("fade_curve "))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn without_cycle_fields(encoded: &str, target_version: u32) -> String {
        without_fade_curve_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                !line.starts_with("cycle_enabled ")
                    && !line.starts_with("cycle_start_frame ")
                    && !line.starts_with("cycle_end_frame ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn without_arranger_fields(encoded: &str, target_version: u32) -> String {
        let mut inside_section = false;
        without_cycle_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                if *line == "arranger_section" {
                    inside_section = true;
                    return false;
                }
                if *line == "end_arranger_section" {
                    inside_section = false;
                    return false;
                }
                !inside_section && !line.starts_with("arranger_sections ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn without_marker_fields(encoded: &str, target_version: u32) -> String {
        let mut inside_marker = false;
        without_arranger_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                if *line == "marker" {
                    inside_marker = true;
                    return false;
                }
                if *line == "end_marker" {
                    inside_marker = false;
                    return false;
                }
                !inside_marker && !line.starts_with("markers ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn without_record_input_fields(encoded: &str, target_version: u32) -> String {
        let mut inside_pitch = false;
        let mut inside_channel_pressure = false;
        let mut inside_poly_pressure = false;
        without_marker_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                if *line == "midi_pitch_bend" {
                    inside_pitch = true;
                    return false;
                }
                if *line == "end_midi_pitch_bend" {
                    inside_pitch = false;
                    return false;
                }
                if *line == "midi_channel_pressure" {
                    inside_channel_pressure = true;
                    return false;
                }
                if *line == "end_midi_channel_pressure" {
                    inside_channel_pressure = false;
                    return false;
                }
                if *line == "midi_poly_pressure" {
                    inside_poly_pressure = true;
                    return false;
                }
                if *line == "end_midi_poly_pressure" {
                    inside_poly_pressure = false;
                    return false;
                }
                !inside_pitch
                    && !inside_channel_pressure
                    && !inside_poly_pressure
                    && !line.starts_with("record_input ")
                    && !line.starts_with("record_input_channel ")
                    && !line.starts_with("midi_pitch_bend_points ")
                    && !line.starts_with("midi_channel_pressure_points ")
                    && !line.starts_with("midi_poly_pressure_points ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn without_mixer_fields(encoded: &str, target_version: u32) -> String {
        let mut inside_bus = false;
        let mut inside_insert = false;
        let mut inside_send = false;
        let mut inside_midi_cc = false;
        without_record_input_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                if *line == "bus" {
                    inside_bus = true;
                    return false;
                }
                if *line == "end_bus" {
                    inside_bus = false;
                    return false;
                }
                if inside_bus {
                    return false;
                }
                if *line == "insert" {
                    inside_insert = true;
                    return false;
                }
                if *line == "end_insert" {
                    inside_insert = false;
                    return false;
                }
                if inside_insert {
                    return false;
                }
                if *line == "send" {
                    inside_send = true;
                    return false;
                }
                if *line == "end_send" {
                    inside_send = false;
                    return false;
                }
                if inside_send {
                    return false;
                }
                if *line == "midi_cc" {
                    inside_midi_cc = true;
                    return false;
                }
                if *line == "end_midi_cc" {
                    inside_midi_cc = false;
                    return false;
                }
                if inside_midi_cc {
                    return false;
                }
                !line.starts_with("master_inserts ")
                    && !line.starts_with("buses ")
                    && !line.starts_with("output ")
                    && !line.starts_with("output_bus_index ")
                    && !line.starts_with("sends ")
                    && !line.starts_with("inserts ")
                    && !line.starts_with("midi_cc_points ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn version_thirteen_keeps_markers_and_defaults_arranger_empty() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_thirteen = without_arranger_fields(&encoded, 13);

        let decoded = decode_project(&version_thirteen).unwrap();

        assert_eq!(decoded.cycle_range, None);
        assert_eq!(decoded.markers.len(), 2);
        assert!(decoded.arranger_sections.is_empty());
    }

    #[test]
    fn version_fourteen_keeps_arranger_and_defaults_cycle_empty() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_fourteen = without_cycle_fields(&encoded, 14);

        let decoded = decode_project(&version_fourteen).unwrap();

        assert_eq!(decoded.cycle_range, None);
        assert_eq!(decoded.arranger_sections.len(), 2);
    }

    #[test]
    fn version_fifteen_keeps_cycle_and_defaults_fade_curves_linear() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_fifteen = without_fade_curve_fields(&encoded, 15);

        let decoded = decode_project(&version_fifteen).unwrap();

        assert_eq!(decoded.cycle_range, CycleRange::new(12_000, 288_000));
        assert!(
            decoded
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .all(|clip| clip.fade_curve == FadeCurve::Linear)
        );
        assert!(
            decoded
                .tracks
                .iter()
                .flat_map(|track| &track.take_lanes)
                .all(|take| take.fade_curve == FadeCurve::Linear)
        );
    }

    #[test]
    fn version_sixteen_defaults_program_changes_empty() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_sixteen = without_program_change_fields(&encoded, 16);

        let decoded = decode_project(&version_sixteen).unwrap();

        assert!(
            decoded
                .tracks
                .iter()
                .all(|track| track.midi_program_changes.is_empty())
        );
    }

    #[test]
    fn version_seventeen_defaults_audio_reverse_off() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_seventeen = without_reverse_fields(&encoded, 17);

        let decoded = decode_project(&version_seventeen).unwrap();

        assert!(
            decoded
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .all(|clip| !matches!(clip.source, ClipSource::AudioFile { reversed: true, .. }))
        );
        assert!(
            decoded.tracks[1]
                .take_lanes
                .iter()
                .all(|take| !take.reversed)
        );
    }

    #[test]
    fn version_twelve_keeps_live_midi_and_defaults_markers_empty() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_twelve = without_marker_fields(&encoded, 12);

        let decoded = decode_project(&version_twelve).unwrap();

        assert!(decoded.markers.is_empty());
        assert_eq!(decoded.tracks[1].input, TrackInput::MidiChannel(10));
        assert_eq!(decoded.tracks[1].midi_pitch_bend.len(), 1);
        assert_eq!(decoded.tracks[1].midi_channel_pressure.len(), 1);
        assert_eq!(decoded.tracks[1].midi_poly_pressure.len(), 1);
    }

    #[test]
    fn version_eleven_keeps_mixer_and_defaults_recording_input_to_audio() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_eleven = without_record_input_fields(&encoded, 11);

        let decoded = decode_project(&version_eleven).unwrap();

        assert_eq!(decoded.buses.len(), 1);
        assert_eq!(decoded.tracks[1].output, ChannelOutput::Bus(0));
        assert!(
            decoded
                .tracks
                .iter()
                .all(|track| track.input == TrackInput::Audio)
        );
    }

    fn without_take_lane_fields(encoded: &str, target_version: u32) -> String {
        let mut inside_take = false;
        without_mixer_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                if *line == "take_lane" {
                    inside_take = true;
                    return false;
                }
                if *line == "end_take_lane" {
                    inside_take = false;
                    return false;
                }
                !inside_take && !line.starts_with("take_lanes ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn version_ten_keeps_take_lanes_and_defaults_mixer_routing() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_ten = without_mixer_fields(&encoded, 10);

        let decoded = decode_project(&version_ten).unwrap();

        assert_eq!(decoded.tracks[1].take_lanes.len(), 1);
        assert!(decoded.buses.is_empty());
        assert!(decoded.master_inserts.is_empty());
        assert!(decoded.tracks.iter().all(|track| {
            track.output == ChannelOutput::Master
                && track.sends.is_empty()
                && track.inserts.is_empty()
        }));
    }

    fn without_record_fields(encoded: &str, target_version: u32) -> String {
        without_take_lane_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                !line.starts_with("record_armed ") && !line.starts_with("input_monitoring ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn version_nine_keeps_recording_state_and_has_no_take_lanes() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_nine = without_take_lane_fields(&encoded, 9);

        let decoded = decode_project(&version_nine).unwrap();

        assert!(decoded.tracks[1].recording.armed);
        assert!(decoded.tracks[1].recording.input_monitoring);
        assert!(
            decoded
                .tracks
                .iter()
                .all(|track| track.take_lanes.is_empty())
        );
    }

    fn without_automation_fields(encoded: &str, target_version: u32) -> String {
        without_record_fields(encoded, target_version)
            .lines()
            .filter(|line| {
                !line.starts_with("master_gain ")
                    && !line.starts_with("volume_automation_points ")
                    && *line != "automation_point"
                    && *line != "end_automation_point"
                    && !line.starts_with("frame ")
                    && !line.starts_with("value ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn version_eight_tracks_are_disarmed_with_monitoring_off() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_eight = without_record_fields(&encoded, 8);

        let decoded = decode_project(&version_eight).unwrap();

        assert_eq!(decoded.master_gain.to_bits(), 0.85_f32.to_bits());
        assert_eq!(decoded.tracks[1].volume_automation.len(), 2);
        assert!(decoded.tracks.iter().all(|track| !track.recording.armed));
        assert!(
            decoded
                .tracks
                .iter()
                .all(|track| !track.recording.input_monitoring)
        );
    }

    #[test]
    fn version_seven_gets_unity_master_and_no_automation() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_seven = without_automation_fields(&encoded, 7);

        let decoded = decode_project(&version_seven).unwrap();

        assert_eq!(decoded.master_gain.to_bits(), 1.0_f32.to_bits());
        assert!(
            decoded
                .tracks
                .iter()
                .all(|track| track.volume_automation.is_empty())
        );
        assert_eq!(
            decoded.tracks[0].clips[0].gain.to_bits(),
            0.75_f32.to_bits()
        );
        assert_eq!(decoded.tracks[0].clips[0].fade_in_frames, 32);
    }

    #[test]
    fn version_six_clips_get_neutral_gain_and_fades() {
        let encoded = encode_project(&complete_project()).unwrap();
        let version_six = without_automation_fields(&encoded, 6)
            .lines()
            .filter(|line| {
                !line.starts_with("clip_gain ")
                    && !line.starts_with("fade_in_frames ")
                    && !line.starts_with("fade_out_frames ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let decoded = decode_project(&version_six).unwrap();

        assert!(
            decoded
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .all(|clip| {
                    clip.gain.to_bits() == 1.0_f32.to_bits()
                        && clip.fade_in_frames == 0
                        && clip.fade_out_frames == 0
                })
        );
        assert!(matches!(
            &decoded.tracks[1].clips[0].source,
            ClipSource::Midi { notes, .. } if notes[0].velocity == 96
        ));
    }

    #[test]
    fn version_three_midi_project_gets_default_instrument_routing() {
        let version_three = concat!(
            "DMO_PROJECT 3\n",
            "name \"MIDI v3\"\n",
            "sample_rate 48000\n",
            "tempo_bpm 120\n",
            "tracks 1\n",
            "track\n",
            "name \"Lead\"\n",
            "gain 1\n",
            "pan 0\n",
            "muted false\n",
            "clips 1\n",
            "clip\n",
            "name \"Part\"\n",
            "start_frame 0\n",
            "length_frames 24000\n",
            "source midi\n",
            "amplitude 0.8\n",
            "notes 1\n",
            "note\n",
            "start_frame 0\n",
            "length_frames 24000\n",
            "midi_note 60\n",
            "end_note\n",
            "end_clip\n",
            "end_track\n",
            "end_project\n",
        );

        let project = decode_project(version_three).unwrap();

        assert_eq!(project.tracks[0].instrument, Instrument::Sine);
        assert_eq!(project.tracks[0].midi_channel, 1);
        assert!(matches!(
            &project.tracks[0].clips[0].source,
            ClipSource::Midi { notes, .. } if notes.len() == 1 && notes[0].velocity == 127
        ));
    }

    #[test]
    fn comments_and_blank_lines_are_accepted() {
        let encoded = encode_project(&Project::new("Empty", 48_000, 120.0).unwrap()).unwrap();
        let commented =
            encoded.replacen("name \"Empty\"", "# A project comment\n\nname \"Empty\"", 1);

        assert_eq!(decode_project(&commented).unwrap().name, "Empty");
    }

    #[test]
    fn unsupported_version_is_reported() {
        let error = decode_project("DMO_PROJECT 99\n").unwrap_err();

        assert!(matches!(error, ProjectFileError::UnsupportedVersion(99)));
        assert_eq!(error.to_string(), "unsupported DMO project version: 99");
    }

    #[test]
    fn unknown_clip_source_is_reported_clearly() {
        let unknown_source = concat!(
            "DMO_PROJECT 2\n",
            "name \"Unknown\"\n",
            "sample_rate 48000\n",
            "tempo_bpm 120\n",
            "tracks 1\n",
            "track\n",
            "name \"Track\"\n",
            "gain 1\n",
            "pan 0\n",
            "muted false\n",
            "clips 1\n",
            "clip\n",
            "name \"Clip\"\n",
            "start_frame 0\n",
            "length_frames 1\n",
            "source sampler\n",
        );

        let error = decode_project(unknown_source).unwrap_err();

        assert!(matches!(
            error,
            ProjectFileError::InvalidData { line: 16, .. }
        ));
        assert_eq!(
            error.to_string(),
            "invalid project file at line 16: unknown clip source `sampler` for DMO project version 2"
        );
    }

    #[test]
    fn malformed_input_returns_line_numbered_errors() {
        let malformed = concat!(
            "DMO_PROJECT 1\n",
            "name \"Broken\"\n",
            "sample_rate 48000\n",
            "tempo_bpm fast\n",
        );
        let error = decode_project(malformed).unwrap_err();

        assert!(matches!(
            error,
            ProjectFileError::InvalidData { line: 4, .. }
        ));
        assert!(error.to_string().contains("tempo_bpm"));
    }

    #[test]
    fn invalid_escapes_and_non_finite_values_are_rejected() {
        let invalid_escape = concat!(
            "DMO_PROJECT 1\n",
            "name \"bad\\q\"\n",
            "sample_rate 48000\n",
            "tempo_bpm 120\n",
            "tracks 0\n",
            "end_project\n",
        );
        assert!(decode_project(invalid_escape).is_err());

        let mut project = complete_project();
        project.tracks[0].gain = f32::NAN;
        assert!(matches!(
            encode_project(&project),
            Err(ProjectFileError::InvalidField { field: "gain", .. })
        ));
    }

    #[test]
    fn save_and_load_use_the_same_format() {
        let id = NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("dmo-core-project-{}-{id}.dmo", std::process::id()));
        let project = complete_project();

        save_project(&path, &project).unwrap();
        let loaded = load_project(&path).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(loaded, project);
    }
}
