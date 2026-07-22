use std::{collections::HashMap, fmt, fs, path::Path};

use crate::{
    ClipSource, MidiChannelPressurePoint, MidiControlPoint, MidiNote, MidiPitchBendPoint,
    MidiPolyPressurePoint, Project, frequency_to_midi_note,
};

pub const MIDI_TICKS_PER_QUARTER: u16 = 960;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedMidiTrack {
    pub name: String,
    /// Conventional 1-based MIDI channel.
    pub channel: u8,
    pub notes: Vec<MidiNote>,
    /// Controller points relative to the imported file origin.
    pub controllers: Vec<MidiControlPoint>,
    pub pitch_bend: Vec<MidiPitchBendPoint>,
    pub channel_pressure: Vec<MidiChannelPressurePoint>,
    pub poly_pressure: Vec<MidiPolyPressurePoint>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedMidi {
    pub format: u16,
    pub ticks_per_quarter: u16,
    /// First tempo event in the file, if present.
    pub tempo_bpm: Option<f64>,
    pub duration_frames: u64,
    pub tracks: Vec<ImportedMidiTrack>,
}

#[derive(Debug)]
pub enum MidiFileError {
    InvalidData(String),
    UnsupportedSmpteDivision(u16),
    Io(std::io::Error),
}

impl fmt::Display for MidiFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(message) => write!(formatter, "invalid MIDI file: {message}"),
            Self::UnsupportedSmpteDivision(division) => write!(
                formatter,
                "SMPTE MIDI time division 0x{division:04X} is not supported"
            ),
            Self::Io(error) => write!(formatter, "MIDI file I/O failed: {error}"),
        }
    }
}

impl std::error::Error for MidiFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidData(_) | Self::UnsupportedSmpteDivision(_) => None,
        }
    }
}

impl From<std::io::Error> for MidiFileError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Copy)]
struct RawNote {
    start_tick: u64,
    length_ticks: u64,
    note: u8,
    velocity: u8,
}

#[derive(Debug, Default)]
struct RawTrack {
    name: String,
    notes: HashMap<u8, Vec<RawNote>>,
    controllers: HashMap<u8, Vec<(u64, u8, u8)>>,
    pitch_bend: HashMap<u8, Vec<(u64, i16)>>,
    channel_pressure: HashMap<u8, Vec<(u64, u8)>>,
    poly_pressure: HashMap<u8, Vec<(u64, u8, u8)>>,
    tempos: Vec<(u64, u32)>,
    end_tick: u64,
}

