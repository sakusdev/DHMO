use std::{collections::VecDeque, fmt};

use crate::{
    AudioTake, Bus, ChannelInsert, ChannelOutput, Clip, Instrument, Project, Track, TrackSend,
};

/// The number of edits retained by [`EditHistory::default`].
pub const DEFAULT_EDIT_HISTORY_LIMIT: usize = 100;

/// A reversible edit to a [`Project`].
///
/// Tracks and clips use their current indices because the project model does
/// not yet assign persistent IDs. All indices are checked before an edit is
/// made.
#[derive(Debug, Clone, PartialEq)]
pub enum EditCommand {
    /// Sets the project tempo in beats per minute.
    SetTempo { tempo_bpm: f64 },
    /// Sets the linear master output gain.
    SetMasterGain { gain: f32 },
    /// Replaces the ordered master insert chain.
    SetMasterInserts { inserts: Vec<ChannelInsert> },
    /// Inserts a bus while preserving existing bus references.
    AddBus { index: usize, bus: Bus },
    /// Deletes a bus and safely reroutes affected tracks to the master.
    DeleteBus { index: usize },
    /// Replaces a bus channel while preserving its project position.
    ReplaceBus { index: usize, bus: Bus },
    #[doc(hidden)]
    RestoreBus {
        index: usize,
        bus: Bus,
        routes: Vec<(ChannelOutput, Vec<TrackSend>)>,
    },
    /// Inserts a track at `index`. The current track count is a valid index and
    /// appends the track.
    AddTrack { index: usize, track: Track },
    /// Removes the track at `track_index`.
    DeleteTrack { track_index: usize },
    /// Replaces a track while preserving its project position.
    ReplaceTrack { track_index: usize, track: Track },
    /// Sets a track's linear gain.
    SetTrackGain { track_index: usize, gain: f32 },
    /// Sets a track's pan value.
    SetTrackPan { track_index: usize, pan: f32 },
    /// Sets a track's mute state.
    SetTrackMuted { track_index: usize, muted: bool },
    /// Sets a track's solo state.
    SetTrackSolo { track_index: usize, soloed: bool },
    /// Arms or disarms a track for audio recording.
    SetTrackRecordArmed {
        track_index: usize,
        record_armed: bool,
    },
    /// Enables or disables software input monitoring on a track.
    SetTrackInputMonitoring {
        track_index: usize,
        input_monitoring: bool,
    },
    /// Inserts a non-destructive alternate recording lane.
    AddTakeLane {
        track_index: usize,
        take_index: usize,
        take: AudioTake,
    },
    /// Removes an alternate recording lane.
    DeleteTakeLane {
        track_index: usize,
        take_index: usize,
    },
    /// Swaps an audible audio clip with one alternate take. Applying the same
    /// command again restores the previous comp, so undo is lossless.
    SwapClipWithTake {
        track_index: usize,
        clip_index: usize,
        take_index: usize,
    },
    /// Selects the built-in instrument used by MIDI clips on a track.
    SetTrackInstrument {
        track_index: usize,
        instrument: Instrument,
    },
    /// Sets a track's logical MIDI channel in the 1..=16 range.
    SetTrackMidiChannel {
        track_index: usize,
        midi_channel: u8,
    },
    /// Inserts a clip at `clip_index`. The current clip count is a valid index
    /// and appends the clip.
    AddClip {
        track_index: usize,
        clip_index: usize,
        clip: Clip,
    },
    /// Removes a clip from a track.
    DeleteClip {
        track_index: usize,
        clip_index: usize,
    },
    /// Replaces a clip while preserving its position in the track.
    ReplaceClip {
        track_index: usize,
        clip_index: usize,
        clip: Clip,
    },
    /// Moves a clip to an exact timeline frame.
    MoveClip {
        track_index: usize,
        clip_index: usize,
        start_frame: u64,
    },
    /// Replaces timeline bounds and the optional file-source offset. This is
    /// the reversible primitive used by non-destructive clip trimming.
    SetClipTiming {
        track_index: usize,
        clip_index: usize,
        start_frame: u64,
        length_frames: u64,
        source_offset_frames: Option<u64>,
    },
}

