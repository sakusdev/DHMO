//! Core timeline and rendering primitives for DMO.

mod audio_file;
mod consolidate;
mod edit;
mod frames;
mod midi_edit;
mod midi_file;
mod pitch;
mod project;
mod project_file;
mod render;
mod soundfont;
mod transport;
mod wav;

pub use audio_file::{
    AudioFileError, AudioFileInfo, AudioSampleFormat, DEFAULT_WAVEFORM_PEAKS, DecodedAudio,
    ImportedAudioFile, WaveformOverview, WaveformPeak, convert_frame_count, decode_wav,
    decode_wav_overview, import_wav, import_wav_with_overview, probe_wav,
};
pub use consolidate::{ConsolidateError, ConsolidatedProject, consolidate_project_media};
pub use edit::{DEFAULT_EDIT_HISTORY_LIMIT, EditCommand, EditError, EditHistory};
pub use frames::rescale_frames_round;
pub use midi_edit::{
    GrooveQuantizeOptions, GrooveTemplate, MidiScale, QuantizeOptions, SwingQuantizeOptions,
    adjust_note_velocities, conform_notes_to_scale, groove_quantize_notes, humanize_notes,
    make_notes_legato, quantize_notes, remove_note_overlaps, scale_note_velocities,
    set_note_lengths, swing_quantize_notes, transpose_notes,
};
pub use midi_file::{
    ImportedMidi, ImportedMidiTrack, MIDI_TICKS_PER_QUARTER, MidiFileError, decode_midi,
    encode_project_midi, load_midi, save_project_midi,
};
pub use pitch::{A4_MIDI_NOTE, frequency_to_midi_note, midi_note_frequency, midi_note_name};
pub use project::{
    ArrangerSection, AudioTake, AutomationPoint, Bus, ChannelInsert, ChannelOutput, Clip,
    ClipSource, CycleRange, FadeCurve, InsertEffect, Instrument, Marker, MidiChannelPressurePoint,
    MidiControlPoint, MidiNote, MidiPitchBendPoint, MidiPolyPressurePoint, MidiProgramChangePoint,
    Project, ProjectError, SoundFontPreset, TimeSignature, Track, TrackInput, TrackRecording,
    TrackSend, automation_value_at, midi_controller_value_at,
};
pub use project_file::{
    PROJECT_FILE_VERSION, ProjectFileError, decode_project, encode_project, load_project,
    save_project,
};
pub use render::{
    RenderError, RenderedStem, render_stereo, try_render_stereo, try_render_track_stems,
};
pub use soundfont::{SoundFontError, SoundFontInfo, inspect_soundfont};
pub use transport::{LoopRange, Transport, TransportError, TransportState};
pub use wav::{WavError, write_stereo_f32_wav, write_stereo_i16_wav, write_stereo_i24_wav};