/// Decodes a Standard MIDI File (format 0, 1, or 2) into project-frame notes
/// and controller points. The first tempo event is used because the current
/// project model has one global tempo.
///
/// # Errors
///
/// Returns [`MidiFileError`] for malformed data, unsupported SMPTE timing, or
/// invalid conversion settings.
#[allow(clippy::too_many_lines)]
pub fn decode_midi(
    bytes: &[u8],
    sample_rate: u32,
    fallback_tempo_bpm: f64,
) -> Result<ImportedMidi, MidiFileError> {
    if sample_rate == 0 || !fallback_tempo_bpm.is_finite() || fallback_tempo_bpm <= 0.0 {
        return Err(MidiFileError::InvalidData(
            "sample rate and fallback tempo must be positive".into(),
        ));
    }
    if bytes.len() < 14 || &bytes[..4] != b"MThd" {
        return Err(MidiFileError::InvalidData("missing MThd header".into()));
    }
    let header_length = read_be_u32(bytes, 4)? as usize;
    if header_length < 6 || bytes.len() < 8_usize.saturating_add(header_length) {
        return Err(MidiFileError::InvalidData(
            "truncated MIDI header chunk".into(),
        ));
    }
    let format = read_be_u16(bytes, 8)?;
    if format > 2 {
        return Err(MidiFileError::InvalidData(format!(
            "unsupported SMF format {format}"
        )));
    }
    let track_count = usize::from(read_be_u16(bytes, 10)?);
    let division = read_be_u16(bytes, 12)?;
    if division & 0x8000 != 0 {
        return Err(MidiFileError::UnsupportedSmpteDivision(division));
    }
    if division == 0 {
        return Err(MidiFileError::InvalidData(
            "ticks per quarter note cannot be zero".into(),
        ));
    }

    let mut offset = 8 + header_length;
    let mut raw_tracks = Vec::with_capacity(track_count);
    for track_index in 0..track_count {
        if offset.saturating_add(8) > bytes.len() || &bytes[offset..offset + 4] != b"MTrk" {
            return Err(MidiFileError::InvalidData(format!(
                "missing MTrk chunk for track {}",
                track_index + 1
            )));
        }
        let length = read_be_u32(bytes, offset + 4)? as usize;
        let start = offset + 8;
        let end = start
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| MidiFileError::InvalidData("truncated MTrk chunk".into()))?;
        raw_tracks.push(parse_track(&bytes[start..end], track_index)?);
        offset = end;
    }

    let tempo_event = raw_tracks
        .iter()
        .flat_map(|track| &track.tempos)
        .min_by_key(|(tick, _)| *tick)
        .copied();
    let tempo_bpm = tempo_event.map(|(_, micros)| 60_000_000.0 / f64::from(micros));
    let conversion_tempo = tempo_bpm.unwrap_or(fallback_tempo_bpm);
    let mut tracks = Vec::new();
    let mut duration_frames = 0_u64;
    for (track_index, raw) in raw_tracks.into_iter().enumerate() {
        let mut channels = raw
            .notes
            .keys()
            .chain(raw.controllers.keys())
            .chain(raw.pitch_bend.keys())
            .chain(raw.channel_pressure.keys())
            .chain(raw.poly_pressure.keys())
            .copied()
            .collect::<Vec<_>>();
        channels.sort_unstable();
        channels.dedup();
        let split_channels = channels.len() > 1;
        for channel in channels {
            let mut notes = raw
                .notes
                .get(&channel)
                .into_iter()
                .flatten()
                .map(|note| MidiNote {
                    start_frame: ticks_to_frames(
                        note.start_tick,
                        division,
                        sample_rate,
                        conversion_tempo,
                    ),
                    length_frames: ticks_to_frames(
                        note.length_ticks,
                        division,
                        sample_rate,
                        conversion_tempo,
                    )
                    .max(1),
                    midi_note: note.note,
                    velocity: note.velocity.max(1),
                })
                .collect::<Vec<_>>();
            notes.sort_by_key(|note| (note.start_frame, note.midi_note));
            let mut controllers = raw
                .controllers
                .get(&channel)
                .into_iter()
                .flatten()
                .map(|(tick, controller, value)| MidiControlPoint {
                    frame: ticks_to_frames(*tick, division, sample_rate, conversion_tempo),
                    controller: *controller,
                    value: *value,
                })
                .collect::<Vec<_>>();
            controllers.sort_by_key(|point| (point.frame, point.controller));
            let mut pitch_bend = raw
                .pitch_bend
                .get(&channel)
                .into_iter()
                .flatten()
                .map(|(tick, value)| MidiPitchBendPoint {
                    frame: ticks_to_frames(*tick, division, sample_rate, conversion_tempo),
                    value: *value,
                })
                .collect::<Vec<_>>();
            pitch_bend.sort_by_key(|point| point.frame);
            let mut channel_pressure = raw
                .channel_pressure
                .get(&channel)
                .into_iter()
                .flatten()
                .map(|(tick, value)| MidiChannelPressurePoint {
                    frame: ticks_to_frames(*tick, division, sample_rate, conversion_tempo),
                    value: *value,
                })
                .collect::<Vec<_>>();
            channel_pressure.sort_by_key(|point| point.frame);
            let mut poly_pressure = raw
                .poly_pressure
                .get(&channel)
                .into_iter()
                .flatten()
                .map(|(tick, note, value)| MidiPolyPressurePoint {
                    frame: ticks_to_frames(*tick, division, sample_rate, conversion_tempo),
                    note: *note,
                    value: *value,
                })
                .collect::<Vec<_>>();
            poly_pressure.sort_by_key(|point| (point.frame, point.note));
            let channel_duration = notes
                .iter()
                .map(|note| note.end_frame())
                .chain(controllers.iter().map(|point| point.frame))
                .chain(pitch_bend.iter().map(|point| point.frame))
                .chain(channel_pressure.iter().map(|point| point.frame))
                .chain(poly_pressure.iter().map(|point| point.frame))
                .max()
                .unwrap_or_else(|| {
                    ticks_to_frames(raw.end_tick, division, sample_rate, conversion_tempo)
                });
            duration_frames = duration_frames.max(channel_duration);
            let base_name = if raw.name.is_empty() {
                format!("MIDI Track {}", track_index + 1)
            } else {
                raw.name.clone()
            };
            tracks.push(ImportedMidiTrack {
                name: if split_channels {
                    format!("{base_name} Ch {}", channel + 1)
                } else {
                    base_name
                },
                channel: channel + 1,
                notes,
                controllers,
                pitch_bend,
                channel_pressure,
                poly_pressure,
            });
        }
    }

    Ok(ImportedMidi {
        format,
        ticks_per_quarter: division,
        tempo_bpm,
        duration_frames,
        tracks,
    })
}