impl EditCommand {
    fn execute(self, project: &mut Project) -> Result<Self, EditError> {
        match self {
            Self::SetTempo { tempo_bpm } => set_tempo(project, tempo_bpm),
            Self::SetMasterGain { gain } => set_master_gain(project, gain),
            Self::SetMasterInserts { inserts } => Ok(set_master_inserts(project, inserts)),
            Self::AddBus { index, bus } => add_bus(project, index, bus),
            Self::DeleteBus { index } => delete_bus(project, index),
            Self::ReplaceBus { index, bus } => replace_bus(project, index, bus),
            Self::RestoreBus { index, bus, routes } => restore_bus(project, index, bus, routes),
            Self::AddTrack { index, track } => add_track(project, index, track),
            Self::DeleteTrack { track_index } => delete_track(project, track_index),
            Self::ReplaceTrack { track_index, track } => replace_track(project, track_index, track),
            Self::SetTrackGain { track_index, gain } => set_track_gain(project, track_index, gain),
            Self::SetTrackPan { track_index, pan } => set_track_pan(project, track_index, pan),
            Self::SetTrackMuted { track_index, muted } => {
                set_track_muted(project, track_index, muted)
            }
            Self::SetTrackSolo {
                track_index,
                soloed,
            } => set_track_solo(project, track_index, soloed),
            Self::SetTrackRecordArmed {
                track_index,
                record_armed,
            } => set_track_record_armed(project, track_index, record_armed),
            Self::SetTrackInputMonitoring {
                track_index,
                input_monitoring,
            } => set_track_input_monitoring(project, track_index, input_monitoring),
            Self::AddTakeLane {
                track_index,
                take_index,
                take,
            } => add_take_lane(project, track_index, take_index, take),
            Self::DeleteTakeLane {
                track_index,
                take_index,
            } => delete_take_lane(project, track_index, take_index),
            Self::SwapClipWithTake {
                track_index,
                clip_index,
                take_index,
            } => swap_clip_with_take(project, track_index, clip_index, take_index),
            Self::SetTrackInstrument {
                track_index,
                instrument,
            } => set_track_instrument(project, track_index, instrument),
            Self::SetTrackMidiChannel {
                track_index,
                midi_channel,
            } => set_track_midi_channel(project, track_index, midi_channel),
            Self::AddClip {
                track_index,
                clip_index,
                clip,
            } => add_clip(project, track_index, clip_index, clip),
            Self::DeleteClip {
                track_index,
                clip_index,
            } => delete_clip(project, track_index, clip_index),
            Self::ReplaceClip {
                track_index,
                clip_index,
                clip,
            } => replace_clip(project, track_index, clip_index, clip),
            Self::MoveClip {
                track_index,
                clip_index,
                start_frame,
            } => move_clip(project, track_index, clip_index, start_frame),
            Self::SetClipTiming {
                track_index,
                clip_index,
                start_frame,
                length_frames,
                source_offset_frames,
            } => set_clip_timing(
                project,
                track_index,
                clip_index,
                start_frame,
                length_frames,
                source_offset_frames,
            ),
        }
    }
}

fn set_tempo(project: &mut Project, tempo_bpm: f64) -> Result<EditCommand, EditError> {
    if !tempo_bpm.is_finite() || !(20.0..=400.0).contains(&tempo_bpm) {
        return Err(EditError::InvalidTempo(tempo_bpm));
    }
    let previous = project.tempo_bpm;
    project.tempo_bpm = tempo_bpm;
    Ok(EditCommand::SetTempo {
        tempo_bpm: previous,
    })
}

fn set_master_gain(project: &mut Project, gain: f32) -> Result<EditCommand, EditError> {
    if !gain.is_finite() || gain < 0.0 {
        return Err(EditError::InvalidMasterGain(gain));
    }
    let previous = project.master_gain;
    project.master_gain = gain;
    Ok(EditCommand::SetMasterGain { gain: previous })
}

fn set_master_inserts(project: &mut Project, inserts: Vec<ChannelInsert>) -> EditCommand {
    let previous = std::mem::replace(&mut project.master_inserts, inserts);
    EditCommand::SetMasterInserts { inserts: previous }
}

fn add_bus(project: &mut Project, index: usize, bus: Bus) -> Result<EditCommand, EditError> {
    if index > project.buses.len() {
        return Err(EditError::BusIndexOutOfBounds {
            bus_index: index,
            bus_count: project.buses.len(),
        });
    }
    for track in &mut project.tracks {
        if let ChannelOutput::Bus(bus_index) = &mut track.output
            && *bus_index >= index
        {
            *bus_index = bus_index.saturating_add(1);
        }
        for send in &mut track.sends {
            if send.bus_index >= index {
                send.bus_index = send.bus_index.saturating_add(1);
            }
        }
    }
    project.buses.insert(index, bus);
    Ok(EditCommand::DeleteBus { index })
}

fn delete_bus(project: &mut Project, index: usize) -> Result<EditCommand, EditError> {
    if index >= project.buses.len() {
        return Err(EditError::BusIndexOutOfBounds {
            bus_index: index,
            bus_count: project.buses.len(),
        });
    }
    let routes = project
        .tracks
        .iter()
        .map(|track| (track.output, track.sends.clone()))
        .collect();
    for track in &mut project.tracks {
        track.output = match track.output {
            ChannelOutput::Master => ChannelOutput::Master,
            ChannelOutput::Bus(bus_index) if bus_index == index => ChannelOutput::Master,
            ChannelOutput::Bus(bus_index) if bus_index > index => ChannelOutput::Bus(bus_index - 1),
            output @ ChannelOutput::Bus(_) => output,
        };
        track.sends.retain(|send| send.bus_index != index);
        for send in &mut track.sends {
            if send.bus_index > index {
                send.bus_index -= 1;
            }
        }
    }
    let bus = project.buses.remove(index);
    Ok(EditCommand::RestoreBus { index, bus, routes })
}

fn restore_bus(
    project: &mut Project,
    index: usize,
    bus: Bus,
    routes: Vec<(ChannelOutput, Vec<TrackSend>)>,
) -> Result<EditCommand, EditError> {
    if index > project.buses.len() {
        return Err(EditError::BusIndexOutOfBounds {
            bus_index: index,
            bus_count: project.buses.len(),
        });
    }
    if routes.len() != project.tracks.len() {
        return Err(EditError::TrackRouteCountMismatch {
            route_count: routes.len(),
            track_count: project.tracks.len(),
        });
    }
    project.buses.insert(index, bus);
    for (track, (output, sends)) in project.tracks.iter_mut().zip(routes) {
        track.output = output;
        track.sends = sends;
    }
    Ok(EditCommand::DeleteBus { index })
}

