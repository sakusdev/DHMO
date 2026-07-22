use std::{
    collections::{HashMap, HashSet},
    f32::consts::{FRAC_PI_4, PI, TAU},
    fmt,
};

use crate::{
    AudioFileError, AutomationPoint, ChannelInsert, ChannelOutput, ClipSource, DecodedAudio,
    InsertEffect, Instrument, MidiChannelPressurePoint, MidiControlPoint, MidiPitchBendPoint,
    MidiPolyPressurePoint, Project, Track, automation_value_at, decode_wav, midi_note_frequency,
};

#[derive(Clone, Copy)]
struct TrackMix<'a> {
    left_pan: f32,
    right_pan: f32,
    base_gain: f32,
    automation: &'a [AutomationPoint],
}

#[derive(Clone, Copy)]
struct MidiExpression<'a> {
    note: Option<u8>,
    pitch_bend: &'a [MidiPitchBendPoint],
    channel_pressure: &'a [MidiChannelPressurePoint],
    poly_pressure: &'a [MidiPolyPressurePoint],
}

impl MidiExpression<'static> {
    const NONE: Self = Self {
        note: None,
        pitch_bend: &[],
        channel_pressure: &[],
        poly_pressure: &[],
    };
}

impl TrackMix<'_> {
    fn gains_at(self, frame: usize) -> (f32, f32) {
        let frame = u64::try_from(frame).unwrap_or(u64::MAX);
        let gain = self.base_gain * automation_value_at(self.automation, frame).max(0.0);
        (self.left_pan * gain, self.right_pan * gain)
    }
}

/// An error produced while rendering a project with file-backed clips.
#[derive(Debug)]
pub enum RenderError {
    InvalidProjectSampleRate(u32),
    ProjectTooLong(u64),
    AudioFile {
        path: String,
        source: AudioFileError,
    },
    InvalidBusReference {
        track_name: String,
        bus_index: usize,
        bus_count: usize,
    },
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProjectSampleRate(rate) => {
                write!(formatter, "invalid project sample rate: {rate}")
            }
            Self::ProjectTooLong(frames) => {
                write!(formatter, "project is too long to render: {frames} frames")
            }
            Self::AudioFile { path, source } => {
                write!(formatter, "failed to render audio file `{path}`: {source}")
            }
            Self::InvalidBusReference {
                track_name,
                bus_index,
                bus_count,
            } => write!(
                formatter,
                "track `{track_name}` references bus {bus_index}, but the project has {bus_count} buses"
            ),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AudioFile { source, .. } => Some(source),
            Self::InvalidProjectSampleRate(_)
            | Self::ProjectTooLong(_)
            | Self::InvalidBusReference { .. } => None,
        }
    }
}

/// Renders the project to interleaved stereo samples in the range -1.0..=1.0.
///
/// This compatibility API skips unreadable file-backed clips. Call
/// [`try_render_stereo`] when missing or corrupt audio must be reported.
#[must_use]
pub fn render_stereo(project: &Project) -> Vec<f32> {
    render_stereo_impl(project, false).unwrap_or_default()
}

/// Renders the project and reports missing, corrupt, or unsupported WAV files.
///
/// WAV I/O and resampling happen synchronously in this function, outside the
/// real-time playback callback. Each distinct source path is decoded once per
/// render.
///
/// # Errors
///
/// Returns [`RenderError`] when the output cannot be allocated, the project
/// sample rate is zero, or any file-backed clip cannot be decoded.
pub fn try_render_stereo(project: &Project) -> Result<Vec<f32>, RenderError> {
    render_stereo_impl(project, true)
}