/// Loads and decodes a Standard MIDI File.
///
/// # Errors
///
/// Returns [`MidiFileError`] when the file cannot be read or decoded.
pub fn load_midi(
    path: impl AsRef<Path>,
    sample_rate: u32,
    fallback_tempo_bpm: f64,
) -> Result<ImportedMidi, MidiFileError> {
    decode_midi(&fs::read(path)?, sample_rate, fallback_tempo_bpm)
}

/// Encodes project MIDI clips and CC events as a format-1 Standard MIDI File.
///
/// # Errors
///
/// Returns [`MidiFileError`] when the project timing or resulting SMF sizes
/// cannot be represented safely.
pub fn encode_project_midi(project: &Project) -> Result<Vec<u8>, MidiFileError> {
    if project.sample_rate == 0 || !project.tempo_bpm.is_finite() || project.tempo_bpm <= 0.0 {
        return Err(MidiFileError::InvalidData(
            "project sample rate and tempo must be positive".into(),
        ));
    }
    let midi_tracks = project
        .tracks
        .iter()
        .filter(|track| {
            !track.midi_cc.is_empty()
                || !track.midi_pitch_bend.is_empty()
                || !track.midi_channel_pressure.is_empty()
                || !track.midi_poly_pressure.is_empty()
                || track.clips.iter().any(|clip| {
                    matches!(
                        clip.source,
                        ClipSource::Midi { .. } | ClipSource::Sine { .. }
                    )
                })
        })
        .collect::<Vec<_>>();
    let track_count = u16::try_from(midi_tracks.len().saturating_add(1)).map_err(|_| {
        MidiFileError::InvalidData("too many tracks for a Standard MIDI File".into())
    })?;
    let mut output = Vec::new();
    output.extend_from_slice(b"MThd");
    output.extend_from_slice(&6_u32.to_be_bytes());
    output.extend_from_slice(&1_u16.to_be_bytes());
    output.extend_from_slice(&track_count.to_be_bytes());
    output.extend_from_slice(&MIDI_TICKS_PER_QUARTER.to_be_bytes());

    append_track_chunk(&mut output, &tempo_track(project.tempo_bpm)?)?;
    for track in midi_tracks {
        append_track_chunk(&mut output, &encode_track(project, track)?)?;
    }
    Ok(output)
}