fn replace_bus(project: &mut Project, index: usize, bus: Bus) -> Result<EditCommand, EditError> {
    let bus_count = project.buses.len();
    let slot = project
        .buses
        .get_mut(index)
        .ok_or(EditError::BusIndexOutOfBounds {
            bus_index: index,
            bus_count,
        })?;
    let previous = std::mem::replace(slot, bus);
    Ok(EditCommand::ReplaceBus {
        index,
        bus: previous,
    })
}

fn add_track(project: &mut Project, index: usize, track: Track) -> Result<EditCommand, EditError> {
    if index > project.tracks.len() {
        return Err(EditError::TrackIndexOutOfBounds {
            track_index: index,
            track_count: project.tracks.len(),
        });
    }

    project.tracks.insert(index, track);
    Ok(EditCommand::DeleteTrack { track_index: index })
}

fn delete_track(project: &mut Project, track_index: usize) -> Result<EditCommand, EditError> {
    if track_index >= project.tracks.len() {
        return Err(EditError::TrackIndexOutOfBounds {
            track_index,
            track_count: project.tracks.len(),
        });
    }

    let track = project.tracks.remove(track_index);
    Ok(EditCommand::AddTrack {
        index: track_index,
        track,
    })
}

fn replace_track(
    project: &mut Project,
    track_index: usize,
    track: Track,
) -> Result<EditCommand, EditError> {
    let slot = track_mut(project, track_index)?;
    let previous = std::mem::replace(slot, track);
    Ok(EditCommand::ReplaceTrack {
        track_index,
        track: previous,
    })
}

fn set_track_gain(
    project: &mut Project,
    track_index: usize,
    gain: f32,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let previous = track.gain;
    track.gain = gain;
    Ok(EditCommand::SetTrackGain {
        track_index,
        gain: previous,
    })
}

fn set_track_pan(
    project: &mut Project,
    track_index: usize,
    pan: f32,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let previous = track.pan;
    track.pan = pan;
    Ok(EditCommand::SetTrackPan {
        track_index,
        pan: previous,
    })
}

fn set_track_muted(
    project: &mut Project,
    track_index: usize,
    muted: bool,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let previous = track.muted;
    track.muted = muted;
    Ok(EditCommand::SetTrackMuted {
        track_index,
        muted: previous,
    })
}

fn set_track_solo(
    project: &mut Project,
    track_index: usize,
    soloed: bool,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let previous = track.soloed;
    track.soloed = soloed;
    Ok(EditCommand::SetTrackSolo {
        track_index,
        soloed: previous,
    })
}

fn set_track_record_armed(
    project: &mut Project,
    track_index: usize,
    record_armed: bool,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let previous = track.recording.armed;
    track.recording.armed = record_armed;
    Ok(EditCommand::SetTrackRecordArmed {
        track_index,
        record_armed: previous,
    })
}

fn set_track_input_monitoring(
    project: &mut Project,
    track_index: usize,
    input_monitoring: bool,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let previous = track.recording.input_monitoring;
    track.recording.input_monitoring = input_monitoring;
    Ok(EditCommand::SetTrackInputMonitoring {
        track_index,
        input_monitoring: previous,
    })
}

fn add_take_lane(
    project: &mut Project,
    track_index: usize,
    take_index: usize,
    take: AudioTake,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    if take_index > track.take_lanes.len() {
        return Err(EditError::TakeIndexOutOfBounds {
            track_index,
            take_index,
            take_count: track.take_lanes.len(),
        });
    }
    track.take_lanes.insert(take_index, take);
    Ok(EditCommand::DeleteTakeLane {
        track_index,
        take_index,
    })
}

fn delete_take_lane(
    project: &mut Project,
    track_index: usize,
    take_index: usize,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    if take_index >= track.take_lanes.len() {
        return Err(EditError::TakeIndexOutOfBounds {
            track_index,
            take_index,
            take_count: track.take_lanes.len(),
        });
    }
    let take = track.take_lanes.remove(take_index);
    Ok(EditCommand::AddTakeLane {
        track_index,
        take_index,
        take,
    })
}

fn swap_clip_with_take(
    project: &mut Project,
    track_index: usize,
    clip_index: usize,
    take_index: usize,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let clip_count = track.clips.len();
    let clip = track
        .clips
        .get_mut(clip_index)
        .ok_or(EditError::ClipIndexOutOfBounds {
            track_index,
            clip_index,
            clip_count,
        })?;
    let Some(active_take) = AudioTake::from_clip(clip) else {
        return Err(EditError::ClipIsNotAudio {
            track_index,
            clip_index,
        });
    };
    let take_count = track.take_lanes.len();
    let alternate =
        track
            .take_lanes
            .get_mut(take_index)
            .ok_or(EditError::TakeIndexOutOfBounds {
                track_index,
                take_index,
                take_count,
            })?;
    *clip = alternate.to_clip();
    *alternate = active_take;
    Ok(EditCommand::SwapClipWithTake {
        track_index,
        clip_index,
        take_index,
    })
}

fn set_track_instrument(
    project: &mut Project,
    track_index: usize,
    instrument: Instrument,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let previous = track.instrument;
    track.instrument = instrument;
    Ok(EditCommand::SetTrackInstrument {
        track_index,
        instrument: previous,
    })
}