#[allow(clippy::too_many_lines)]
fn render_stereo_impl(
    project: &Project,
    fail_on_audio_file_error: bool,
) -> Result<Vec<f32>, RenderError> {
    if project.sample_rate == 0 {
        return Err(RenderError::InvalidProjectSampleRate(project.sample_rate));
    }

    let duration_frames = project.duration_frames();
    let frame_count = usize::try_from(duration_frames)
        .map_err(|_| RenderError::ProjectTooLong(duration_frames))?;
    let sample_count = frame_count
        .checked_mul(2)
        .ok_or(RenderError::ProjectTooLong(duration_frames))?;
    let mut output = zeroed_buffer(sample_count, duration_frames)?;
    let mut bus_buffers = (0..project.buses.len())
        .map(|_| zeroed_buffer(sample_count, duration_frames))
        .collect::<Result<Vec<_>, _>>()?;

    let mut decoded_files = HashMap::<String, DecodedAudio>::new();
    let mut unreadable_files = HashSet::<String>::new();
    let any_soloed = project.tracks.iter().any(|track| track.soloed);

    for track in &project.tracks {
        if track.muted || (any_soloed && !track.soloed) {
            continue;
        }

        let mut track_output = zeroed_buffer(sample_count, duration_frames)?;
        let mut pitch_bend = track.midi_pitch_bend.clone();
        pitch_bend.sort_by_key(|point| point.frame);
        let mut channel_pressure = track.midi_channel_pressure.clone();
        channel_pressure.sort_by_key(|point| point.frame);
        let mut poly_pressure = track.midi_poly_pressure.clone();
        poly_pressure.sort_by_key(|point| (point.frame, point.note));
        let track_mix = TrackMix {
            left_pan: 1.0,
            right_pan: 1.0,
            base_gain: 1.0,
            automation: &[],
        };

        for clip in &track.clips {
            let start = usize::try_from(clip.start_frame).unwrap_or(usize::MAX);
            let length = usize::try_from(clip.length_frames).unwrap_or(usize::MAX);
            let clip_gain = clip.gain.max(0.0);
            let fade_in = usize::try_from(clip.fade_in_frames).unwrap_or(usize::MAX);
            let fade_out = usize::try_from(clip.fade_out_frames).unwrap_or(usize::MAX);
            match &clip.source {
                ClipSource::Midi { notes, amplitude } => {
                    for note in notes {
                        let note_start = start.saturating_add(
                            usize::try_from(note.start_frame).unwrap_or(usize::MAX),
                        );
                        let available = clip
                            .length_frames
                            .saturating_sub(note.start_frame)
                            .min(note.length_frames);
                        let note_length = usize::try_from(available).unwrap_or(usize::MAX);
                        render_tone_clip(
                            &mut track_output,
                            frame_count,
                            note_start,
                            note_length,
                            project.sample_rate,
                            midi_note_frequency(note.midi_note),
                            *amplitude * f32::from(note.velocity) / 127.0,
                            track.instrument,
                            true,
                            usize::try_from(note.start_frame).unwrap_or(usize::MAX),
                            length,
                            clip_gain,
                            fade_in,
                            fade_out,
                            track_mix,
                            MidiExpression {
                                note: Some(note.midi_note),
                                pitch_bend: &pitch_bend,
                                channel_pressure: &channel_pressure,
                                poly_pressure: &poly_pressure,
                            },
                        );
                    }
                }
                ClipSource::Sine {
                    frequency_hz,
                    amplitude,
                } => {
                    render_tone_clip(
                        &mut track_output,
                        frame_count,
                        start,
                        length,
                        project.sample_rate,
                        *frequency_hz,
                        *amplitude,
                        Instrument::Sine,
                        false,
                        0,
                        length,
                        clip_gain,
                        fade_in,
                        fade_out,
                        track_mix,
                        MidiExpression::NONE,
                    );
                }
                ClipSource::AudioFile {
                    path,
                    source_offset_frames,
                    ..
                } => {
                    if unreadable_files.contains(path) {
                        continue;
                    }
                    if !decoded_files.contains_key(path) {
                        match decode_wav(path) {
                            Ok(decoded) => {
                                decoded_files.insert(path.clone(), decoded);
                            }
                            Err(source) if fail_on_audio_file_error => {
                                return Err(RenderError::AudioFile {
                                    path: path.clone(),
                                    source,
                                });
                            }
                            Err(_) => {
                                unreadable_files.insert(path.clone());
                                continue;
                            }
                        }
                    }
                    if let Some(decoded) = decoded_files.get(path) {
                        render_audio_file_clip(
                            &mut track_output,
                            frame_count,
                            start,
                            length,
                            project.sample_rate,
                            *source_offset_frames,
                            decoded,
                            clip_gain,
                            fade_in,
                            fade_out,
                            track_mix,
                        );
                    }
                }
            }
        }

        apply_inserts(&mut track_output, project.sample_rate, &track.inserts);
        for send in track
            .sends
            .iter()
            .filter(|send| send.enabled && send.pre_fader)
        {
            let bus = bus_buffers
                .get_mut(send.bus_index)
                .ok_or_else(|| invalid_bus_reference(track, send.bus_index, project.buses.len()))?;
            mix_buffer_scaled(bus, &track_output, send.gain.max(0.0));
        }
        apply_channel_fader(
            &mut track_output,
            track.gain,
            track.pan,
            &track.volume_automation,
            &track.midi_cc,
        );
        for send in track
            .sends
            .iter()
            .filter(|send| send.enabled && !send.pre_fader)
        {
            let bus = bus_buffers
                .get_mut(send.bus_index)
                .ok_or_else(|| invalid_bus_reference(track, send.bus_index, project.buses.len()))?;
            mix_buffer_scaled(bus, &track_output, send.gain.max(0.0));
        }
        match track.output {
            ChannelOutput::Master => mix_buffer_scaled(&mut output, &track_output, 1.0),
            ChannelOutput::Bus(bus_index) => {
                let bus = bus_buffers
                    .get_mut(bus_index)
                    .ok_or_else(|| invalid_bus_reference(track, bus_index, project.buses.len()))?;
                mix_buffer_scaled(bus, &track_output, 1.0);
            }
        }
    }

    let any_bus_soloed = project.buses.iter().any(|bus| bus.soloed);
    for (bus, mut samples) in project.buses.iter().zip(bus_buffers) {
        if bus.muted || (any_bus_soloed && !bus.soloed) {
            continue;
        }
        apply_inserts(&mut samples, project.sample_rate, &bus.inserts);
        apply_stereo_bus_fader(&mut samples, bus.gain, bus.pan);
        mix_buffer_scaled(&mut output, &samples, 1.0);
    }

    apply_inserts(&mut output, project.sample_rate, &project.master_inserts);
    let master_gain = project.master_gain.max(0.0);
    for sample in &mut output {
        *sample = (*sample * master_gain).clamp(-1.0, 1.0);
    }
    Ok(output)
}