/// Saves project MIDI as a format-1 Standard MIDI File.
///
/// # Errors
///
/// Returns [`MidiFileError`] when encoding or writing fails.
pub fn save_project_midi(path: impl AsRef<Path>, project: &Project) -> Result<(), MidiFileError> {
    fs::write(path, encode_project_midi(project)?)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn parse_track(bytes: &[u8], track_index: usize) -> Result<RawTrack, MidiFileError> {
    let mut result = RawTrack::default();
    let mut active = HashMap::<(u8, u8), Vec<(u64, u8)>>::new();
    let mut offset = 0_usize;
    let mut tick = 0_u64;
    let mut running_status = None;
    while offset < bytes.len() {
        let delta = read_vlq(bytes, &mut offset)?;
        tick = tick.saturating_add(u64::from(delta));
        if offset >= bytes.len() {
            return Err(MidiFileError::InvalidData(format!(
                "track {} ends after a delta time",
                track_index + 1
            )));
        }
        let next = bytes[offset];
        let (status, first_data) = if next & 0x80 == 0 {
            (
                running_status.ok_or_else(|| {
                    MidiFileError::InvalidData("running status without prior status".into())
                })?,
                Some(next),
            )
        } else {
            offset += 1;
            if next < 0xF0 {
                running_status = Some(next);
            }
            (next, None)
        };

        match status {
            0x80..=0xEF => {
                let kind = status & 0xF0;
                let channel = status & 0x0F;
                let data_length = if matches!(kind, 0xC0 | 0xD0) { 1 } else { 2 };
                let first = if let Some(first) = first_data {
                    offset += 1;
                    first
                } else {
                    read_byte(bytes, &mut offset)?
                };
                let second = if data_length == 2 {
                    read_byte(bytes, &mut offset)?
                } else {
                    0
                };
                match kind {
                    0x80 => finish_note(&mut result, &mut active, channel, first, tick),
                    0x90 if second == 0 => {
                        finish_note(&mut result, &mut active, channel, first, tick);
                    }
                    0x90 => active
                        .entry((channel, first))
                        .or_default()
                        .push((tick, second)),
                    0xB0 => result.controllers.entry(channel).or_default().push((
                        tick,
                        first.min(127),
                        second.min(127),
                    )),
                    0xA0 => result.poly_pressure.entry(channel).or_default().push((
                        tick,
                        first.min(127),
                        second.min(127),
                    )),
                    0xD0 => result
                        .channel_pressure
                        .entry(channel)
                        .or_default()
                        .push((tick, first.min(127))),
                    0xE0 => {
                        let unsigned =
                            u16::from(first.min(127)) | (u16::from(second.min(127)) << 7);
                        let value = i16::try_from(unsigned).unwrap_or(16_383) - 8192;
                        result
                            .pitch_bend
                            .entry(channel)
                            .or_default()
                            .push((tick, value));
                    }
                    _ => {}
                }
            }
            0xFF => {
                running_status = None;
                let meta_type = read_byte(bytes, &mut offset)?;
                let length = read_vlq(bytes, &mut offset)? as usize;
                let end = offset
                    .checked_add(length)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| MidiFileError::InvalidData("truncated meta event".into()))?;
                let data = &bytes[offset..end];
                match (meta_type, data) {
                    (0x03, name) => result.name = String::from_utf8_lossy(name).into_owned(),
                    (0x51, [a, b, c]) => {
                        let micros = u32::from(*a) << 16 | u32::from(*b) << 8 | u32::from(*c);
                        if micros > 0 {
                            result.tempos.push((tick, micros));
                        }
                    }
                    _ => {}
                }
                offset = end;
            }
            0xF0 | 0xF7 => {
                running_status = None;
                let length = read_vlq(bytes, &mut offset)? as usize;
                offset = offset
                    .checked_add(length)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| MidiFileError::InvalidData("truncated SysEx event".into()))?;
            }
            other => {
                return Err(MidiFileError::InvalidData(format!(
                    "unsupported system status 0x{other:02X}"
                )));
            }
        }
    }
    for ((channel, note), starts) in active {
        for (start_tick, velocity) in starts {
            result.notes.entry(channel).or_default().push(RawNote {
                start_tick,
                length_ticks: tick.saturating_sub(start_tick).max(1),
                note,
                velocity,
            });
        }
    }
    result.end_tick = tick;
    Ok(result)
}