fn set_track_midi_channel(
    project: &mut Project,
    track_index: usize,
    midi_channel: u8,
) -> Result<EditCommand, EditError> {
    if !(1..=16).contains(&midi_channel) {
        return Err(EditError::InvalidMidiChannel(midi_channel));
    }
    let track = track_mut(project, track_index)?;
    let previous = track.midi_channel;
    track.midi_channel = midi_channel;
    Ok(EditCommand::SetTrackMidiChannel {
        track_index,
        midi_channel: previous,
    })
}

fn add_clip(
    project: &mut Project,
    track_index: usize,
    clip_index: usize,
    clip: Clip,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    if clip_index > track.clips.len() {
        return Err(EditError::ClipIndexOutOfBounds {
            track_index,
            clip_index,
            clip_count: track.clips.len(),
        });
    }

    track.clips.insert(clip_index, clip);
    Ok(EditCommand::DeleteClip {
        track_index,
        clip_index,
    })
}

fn delete_clip(
    project: &mut Project,
    track_index: usize,
    clip_index: usize,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    if clip_index >= track.clips.len() {
        return Err(EditError::ClipIndexOutOfBounds {
            track_index,
            clip_index,
            clip_count: track.clips.len(),
        });
    }

    let clip = track.clips.remove(clip_index);
    Ok(EditCommand::AddClip {
        track_index,
        clip_index,
        clip,
    })
}

fn replace_clip(
    project: &mut Project,
    track_index: usize,
    clip_index: usize,
    clip: Clip,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let clip_count = track.clips.len();
    let slot = track
        .clips
        .get_mut(clip_index)
        .ok_or(EditError::ClipIndexOutOfBounds {
            track_index,
            clip_index,
            clip_count,
        })?;
    let previous = std::mem::replace(slot, clip);
    Ok(EditCommand::ReplaceClip {
        track_index,
        clip_index,
        clip: previous,
    })
}

fn move_clip(
    project: &mut Project,
    track_index: usize,
    clip_index: usize,
    start_frame: u64,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let clip_count = track.clips.len();
    let clip = track
        .clips
        .get_mut(clip_index)
        .ok_or(EditError::ClipIndexOutOfBounds {
            track_index,
            clip_index,
            clip_count,
        })?;
    let previous = clip.start_frame;
    clip.start_frame = start_frame;
    Ok(EditCommand::MoveClip {
        track_index,
        clip_index,
        start_frame: previous,
    })
}

fn set_clip_timing(
    project: &mut Project,
    track_index: usize,
    clip_index: usize,
    start_frame: u64,
    length_frames: u64,
    source_offset_frames: Option<u64>,
) -> Result<EditCommand, EditError> {
    let track = track_mut(project, track_index)?;
    let clip_count = track.clips.len();
    let clip = track
        .clips
        .get_mut(clip_index)
        .ok_or(EditError::ClipIndexOutOfBounds {
            track_index,
            clip_index,
            clip_count,
        })?;

    let previous_offset = match &mut clip.source {
        crate::ClipSource::AudioFile {
            source_offset_frames: offset,
            ..
        } => {
            let previous = *offset;
            if let Some(next) = source_offset_frames {
                *offset = next;
            }
            Some(previous)
        }
        crate::ClipSource::Midi { .. } | crate::ClipSource::Sine { .. } => {
            if source_offset_frames.is_some() {
                return Err(EditError::SourceOffsetOnGeneratedClip {
                    track_index,
                    clip_index,
                });
            }
            None
        }
    };
    let previous_start = clip.start_frame;
    let previous_length = clip.length_frames;
    clip.start_frame = start_frame;
    clip.length_frames = length_frames;

    Ok(EditCommand::SetClipTiming {
        track_index,
        clip_index,
        start_frame: previous_start,
        length_frames: previous_length,
        source_offset_frames: previous_offset,
    })
}

fn track_mut(project: &mut Project, track_index: usize) -> Result<&mut Track, EditError> {
    let track_count = project.tracks.len();
    project
        .tracks
        .get_mut(track_index)
        .ok_or(EditError::TrackIndexOutOfBounds {
            track_index,
            track_count,
        })
}

/// Errors produced while applying or replaying an edit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditError {
    /// The tempo is not finite or falls outside the supported project range.
    InvalidTempo(f64),
    /// Master gain must be finite and non-negative.
    InvalidMasterGain(f32),
    BusIndexOutOfBounds {
        bus_index: usize,
        bus_count: usize,
    },
    TrackRouteCountMismatch {
        route_count: usize,
        track_count: usize,
    },
    /// A command referred to a track that does not exist. For insertion, an
    /// index equal to `track_count` is valid.
    TrackIndexOutOfBounds {
        track_index: usize,
        track_count: usize,
    },
    /// A command referred to a clip that does not exist. For insertion, an
    /// index equal to `clip_count` is valid.
    ClipIndexOutOfBounds {
        track_index: usize,
        clip_index: usize,
        clip_count: usize,
    },
    /// A command referred to an alternate take that does not exist.
    TakeIndexOutOfBounds {
        track_index: usize,
        take_index: usize,
        take_count: usize,
    },
    /// Only file-backed audio clips can be exchanged with recorded takes.
    ClipIsNotAudio {
        track_index: usize,
        clip_index: usize,
    },
    /// A generated clip was given an offset that only file-backed clips have.
    SourceOffsetOnGeneratedClip {
        track_index: usize,
        clip_index: usize,
    },
    /// MIDI channels use the conventional inclusive range 1..=16.
    InvalidMidiChannel(u8),
}