fn zeroed_buffer(sample_count: usize, duration_frames: u64) -> Result<Vec<f32>, RenderError> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(sample_count)
        .map_err(|_| RenderError::ProjectTooLong(duration_frames))?;
    buffer.resize(sample_count, 0.0);
    Ok(buffer)
}

fn invalid_bus_reference(track: &Track, bus_index: usize, bus_count: usize) -> RenderError {
    RenderError::InvalidBusReference {
        track_name: track.name.clone(),
        bus_index,
        bus_count,
    }
}

fn mix_buffer_scaled(destination: &mut [f32], source: &[f32], gain: f32) {
    for (destination, source) in destination.iter_mut().zip(source) {
        *destination += *source * gain;
    }
}

fn apply_channel_fader(
    samples: &mut [f32],
    gain: f32,
    pan: f32,
    automation: &[AutomationPoint],
    midi_cc: &[MidiControlPoint],
) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
    let left_pan = angle.cos();
    let right_pan = angle.sin();
    let mut volume_points = midi_cc
        .iter()
        .filter(|point| point.controller == 7)
        .collect::<Vec<_>>();
    let mut expression_points = midi_cc
        .iter()
        .filter(|point| point.controller == 11)
        .collect::<Vec<_>>();
    volume_points.sort_by_key(|point| point.frame);
    expression_points.sort_by_key(|point| point.frame);
    let mut volume_index = 0;
    let mut expression_index = 0;
    let mut cc_volume = 127_u8;
    let mut expression = 127_u8;

    for (frame, stereo) in samples.chunks_exact_mut(2).enumerate() {
        let frame = u64::try_from(frame).unwrap_or(u64::MAX);
        while volume_points
            .get(volume_index)
            .is_some_and(|point| point.frame <= frame)
        {
            cc_volume = volume_points[volume_index].value;
            volume_index += 1;
        }
        while expression_points
            .get(expression_index)
            .is_some_and(|point| point.frame <= frame)
        {
            expression = expression_points[expression_index].value;
            expression_index += 1;
        }
        let automated_gain = gain.max(0.0)
            * automation_value_at(automation, frame).max(0.0)
            * (f32::from(cc_volume) / 127.0)
            * (f32::from(expression) / 127.0);
        stereo[0] *= left_pan * automated_gain;
        stereo[1] *= right_pan * automated_gain;
    }
}