fn finish_note(
    track: &mut RawTrack,
    active: &mut HashMap<(u8, u8), Vec<(u64, u8)>>,
    channel: u8,
    note: u8,
    tick: u64,
) {
    let Some(starts) = active.get_mut(&(channel, note)) else {
        return;
    };
    if let Some((start_tick, velocity)) = starts.first().copied() {
        starts.remove(0);
        track.notes.entry(channel).or_default().push(RawNote {
            start_tick,
            length_ticks: tick.saturating_sub(start_tick).max(1),
            note,
            velocity,
        });
    }
    if starts.is_empty() {
        active.remove(&(channel, note));
    }
}

#[derive(Debug)]
struct MidiEvent {
    tick: u64,
    priority: u8,
    bytes: Vec<u8>,
}

fn tempo_track(tempo_bpm: f64) -> Result<Vec<u8>, MidiFileError> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let micros = (60_000_000.0 / tempo_bpm).round() as u32;
    let micros = micros.clamp(1, 0x00FF_FFFF);
    let mut track = Vec::new();
    write_vlq(&mut track, 0)?;
    track.extend_from_slice(&[
        0xFF,
        0x51,
        0x03,
        ((micros >> 16) & 0xFF) as u8,
        ((micros >> 8) & 0xFF) as u8,
        (micros & 0xFF) as u8,
    ]);
    track.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
    Ok(track)
}

#[allow(clippy::too_many_lines)]
fn encode_track(project: &Project, track: &crate::Track) -> Result<Vec<u8>, MidiFileError> {
    let channel = track.midi_channel.clamp(1, 16) - 1;
    let mut events = Vec::<MidiEvent>::new();
    for clip in &track.clips {
        match &clip.source {
            ClipSource::Midi { notes, .. } => {
                for note in notes {
                    let start_frame = clip.start_frame.saturating_add(note.start_frame);
                    let end_frame = start_frame.saturating_add(note.length_frames.max(1));
                    events.push(MidiEvent {
                        tick: frames_to_ticks(start_frame, project),
                        priority: 2,
                        bytes: vec![
                            0x90 | channel,
                            note.midi_note.min(127),
                            note.velocity.max(1),
                        ],
                    });
                    events.push(MidiEvent {
                        tick: frames_to_ticks(end_frame, project),
                        priority: 0,
                        bytes: vec![0x80 | channel, note.midi_note.min(127), 0],
                    });
                }
            }
            ClipSource::Sine { frequency_hz, .. } => {
                let note = frequency_to_midi_note(*frequency_hz).min(127);
                events.push(MidiEvent {
                    tick: frames_to_ticks(clip.start_frame, project),
                    priority: 2,
                    bytes: vec![0x90 | channel, note, 100],
                });
                events.push(MidiEvent {
                    tick: frames_to_ticks(clip.end_frame(), project),
                    priority: 0,
                    bytes: vec![0x80 | channel, note, 0],
                });
            }
            ClipSource::AudioFile { .. } => {}
        }
    }
    for point in &track.midi_cc {
        events.push(MidiEvent {
            tick: frames_to_ticks(point.frame, project),
            priority: 1,
            bytes: vec![
                0xB0 | channel,
                point.controller.min(127),
                point.value.min(127),
            ],
        });
    }
    for point in &track.midi_pitch_bend {
        let unsigned =
            u16::try_from((i32::from(point.value) + 8192).clamp(0, 16_383)).unwrap_or(8192);
        events.push(MidiEvent {
            tick: frames_to_ticks(point.frame, project),
            priority: 1,
            bytes: vec![
                0xE0 | channel,
                u8::try_from(unsigned & 0x7F).unwrap_or(0),
                u8::try_from((unsigned >> 7) & 0x7F).unwrap_or(64),
            ],
        });
    }
    for point in &track.midi_channel_pressure {
        events.push(MidiEvent {
            tick: frames_to_ticks(point.frame, project),
            priority: 1,
            bytes: vec![0xD0 | channel, point.value.min(127)],
        });
    }
    for point in &track.midi_poly_pressure {
        events.push(MidiEvent {
            tick: frames_to_ticks(point.frame, project),
            priority: 1,
            bytes: vec![0xA0 | channel, point.note.min(127), point.value.min(127)],
        });
    }
    events.sort_by_key(|event| (event.tick, event.priority));

    let mut encoded = Vec::new();
    write_vlq(&mut encoded, 0)?;
    encoded.extend_from_slice(&[0xFF, 0x03]);
    write_vlq(
        &mut encoded,
        u32::try_from(track.name.len())
            .map_err(|_| MidiFileError::InvalidData("track name is too long".into()))?,
    )?;
    encoded.extend_from_slice(track.name.as_bytes());
    let mut previous_tick = 0_u64;
    for event in events {
        let delta = event.tick.saturating_sub(previous_tick);
        write_vlq(
            &mut encoded,
            u32::try_from(delta)
                .map_err(|_| MidiFileError::InvalidData("MIDI delta time is too large".into()))?,
        )?;
        encoded.extend_from_slice(&event.bytes);
        previous_tick = event.tick;
    }
    encoded.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
    Ok(encoded)
}