impl fmt::Display for EditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTempo(tempo_bpm) => write!(
                formatter,
                "invalid tempo: {tempo_bpm} BPM (expected 20..=400)"
            ),
            Self::InvalidMasterGain(gain) => {
                write!(formatter, "invalid master gain: {gain} (expected >= 0)")
            }
            Self::BusIndexOutOfBounds {
                bus_index,
                bus_count,
            } => write!(
                formatter,
                "bus index {bus_index} is out of bounds for {bus_count} buses"
            ),
            Self::TrackRouteCountMismatch {
                route_count,
                track_count,
            } => write!(
                formatter,
                "cannot restore {route_count} track routes into a project with {track_count} tracks"
            ),
            Self::TrackIndexOutOfBounds {
                track_index,
                track_count,
            } => write!(
                formatter,
                "track index {track_index} is out of bounds for {track_count} tracks"
            ),
            Self::ClipIndexOutOfBounds {
                track_index,
                clip_index,
                clip_count,
            } => write!(
                formatter,
                "clip index {clip_index} is out of bounds for {clip_count} clips on track {track_index}"
            ),
            Self::TakeIndexOutOfBounds {
                track_index,
                take_index,
                take_count,
            } => write!(
                formatter,
                "take index {take_index} is out of bounds for {take_count} takes on track {track_index}"
            ),
            Self::ClipIsNotAudio {
                track_index,
                clip_index,
            } => write!(
                formatter,
                "clip {clip_index} on track {track_index} is not file-backed audio"
            ),
            Self::SourceOffsetOnGeneratedClip {
                track_index,
                clip_index,
            } => write!(
                formatter,
                "clip {clip_index} on track {track_index} is generated and has no source offset"
            ),
            Self::InvalidMidiChannel(channel) => {
                write!(
                    formatter,
                    "invalid MIDI channel: {channel} (expected 1..=16)"
                )
            }
        }
    }
}

impl std::error::Error for EditError {}

/// A bounded undo/redo history for project edits.
///
/// Applying a new command clears the redo stack. A limit of zero still
/// applies commands but disables history retention.
#[derive(Debug, Clone)]
pub struct EditHistory {
    limit: usize,
    undo_stack: VecDeque<EditCommand>,
    redo_stack: VecDeque<EditCommand>,
}

impl Default for EditHistory {
    fn default() -> Self {
        Self::new(DEFAULT_EDIT_HISTORY_LIMIT)
    }
}

impl EditHistory {
    /// Creates an empty history that retains at most `limit` edits.
    #[must_use]
    pub const fn new(limit: usize) -> Self {
        Self {
            limit,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
        }
    }

    /// Returns the maximum number of undo entries retained.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Returns the number of edits currently available to undo.
    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    /// Returns the number of edits currently available to redo.
    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    /// Returns whether an edit can currently be undone.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Returns whether an edit can currently be redone.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Removes every undo and redo entry without changing the project.
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Applies one edit and records its inverse.
    ///
    /// Failed edits leave both the project and history unchanged. A successful
    /// new edit always invalidates the redo stack.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when a track or clip index is out of bounds.
    pub fn apply(&mut self, project: &mut Project, command: EditCommand) -> Result<(), EditError> {
        let inverse = command.execute(project)?;
        self.redo_stack.clear();
        self.push_undo(inverse);
        Ok(())
    }

    /// Undoes the most recently applied edit.
    ///
    /// Returns `Ok(false)` when the history is empty. If the project was
    /// changed outside this history and replay fails, the history entry is
    /// retained.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when the saved command's target is no longer
    /// available.
    pub fn undo(&mut self, project: &mut Project) -> Result<bool, EditError> {
        let Some(command) = self.undo_stack.back().cloned() else {
            return Ok(false);
        };
        let inverse = command.execute(project)?;

        self.undo_stack.pop_back();
        self.redo_stack.push_back(inverse);
        Ok(true)
    }

    /// Reapplies the most recently undone edit.
    ///
    /// Returns `Ok(false)` when the redo history is empty. If the project was
    /// changed outside this history and replay fails, the history entry is
    /// retained.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when the saved command's target is no longer
    /// available.
    pub fn redo(&mut self, project: &mut Project) -> Result<bool, EditError> {
        let Some(command) = self.redo_stack.back().cloned() else {
            return Ok(false);
        };
        let inverse = command.execute(project)?;

        self.redo_stack.pop_back();
        self.push_undo(inverse);
        Ok(true)
    }