fn apply_stereo_bus_fader(samples: &mut [f32], gain: f32, pan: f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let (left, right) = if pan < 0.0 {
        (1.0, 1.0 + pan)
    } else {
        (1.0 - pan, 1.0)
    };
    let gain = gain.max(0.0);
    for stereo in samples.chunks_exact_mut(2) {
        stereo[0] *= left * gain;
        stereo[1] *= right * gain;
    }
}

fn apply_inserts(samples: &mut [f32], sample_rate: u32, inserts: &[ChannelInsert]) {
    for insert in inserts.iter().filter(|insert| insert.enabled) {
        match insert.effect {
            InsertEffect::Gain { gain_db } => {
                let gain = db_to_gain(gain_db.clamp(-60.0, 24.0));
                for sample in &mut *samples {
                    *sample *= gain;
                }
            }
            InsertEffect::ThreeBandEq {
                low_db,
                mid_db,
                high_db,
            } => apply_three_band_eq(samples, sample_rate, low_db, mid_db, high_db),
            InsertEffect::Compressor {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            } => apply_compressor(
                samples,
                sample_rate,
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            ),
            InsertEffect::Saturation { drive, mix } => {
                apply_saturation(samples, drive, mix);
            }
        }
    }
}

fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[allow(clippy::cast_precision_loss)]
fn apply_three_band_eq(
    samples: &mut [f32],
    sample_rate: u32,
    low_db: f32,
    mid_db: f32,
    high_db: f32,
) {
    if sample_rate == 0 {
        return;
    }
    let rate = sample_rate as f32;
    let low_alpha = 1.0 - (-TAU * 200.0 / rate).exp();
    let high_alpha = 1.0 - (-TAU * 4_000.0 / rate).exp();
    let gains = [
        db_to_gain(low_db.clamp(-24.0, 24.0)),
        db_to_gain(mid_db.clamp(-24.0, 24.0)),
        db_to_gain(high_db.clamp(-24.0, 24.0)),
    ];
    let mut low_state = [0.0_f32; 2];
    let mut high_lowpass = [0.0_f32; 2];
    for stereo in samples.chunks_exact_mut(2) {
        for channel in 0..2 {
            let input = stereo[channel];
            low_state[channel] += low_alpha * (input - low_state[channel]);
            high_lowpass[channel] += high_alpha * (input - high_lowpass[channel]);
            let low = low_state[channel];
            let high = input - high_lowpass[channel];
            let mid = input - low - high;
            stereo[channel] = low * gains[0] + mid * gains[1] + high * gains[2];
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::too_many_arguments)]
fn apply_compressor(
    samples: &mut [f32],
    sample_rate: u32,
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup_db: f32,
) {
    if sample_rate == 0 {
        return;
    }
    let threshold_db = threshold_db.clamp(-60.0, 0.0);
    let ratio = ratio.clamp(1.0, 20.0);
    let attack_seconds = (attack_ms.clamp(0.1, 500.0) / 1_000.0).max(f32::EPSILON);
    let release_seconds = (release_ms.clamp(5.0, 5_000.0) / 1_000.0).max(f32::EPSILON);
    let attack = (-1.0 / (sample_rate as f32 * attack_seconds)).exp();
    let release = (-1.0 / (sample_rate as f32 * release_seconds)).exp();
    let makeup = makeup_db.clamp(-24.0, 24.0);
    let mut envelope = 0.0_f32;
    for stereo in samples.chunks_exact_mut(2) {
        let detector = stereo[0].abs().max(stereo[1].abs());
        let coefficient = if detector > envelope { attack } else { release };
        envelope = coefficient * envelope + (1.0 - coefficient) * detector;
        let input_db = 20.0 * envelope.max(1.0e-9).log10();
        let reduction_db = if input_db > threshold_db {
            (input_db - threshold_db) * (1.0 - 1.0 / ratio)
        } else {
            0.0
        };
        let gain = db_to_gain(makeup - reduction_db);
        stereo[0] *= gain;
        stereo[1] *= gain;
    }
}

fn apply_saturation(samples: &mut [f32], drive: f32, mix: f32) {
    let drive = 1.0 + drive.clamp(0.0, 10.0) * 2.0;
    let mix = mix.clamp(0.0, 1.0);
    let normalization = drive.tanh().max(f32::EPSILON);
    for sample in samples {
        let wet = (*sample * drive).tanh() / normalization;
        *sample = *sample * (1.0 - mix) + wet * mix;
    }
}

#[allow(clippy::too_many_arguments)]
fn render_tone_clip(
    output: &mut [f32],
    frame_count: usize,
    start: usize,
    length: usize,
    sample_rate: u32,
    frequency_hz: f32,
    amplitude: f32,
    instrument: Instrument,
    apply_envelope: bool,
    clip_local_start: usize,
    clip_length: usize,
    clip_gain: f32,
    fade_in: usize,
    fade_out: usize,
    track_mix: TrackMix<'_>,
    expression: MidiExpression<'_>,
) {
    let mut phase = 0.0_f32;
    for local_frame in 0..length {
        let frame = start.saturating_add(local_frame);
        if frame >= frame_count {
            break;
        }

        let sine = (TAU * phase).sin();
        let oscillator = match instrument {
            Instrument::Sine => sine,
            Instrument::Triangle => (2.0 / PI) * sine.asin(),
            Instrument::Saw => 2.0 * phase.fract() - 1.0,
            Instrument::Square => {
                if sine >= 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        let absolute_frame = u64::try_from(frame).unwrap_or(u64::MAX);
        let bend = latest_pitch_bend(expression.pitch_bend, absolute_frame);
        let bend_semitones = f32::from(bend) / 8192.0 * 2.0;
        let bent_frequency = frequency_hz.max(0.0) * 2.0_f32.powf(bend_semitones / 12.0);
        #[allow(clippy::cast_precision_loss)]
        {
            phase = (phase + bent_frequency / sample_rate as f32).fract();
        }
        let pressure = pressure_gain(expression, absolute_frame);
        let envelope = if apply_envelope {
            note_envelope(local_frame, length, sample_rate)
        } else {
            1.0
        };
        let clip_envelope = clip_envelope(
            clip_local_start.saturating_add(local_frame),
            clip_length,
            fade_in,
            fade_out,
        );
        let sample = oscillator
            * amplitude.clamp(0.0, 1.0)
            * envelope
            * clip_gain
            * clip_envelope
            * pressure;
        mix_stereo_frame(output, frame, sample, sample, track_mix);
    }
}

fn latest_pitch_bend(points: &[MidiPitchBendPoint], frame: u64) -> i16 {
    points
        .get(
            points
                .partition_point(|point| point.frame <= frame)
                .saturating_sub(1),
        )
        .filter(|point| point.frame <= frame)
        .map_or(0, |point| point.value)
}

fn pressure_gain(expression: MidiExpression<'_>, frame: u64) -> f32 {
    let channel = expression
        .channel_pressure
        .get(
            expression
                .channel_pressure
                .partition_point(|point| point.frame <= frame)
                .saturating_sub(1),
        )
        .filter(|point| point.frame <= frame)
        .map(|point| point.value);
    let poly = expression.note.and_then(|note| {
        expression
            .poly_pressure
            .iter()
            .rev()
            .find(|point| point.frame <= frame && point.note == note)
            .map(|point| point.value)
    });
    channel
        .into_iter()
        .chain(poly)
        .map(|value| 0.75 + f32::from(value) / 508.0)
        .product()
}

fn note_envelope(local_frame: usize, length: usize, sample_rate: u32) -> f32 {
    if length <= 1 {
        return 0.0;
    }
    let fade_frames = usize::try_from(sample_rate / 200)
        .unwrap_or(1)
        .max(1)
        .min((length / 2).max(1));
    #[allow(clippy::cast_precision_loss)]
    let attack = local_frame.min(fade_frames) as f32 / fade_frames as f32;
    let remaining = length.saturating_sub(local_frame + 1);
    #[allow(clippy::cast_precision_loss)]
    let release = remaining.min(fade_frames) as f32 / fade_frames as f32;
    attack.min(release).min(1.0)
}

fn clip_envelope(local_frame: usize, length: usize, fade_in: usize, fade_out: usize) -> f32 {
    if length == 0 || local_frame >= length {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let fade_in_gain = if fade_in == 0 {
        1.0
    } else {
        local_frame.min(fade_in) as f32 / fade_in as f32
    };
    let remaining = length.saturating_sub(local_frame + 1);
    #[allow(clippy::cast_precision_loss)]
    let fade_out_gain = if fade_out == 0 {
        1.0
    } else {
        remaining.min(fade_out) as f32 / fade_out as f32
    };
    fade_in_gain.min(fade_out_gain).clamp(0.0, 1.0)
}

#[allow(clippy::too_many_arguments)]
fn render_audio_file_clip(
    output: &mut [f32],
    frame_count: usize,
    start: usize,
    length: usize,
    project_sample_rate: u32,
    source_offset_frames: u64,
    decoded: &DecodedAudio,
    clip_gain: f32,
    fade_in: usize,
    fade_out: usize,
    track_mix: TrackMix<'_>,
) {
    for local_frame in 0..length {
        let frame = start.saturating_add(local_frame);
        if frame >= frame_count {
            break;
        }
        let Ok(local_frame) = u64::try_from(local_frame) else {
            break;
        };
        let Some([left, right]) =
            decoded.sample_at_rate(source_offset_frames, local_frame, project_sample_rate)
        else {
            break;
        };
        let envelope = clip_envelope(
            usize::try_from(local_frame).unwrap_or(usize::MAX),
            length,
            fade_in,
            fade_out,
        ) * clip_gain;
        mix_stereo_frame(output, frame, left * envelope, right * envelope, track_mix);
    }
}

fn mix_stereo_frame(
    output: &mut [f32],
    frame: usize,
    left: f32,
    right: f32,
    track_mix: TrackMix<'_>,
) {
    let (left_gain, right_gain) = track_mix.gains_at(frame);
    output[frame * 2] += left * left_gain;
    output[frame * 2 + 1] += right * right_gain;
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::{
        Bus, ChannelInsert, ChannelOutput, Clip, ClipSource, InsertEffect, MidiControlPoint,
        MidiNote, Project, Track, TrackSend,
    };

    use super::*;

    fn tone_project(muted: bool) -> Project {
        let mut project = Project::new("Test", 48_000, 120.0).unwrap();
        let mut track = Track::new("Tone");
        track.muted = muted;
        track.clips.push(Clip {
            name: "Tone".into(),
            start_frame: 0,
            length_frames: 10,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::Sine {
                frequency_hz: 12_000.0,
                amplitude: 1.0,
            },
        });
        project.tracks.push(track);
        project
    }

    fn wav_test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dmo-render-{label}-{}.wav", std::process::id()))
    }

    #[test]
    fn renders_interleaved_stereo() {
        let rendered = render_stereo(&tone_project(false));
        assert_eq!(rendered.len(), 20);
        assert!((rendered[2] - 0.707_106_77).abs() < 0.000_1);
        assert!((rendered[3] - 0.707_106_77).abs() < 0.000_1);
    }

    #[test]
    fn clip_gain_and_fades_shape_generated_audio() {
        let mut project = tone_project(false);
        let clip = &mut project.tracks[0].clips[0];
        clip.gain = 0.5;
        clip.fade_in_frames = 2;
        clip.fade_out_frames = 2;

        let rendered = render_stereo(&project);

        assert!((rendered[2] - 0.176_776_69).abs() < 0.000_1);
        assert!(rendered[18].abs() < f32::EPSILON);
    }

    #[test]
    fn master_gain_and_track_automation_shape_the_mix() {
        let mut project = tone_project(false);
        project.master_gain = 0.5;
        project.tracks[0].volume_automation = vec![
            AutomationPoint {
                frame: 0,
                value: 0.5,
            },
            AutomationPoint {
                frame: 2,
                value: 1.0,
            },
        ];

        let rendered = render_stereo(&project);

        assert!((rendered[2] - 0.265_165_03).abs() < 0.000_1);
    }

    #[test]
    fn unity_bus_routing_preserves_the_track_signal() {
        let mut project = tone_project(false);
        let direct = render_stereo(&project);
        project.buses.push(Bus::new("Group"));
        project.tracks[0].output = ChannelOutput::Bus(0);

        let routed = render_stereo(&project);

        assert_eq!(routed.len(), direct.len());
        assert!(
            routed
                .iter()
                .zip(direct)
                .all(|(routed, direct)| (*routed - direct).abs() < 0.000_01)
        );
    }

    #[test]
    fn sends_and_insert_gain_are_processed_in_signal_order() {
        let mut project = tone_project(false);
        project.buses.push(Bus::new("Parallel"));
        project.tracks[0].gain = 0.5;
        project.tracks[0].sends.push(TrackSend {
            bus_index: 0,
            gain: 0.5,
            enabled: true,
            pre_fader: false,
        });
        let parallel = render_stereo(&project);
        assert!((parallel[2] - 0.707_106_77 * 0.75).abs() < 0.000_1);

        project.tracks[0]
            .inserts
            .push(ChannelInsert::new(InsertEffect::Gain { gain_db: -6.020_6 }));
        let attenuated = render_stereo(&project);
        assert!((attenuated[2] - parallel[2] * 0.5).abs() < 0.000_2);
    }

    #[test]
    fn midi_volume_and_expression_cc_shape_the_track() {
        let mut project = tone_project(false);
        project.tracks[0].midi_cc = vec![
            MidiControlPoint {
                frame: 1,
                controller: 7,
                value: 64,
            },
            MidiControlPoint {
                frame: 1,
                controller: 11,
                value: 0,
            },
        ];

        let rendered = render_stereo(&project);

        assert!(rendered[2].abs() < f32::EPSILON);
        assert!(rendered[3].abs() < f32::EPSILON);
    }

    #[test]
    fn renders_multiple_notes_from_one_midi_clip() {
        let mut project = Project::new("MIDI", 48_000, 120.0).unwrap();
        let mut track = Track::new("Lead");
        track.clips.push(Clip {
            name: "Melody Part".into(),
            start_frame: 0,
            length_frames: 8,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::Midi {
                notes: vec![
                    MidiNote {
                        start_frame: 0,
                        length_frames: 4,
                        midi_note: 60,
                        velocity: 127,
                    },
                    MidiNote {
                        start_frame: 4,
                        length_frames: 4,
                        midi_note: 72,
                        velocity: 127,
                    },
                ],
                amplitude: 1.0,
            },
        });
        project.tracks.push(track);

        let rendered = render_stereo(&project);

        assert_eq!(rendered.len(), 16);
        assert!(rendered[2].abs() > 0.001);
        assert!(rendered[10].abs() > 0.001);
    }

    #[test]
    fn midi_velocity_scales_note_level() {
        let mut project = Project::new("Velocity", 48_000, 120.0).unwrap();
        let mut track = Track::new("Lead");
        track.clips.push(Clip {
            name: "Part".into(),
            start_frame: 0,
            length_frames: 1_000,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::Midi {
                notes: vec![MidiNote {
                    start_frame: 0,
                    length_frames: 1_000,
                    midi_note: 69,
                    velocity: 127,
                }],
                amplitude: 1.0,
            },
        });
        project.tracks.push(track);

        let loud = render_stereo(&project);
        let ClipSource::Midi { notes, .. } = &mut project.tracks[0].clips[0].source else {
            panic!("expected MIDI clip");
        };
        notes[0].velocity = 32;
        let quiet = render_stereo(&project);

        assert!(loud[400].abs() > quiet[400].abs() * 3.0);
    }

    #[test]
    fn built_in_instruments_produce_distinct_enveloped_waveforms() {
        let mut project = Project::new("Instrument", 48_000, 120.0).unwrap();
        let mut track = Track::new("Synth");
        track.clips.push(Clip {
            name: "Part".into(),
            start_frame: 0,
            length_frames: 1_000,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::Midi {
                notes: vec![MidiNote {
                    start_frame: 0,
                    length_frames: 1_000,
                    midi_note: 69,
                    velocity: 127,
                }],
                amplitude: 1.0,
            },
        });
        project.tracks.push(track);

        let sine = render_stereo(&project);
        project.tracks[0].instrument = Instrument::Square;
        let square = render_stereo(&project);

        assert!(sine[0].abs() < f32::EPSILON);
        assert!(square[0].abs() < f32::EPSILON);
        assert_ne!(sine[400].to_bits(), square[400].to_bits());
        assert!(sine[sine.len() - 2].abs() < f32::EPSILON);
        assert!(square[square.len() - 2].abs() < f32::EPSILON);
    }

    #[test]
    fn muted_tracks_are_silent() {
        assert!(
            render_stereo(&tone_project(true))
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }

    #[test]
    fn solo_silences_every_non_solo_track() {
        let mut project = tone_project(false);
        let mut empty_solo = Track::new("Solo");
        empty_solo.soloed = true;
        project.tracks.push(empty_solo);

        assert!(
            render_stereo(&project)
                .iter()
                .all(|sample| sample.abs() < f32::EPSILON)
        );
    }

    #[test]
    fn renders_and_resamples_an_audio_file_clip() {
        let path = wav_test_path("resample");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(0_i16).unwrap();
        writer.write_sample(i16::MAX).unwrap();
        writer.finalize().unwrap();

        let mut project = Project::new("Audio", 48_000, 120.0).unwrap();
        let mut track = Track::new("File");
        track.clips.push(Clip {
            name: "Imported".into(),
            start_frame: 0,
            length_frames: 4,
            gain: 0.5,
            fade_in_frames: 0,
            fade_out_frames: 1,
            source: ClipSource::AudioFile {
                path: path.to_string_lossy().into_owned(),
                source_offset_frames: 0,
                source_sample_rate: 24_000,
                channels: 1,
            },
        });
        project.tracks.push(track);

        let rendered = try_render_stereo(&project).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(rendered.len(), 8);
        assert!((rendered[2] - 0.176_77).abs() < 0.000_1);
        assert!((rendered[4] - 0.353_54).abs() < 0.000_1);
        assert!(rendered[6].abs() < f32::EPSILON);
    }

    #[test]
    fn pitch_bend_and_aftertouch_shape_built_in_instruments() {
        let mut plain = tone_project(false);
        plain.tracks[0].clips[0].source = ClipSource::Midi {
            notes: vec![MidiNote {
                start_frame: 0,
                length_frames: 10,
                midi_note: 69,
                velocity: 127,
            }],
            amplitude: 1.0,
        };
        let baseline = render_stereo(&plain);

        let mut expressive = plain;
        expressive.tracks[0].midi_pitch_bend = vec![MidiPitchBendPoint {
            frame: 1,
            value: 4096,
        }];
        expressive.tracks[0].midi_channel_pressure =
            vec![MidiChannelPressurePoint { frame: 1, value: 0 }];
        expressive.tracks[0].midi_poly_pressure = vec![MidiPolyPressurePoint {
            frame: 1,
            note: 69,
            value: 127,
        }];
        let rendered = render_stereo(&expressive);

        assert!(rendered[2].abs() < baseline[2].abs());
        assert!((rendered[6] - baseline[6]).abs() > 0.000_1);
    }

    #[test]
    fn source_offset_is_measured_in_native_frames() {
        let path = wav_test_path("offset");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in [0.0_f32, 0.25, 0.5] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let mut project = Project::new("Offset", 48_000, 120.0).unwrap();
        let mut track = Track::new("File");
        track.clips.push(Clip {
            name: "Trimmed".into(),
            start_frame: 0,
            length_frames: 2,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::AudioFile {
                path: path.to_string_lossy().into_owned(),
                source_offset_frames: 1,
                source_sample_rate: 48_000,
                channels: 1,
            },
        });
        project.tracks.push(track);

        let rendered = try_render_stereo(&project).unwrap();
        let _ = fs::remove_file(path);
        assert!((rendered[0] - 0.176_776_7).abs() < 0.000_1);
        assert!((rendered[2] - 0.353_553_4).abs() < 0.000_1);
    }

    #[test]
    fn strict_render_reports_a_missing_file() {
        let path = wav_test_path("missing");
        let _ = fs::remove_file(&path);
        let mut project = Project::new("Missing", 48_000, 120.0).unwrap();
        let mut track = Track::new("File");
        track.clips.push(Clip {
            name: "Missing".into(),
            start_frame: 0,
            length_frames: 4,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::AudioFile {
                path: path.to_string_lossy().into_owned(),
                source_offset_frames: 0,
                source_sample_rate: 48_000,
                channels: 2,
            },
        });
        project.tracks.push(track);

        assert!(matches!(
            try_render_stereo(&project),
            Err(RenderError::AudioFile { .. })
        ));
        assert_eq!(render_stereo(&project), vec![0.0; 8]);
    }
}