fn append_track_chunk(output: &mut Vec<u8>, track: &[u8]) -> Result<(), MidiFileError> {
    let length = u32::try_from(track.len())
        .map_err(|_| MidiFileError::InvalidData("MIDI track is too large".into()))?;
    output.extend_from_slice(b"MTrk");
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(track);
    Ok(())
}

fn ticks_to_frames(ticks: u64, division: u16, sample_rate: u32, tempo_bpm: f64) -> u64 {
    #[allow(clippy::cast_precision_loss)]
    let frames = ticks as f64 * f64::from(sample_rate) * 60.0 / (f64::from(division) * tempo_bpm);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        frames.round() as u64
    }
}

fn frames_to_ticks(frame: u64, project: &Project) -> u64 {
    #[allow(clippy::cast_precision_loss)]
    let ticks = frame as f64 * project.tempo_bpm * f64::from(MIDI_TICKS_PER_QUARTER)
        / (f64::from(project.sample_rate) * 60.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        ticks.round() as u64
    }
}

fn read_be_u16(bytes: &[u8], offset: usize) -> Result<u16, MidiFileError> {
    let data = bytes
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| MidiFileError::InvalidData("truncated 16-bit field".into()))?;
    Ok(u16::from_be_bytes([data[0], data[1]]))
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32, MidiFileError> {
    let data = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| MidiFileError::InvalidData("truncated 32-bit field".into()))?;
    Ok(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
}

fn read_byte(bytes: &[u8], offset: &mut usize) -> Result<u8, MidiFileError> {
    let byte = bytes
        .get(*offset)
        .copied()
        .ok_or_else(|| MidiFileError::InvalidData("truncated MIDI event".into()))?;
    *offset += 1;
    Ok(byte)
}