    fn push_undo(&mut self, command: EditCommand) {
        if self.limit == 0 {
            return;
        }

        if self.undo_stack.len() == self.limit {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClipSource;

    fn project_with_track_and_clip() -> Project {
        let mut project = Project::new("Edit test", 48_000, 120.0).unwrap();
        let mut track = Track::new("Lead");
        track.clips.push(test_clip("Opening", 10));
        project.tracks.push(track);
        project
    }

    fn test_clip(name: &str, start_frame: u64) -> Clip {
        Clip {
            name: name.into(),
            start_frame,
            length_frames: 100,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::Sine {
                frequency_hz: 440.0,
                amplitude: 0.5,
            },
        }
    }

    fn audio_clip(name: &str, path: &str, start_frame: u64) -> Clip {
        Clip {
            name: name.into(),
            start_frame,
            length_frames: 100,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::AudioFile {
                path: path.into(),
                source_offset_frames: 0,
                source_sample_rate: 48_000,
                channels: 2,
            },
        }
    }

    #[test]
    fn alternate_take_add_swap_delete_are_reversible() {
        let mut project = Project::new("Takes", 48_000, 120.0).unwrap();
        let mut track = Track::new("Vocal");
        track.clips.push(audio_clip("Take 2", "take2.wav", 0));
        project.tracks.push(track);
        let alternate = AudioTake::from_clip(&audio_clip("Take 1", "take1.wav", 0)).unwrap();
        let mut history = EditHistory::new(10);

        history
            .apply(
                &mut project,
                EditCommand::AddTakeLane {
                    track_index: 0,
                    take_index: 0,
                    take: alternate,
                },
            )
            .unwrap();
        history
            .apply(
                &mut project,
                EditCommand::SwapClipWithTake {
                    track_index: 0,
                    clip_index: 0,
                    take_index: 0,
                },
            )
            .unwrap();
        assert!(matches!(
            &project.tracks[0].clips[0].source,
            ClipSource::AudioFile { path, .. } if path == "take1.wav"
        ));
        assert_eq!(project.tracks[0].take_lanes[0].path, "take2.wav");

        assert!(history.undo(&mut project).unwrap());
        assert!(matches!(
            &project.tracks[0].clips[0].source,
            ClipSource::AudioFile { path, .. } if path == "take2.wav"
        ));
        assert!(history.undo(&mut project).unwrap());
        assert!(project.tracks[0].take_lanes.is_empty());
        assert!(history.redo(&mut project).unwrap());
        assert!(history.redo(&mut project).unwrap());

        history
            .apply(
                &mut project,
                EditCommand::DeleteTakeLane {
                    track_index: 0,
                    take_index: 0,
                },
            )
            .unwrap();
        assert!(project.tracks[0].take_lanes.is_empty());
        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project.tracks[0].take_lanes.len(), 1);
    }

    #[test]
    fn bus_delete_reroutes_tracks_and_undo_restores_every_route() {
        let mut project = project_with_track_and_clip();
        project.buses.push(Bus::new("Delay"));
        project.buses.push(Bus::new("Drums"));
        project.tracks[0].output = ChannelOutput::Bus(1);
        project.tracks[0].sends = vec![TrackSend::new(0), TrackSend::new(1)];
        let original = project.clone();
        let mut history = EditHistory::new(10);

        history
            .apply(&mut project, EditCommand::DeleteBus { index: 0 })
            .unwrap();

        assert_eq!(project.buses.len(), 1);
        assert_eq!(project.tracks[0].output, ChannelOutput::Bus(0));
        assert_eq!(project.tracks[0].sends.len(), 1);
        assert_eq!(project.tracks[0].sends[0].bus_index, 0);
        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project, original);
        assert!(history.redo(&mut project).unwrap());
        assert_eq!(project.buses.len(), 1);
    }

    #[test]
    fn track_add_and_delete_are_reversible() {
        let mut project = project_with_track_and_clip();
        let original = project.clone();
        let mut history = EditHistory::new(10);

        history
            .apply(
                &mut project,
                EditCommand::AddTrack {
                    index: 0,
                    track: Track::new("Drums"),
                },
            )
            .unwrap();
        assert_eq!(project.tracks[0].name, "Drums");

        history
            .apply(&mut project, EditCommand::DeleteTrack { track_index: 1 })
            .unwrap();
        assert_eq!(project.tracks.len(), 1);

        assert!(history.undo(&mut project).unwrap());
        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project, original);

        assert!(history.redo(&mut project).unwrap());
        assert!(history.redo(&mut project).unwrap());
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].name, "Drums");
    }

    #[test]
    fn tempo_edit_is_reversible() {
        let mut project = project_with_track_and_clip();
        let mut history = EditHistory::new(10);

        history
            .apply(&mut project, EditCommand::SetTempo { tempo_bpm: 138.5 })
            .unwrap();
        assert!((project.tempo_bpm - 138.5).abs() < f64::EPSILON);
        assert!(history.undo(&mut project).unwrap());
        assert!((project.tempo_bpm - 120.0).abs() < f64::EPSILON);
        assert!(history.redo(&mut project).unwrap());
        assert!((project.tempo_bpm - 138.5).abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_tempo_does_not_mutate_history() {
        let mut project = project_with_track_and_clip();
        let mut history = EditHistory::new(10);

        assert_eq!(
            history.apply(&mut project, EditCommand::SetTempo { tempo_bpm: 0.0 }),
            Err(EditError::InvalidTempo(0.0))
        );
        assert!((project.tempo_bpm - 120.0).abs() < f64::EPSILON);
        assert!(!history.can_undo());
    }

    #[test]
    fn master_gain_edit_is_reversible_and_validated() {
        let mut project = project_with_track_and_clip();
        let mut history = EditHistory::new(10);

        history
            .apply(&mut project, EditCommand::SetMasterGain { gain: 0.75 })
            .unwrap();
        assert_eq!(project.master_gain.to_bits(), 0.75_f32.to_bits());
        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project.master_gain.to_bits(), 1.0_f32.to_bits());
        assert_eq!(
            history.apply(&mut project, EditCommand::SetMasterGain { gain: -1.0 }),
            Err(EditError::InvalidMasterGain(-1.0))
        );
    }

    #[test]
    fn mixer_edits_undo_and_redo_in_order() {
        let mut project = project_with_track_and_clip();
        let mut history = EditHistory::new(10);

        history
            .apply(
                &mut project,
                EditCommand::SetTrackGain {
                    track_index: 0,
                    gain: 0.25,
                },
            )
            .unwrap();
        history
            .apply(
                &mut project,
                EditCommand::SetTrackPan {
                    track_index: 0,
                    pan: -0.75,
                },
            )
            .unwrap();
        history
            .apply(
                &mut project,
                EditCommand::SetTrackMuted {
                    track_index: 0,
                    muted: true,
                },
            )
            .unwrap();

        assert_eq!(project.tracks[0].gain.to_bits(), 0.25_f32.to_bits());
        assert_eq!(project.tracks[0].pan.to_bits(), (-0.75_f32).to_bits());
        assert!(project.tracks[0].muted);

        history.undo(&mut project).unwrap();
        history.undo(&mut project).unwrap();
        history.undo(&mut project).unwrap();
        assert_eq!(project.tracks[0].gain.to_bits(), 1.0_f32.to_bits());
        assert_eq!(project.tracks[0].pan.to_bits(), 0.0_f32.to_bits());
        assert!(!project.tracks[0].muted);

        history.redo(&mut project).unwrap();
        history.redo(&mut project).unwrap();
        history.redo(&mut project).unwrap();
        assert_eq!(project.tracks[0].gain.to_bits(), 0.25_f32.to_bits());
        assert_eq!(project.tracks[0].pan.to_bits(), (-0.75_f32).to_bits());
        assert!(project.tracks[0].muted);
    }

    #[test]
    fn record_arm_and_monitoring_are_reversible() {
        let mut project = project_with_track_and_clip();
        let mut history = EditHistory::new(10);

        history
            .apply(
                &mut project,
                EditCommand::SetTrackRecordArmed {
                    track_index: 0,
                    record_armed: true,
                },
            )
            .unwrap();
        history
            .apply(
                &mut project,
                EditCommand::SetTrackInputMonitoring {
                    track_index: 0,
                    input_monitoring: true,
                },
            )
            .unwrap();
        assert!(project.tracks[0].recording.armed);
        assert!(project.tracks[0].recording.input_monitoring);
        history.undo(&mut project).unwrap();
        history.undo(&mut project).unwrap();
        assert!(!project.tracks[0].recording.armed);
        assert!(!project.tracks[0].recording.input_monitoring);
    }

    #[test]
    fn instrument_and_midi_channel_edits_are_reversible() {
        let mut project = project_with_track_and_clip();
        let mut history = EditHistory::new(10);

        history
            .apply(
                &mut project,
                EditCommand::SetTrackInstrument {
                    track_index: 0,
                    instrument: Instrument::Square,
                },
            )
            .unwrap();
        history
            .apply(
                &mut project,
                EditCommand::SetTrackMidiChannel {
                    track_index: 0,
                    midi_channel: 16,
                },
            )
            .unwrap();
        assert_eq!(project.tracks[0].instrument, Instrument::Square);
        assert_eq!(project.tracks[0].midi_channel, 16);

        assert!(matches!(
            history.apply(
                &mut project,
                EditCommand::SetTrackMidiChannel {
                    track_index: 0,
                    midi_channel: 0,
                },
            ),
            Err(EditError::InvalidMidiChannel(0))
        ));
        history.undo(&mut project).unwrap();
        history.undo(&mut project).unwrap();
        assert_eq!(project.tracks[0].instrument, Instrument::Sine);
        assert_eq!(project.tracks[0].midi_channel, 1);
    }

    #[test]
    fn clip_add_delete_and_move_are_reversible() {
        let mut project = project_with_track_and_clip();
        let original = project.clone();
        let mut history = EditHistory::new(10);

        history
            .apply(
                &mut project,
                EditCommand::AddClip {
                    track_index: 0,
                    clip_index: 0,
                    clip: test_clip("Intro", 0),
                },
            )
            .unwrap();
        history
            .apply(
                &mut project,
                EditCommand::MoveClip {
                    track_index: 0,
                    clip_index: 1,
                    start_frame: 2_000,
                },
            )
            .unwrap();
        history
            .apply(
                &mut project,
                EditCommand::DeleteClip {
                    track_index: 0,
                    clip_index: 0,
                },
            )
            .unwrap();

        assert_eq!(project.tracks[0].clips.len(), 1);
        assert_eq!(project.tracks[0].clips[0].start_frame, 2_000);

        history.undo(&mut project).unwrap();
        history.undo(&mut project).unwrap();
        history.undo(&mut project).unwrap();
        assert_eq!(project, original);

        history.redo(&mut project).unwrap();
        history.redo(&mut project).unwrap();
        history.redo(&mut project).unwrap();
        assert_eq!(project.tracks[0].clips.len(), 1);
        assert_eq!(project.tracks[0].clips[0].start_frame, 2_000);
    }

    #[test]
    fn clip_replacement_is_reversible() {
        let mut project = project_with_track_and_clip();
        let original = project.tracks[0].clips[0].clone();
        let replacement = test_clip("MIDI Part", 240);
        let mut history = EditHistory::new(10);

        history
            .apply(
                &mut project,
                EditCommand::ReplaceClip {
                    track_index: 0,
                    clip_index: 0,
                    clip: replacement.clone(),
                },
            )
            .unwrap();
        assert_eq!(project.tracks[0].clips[0], replacement);

        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project.tracks[0].clips[0], original);
        assert!(history.redo(&mut project).unwrap());
        assert_eq!(project.tracks[0].clips[0], replacement);
    }

    #[test]
    fn file_clip_timing_and_source_offset_are_reversible() {
        let mut project = Project::new("Trim test", 48_000, 120.0).unwrap();
        let mut track = Track::new("Audio");
        track.clips.push(Clip {
            name: "Take".into(),
            start_frame: 1_000,
            length_frames: 4_000,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            source: ClipSource::AudioFile {
                path: "take.wav".into(),
                source_offset_frames: 200,
                source_sample_rate: 44_100,
                channels: 2,
            },
        });
        project.tracks.push(track);
        let original = project.clone();
        let mut history = EditHistory::new(10);

        history
            .apply(
                &mut project,
                EditCommand::SetClipTiming {
                    track_index: 0,
                    clip_index: 0,
                    start_frame: 2_000,
                    length_frames: 3_000,
                    source_offset_frames: Some(1_119),
                },
            )
            .unwrap();

        let clip = &project.tracks[0].clips[0];
        assert_eq!(clip.start_frame, 2_000);
        assert_eq!(clip.length_frames, 3_000);
        assert!(matches!(
            clip.source,
            ClipSource::AudioFile {
                source_offset_frames: 1_119,
                ..
            }
        ));

        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project, original);
        assert!(history.redo(&mut project).unwrap());
        assert_eq!(project.tracks[0].clips[0].start_frame, 2_000);
        assert!(matches!(
            project.tracks[0].clips[0].source,
            ClipSource::AudioFile {
                source_offset_frames: 1_119,
                ..
            }
        ));
    }

    #[test]
    fn generated_clip_rejects_file_source_offset_without_mutation() {
        let mut project = project_with_track_and_clip();
        let original = project.clone();
        let mut history = EditHistory::new(10);

        assert_eq!(
            history.apply(
                &mut project,
                EditCommand::SetClipTiming {
                    track_index: 0,
                    clip_index: 0,
                    start_frame: 20,
                    length_frames: 80,
                    source_offset_frames: Some(4),
                },
            ),
            Err(EditError::SourceOffsetOnGeneratedClip {
                track_index: 0,
                clip_index: 0,
            })
        );
        assert_eq!(project, original);
        assert!(!history.can_undo());
    }

    #[test]
    fn invalid_indices_do_not_mutate_project_or_history() {
        let mut project = project_with_track_and_clip();
        let original = project.clone();
        let mut history = EditHistory::new(10);

        assert_eq!(
            history.apply(
                &mut project,
                EditCommand::SetTrackGain {
                    track_index: 9,
                    gain: 0.5,
                },
            ),
            Err(EditError::TrackIndexOutOfBounds {
                track_index: 9,
                track_count: 1,
            })
        );
        assert_eq!(
            history.apply(
                &mut project,
                EditCommand::MoveClip {
                    track_index: 0,
                    clip_index: 4,
                    start_frame: 0,
                },
            ),
            Err(EditError::ClipIndexOutOfBounds {
                track_index: 0,
                clip_index: 4,
                clip_count: 1,
            })
        );
        assert_eq!(
            history.apply(
                &mut project,
                EditCommand::AddClip {
                    track_index: 0,
                    clip_index: 2,
                    clip: test_clip("Invalid", 0),
                },
            ),
            Err(EditError::ClipIndexOutOfBounds {
                track_index: 0,
                clip_index: 2,
                clip_count: 1,
            })
        );

        assert_eq!(project, original);
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn history_limit_evicts_the_oldest_edits() {
        let mut project = project_with_track_and_clip();
        let mut history = EditHistory::new(2);

        for gain in [0.8, 0.6, 0.4] {
            history
                .apply(
                    &mut project,
                    EditCommand::SetTrackGain {
                        track_index: 0,
                        gain,
                    },
                )
                .unwrap();
        }

        assert_eq!(history.undo_len(), 2);
        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project.tracks[0].gain.to_bits(), 0.6_f32.to_bits());
        assert!(history.undo(&mut project).unwrap());
        assert_eq!(project.tracks[0].gain.to_bits(), 0.8_f32.to_bits());
        assert!(!history.undo(&mut project).unwrap());
    }

    #[test]
    fn new_edit_clears_redo_and_zero_limit_disables_history() {
        let mut project = project_with_track_and_clip();
        let mut history = EditHistory::new(1);

        history
            .apply(
                &mut project,
                EditCommand::SetTrackMuted {
                    track_index: 0,
                    muted: true,
                },
            )
            .unwrap();
        history.undo(&mut project).unwrap();
        assert!(history.can_redo());

        history
            .apply(
                &mut project,
                EditCommand::SetTrackPan {
                    track_index: 0,
                    pan: 0.5,
                },
            )
            .unwrap();
        assert!(!history.can_redo());

        let mut unrecorded = EditHistory::new(0);
        unrecorded
            .apply(
                &mut project,
                EditCommand::SetTrackGain {
                    track_index: 0,
                    gain: 0.1,
                },
            )
            .unwrap();
        assert_eq!(project.tracks[0].gain.to_bits(), 0.1_f32.to_bits());
        assert!(!unrecorded.can_undo());
    }
}