fn read_vlq(bytes: &[u8], offset: &mut usize) -> Result<u32, MidiFileError> {
    let mut value = 0_u32;
    for _ in 0..4 {
        let byte = read_byte(bytes, offset)?;
        value = (value << 7) | u32::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(MidiFileError::InvalidData(
        "variable-length quantity exceeds four bytes".into(),
    ))
}

fn write_vlq(output: &mut Vec<u8>, value: u32) -> Result<(), MidiFileError> {
    if value > 0x0FFF_FFFF {
        return Err(MidiFileError::InvalidData(
            "variable-length quantity exceeds 28 bits".into(),
        ));
    }
    let mut buffer = [0_u8; 4];
    let mut index = 3;
    buffer[index] = (value & 0x7F) as u8;
    let mut remaining = value >> 7;
    while remaining > 0 {
        index -= 1;
        buffer[index] = ((remaining & 0x7F) as u8) | 0x80;
        remaining >>= 7;
    }
    output.extend_from_slice(&buffer[index..]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, ClipSource, Track};

    #[test]
    fn format_one_round_trips_notes_cc_tempo_and_track_names() {
        let mut project = Project::new("MIDI", 48_000, 128.0).unwrap();
        let mut track = Track::new("Lead Synth");
        track.midi_channel = 3;
        track.midi_cc.push(MidiControlPoint {
            frame: 12_000,
            controller: 11,
            value: 96,
        });
        track.midi_pitch_bend.push(MidiPitchBendPoint {
            frame: 18_000,
            value: 4096,
        });
        track.midi_channel_pressure.push(MidiChannelPressurePoint {
            frame: 20_000,
            value: 87,
        });
        track.midi_poly_pressure.push(MidiPolyPressurePoint {
            frame: 22_000,
            note: 64,
            value: 73,
        });
        track.clips.push(Clip {
            name: "Part".into(),
            start_frame: 24_000,
            length_frames: 48_000,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::Midi {
                notes: vec![MidiNote {
                    start_frame: 0,
                    length_frames: 24_000,
                    midi_note: 64,
                    velocity: 110,
                }],
                amplitude: 0.8,
            },
        });
        project.tracks.push(track);

        let bytes = encode_project_midi(&project).unwrap();
        let decoded = decode_midi(&bytes, 48_000, 120.0).unwrap();

        assert_eq!(decoded.format, 1);
        assert!((decoded.tempo_bpm.unwrap() - 128.0).abs() < 0.001);
        assert_eq!(decoded.tracks.len(), 1);
        assert_eq!(decoded.tracks[0].name, "Lead Synth");
        assert_eq!(decoded.tracks[0].channel, 3);
        assert_eq!(decoded.tracks[0].notes[0].midi_note, 64);
        assert_eq!(decoded.tracks[0].notes[0].velocity, 110);
        assert_eq!(decoded.tracks[0].controllers[0].controller, 11);
        assert_eq!(decoded.tracks[0].controllers[0].value, 96);
        assert_eq!(decoded.tracks[0].pitch_bend[0].value, 4096);
        assert_eq!(decoded.tracks[0].channel_pressure[0].value, 87);
        assert_eq!(decoded.tracks[0].poly_pressure[0].note, 64);
        assert_eq!(decoded.tracks[0].poly_pressure[0].value, 73);
    }

    #[test]
    fn running_status_and_zero_velocity_note_off_are_supported() {
        let bytes = [
            b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xE0, b'M', b'T', b'r', b'k', 0,
            0, 0, 12, 0, 0x90, 60, 100, 0x83, 0x60, 60, 0, 0, 0xFF, 0x2F, 0,
        ];

        let decoded = decode_midi(&bytes, 48_000, 120.0).unwrap();

        assert_eq!(decoded.tracks.len(), 1);
        assert_eq!(decoded.tracks[0].notes.len(), 1);
        assert_eq!(decoded.tracks[0].notes[0].midi_note, 60);
        assert_eq!(decoded.tracks[0].notes[0].length_frames, 24_000);
    }

    #[test]
    fn malformed_and_smpte_files_are_reported() {
        assert!(matches!(
            decode_midi(b"not midi", 48_000, 120.0),
            Err(MidiFileError::InvalidData(_))
        ));
        let smpte = [b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 0, 0xE7, 0x28];
        assert!(matches!(
            decode_midi(&smpte, 48_000, 120.0),
            Err(MidiFileError::UnsupportedSmpteDivision(_))
        ));
    }
}
