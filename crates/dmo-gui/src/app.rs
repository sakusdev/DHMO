//! Main DMO desktop application.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use dmo_audio::{
    AudioInputDevice, Playback, Recording as AudioRecording, available_input_devices,
    default_output_device_info,
};
use dmo_core::{
    ArrangerSection, AudioTake, AutomationPoint, Bus, ChannelInsert, ChannelOutput, Clip,
    ClipSource, CycleRange, EditCommand, EditHistory, FadeCurve, GrooveQuantizeOptions,
    GrooveTemplate, InsertEffect, Instrument, Marker, MidiChannelPressurePoint, MidiControlPoint,
    MidiNote, MidiPitchBendPoint, MidiPolyPressurePoint, MidiProgramChangePoint, MidiScale,
    Project, QuantizeOptions, SoundFontPreset, SwingQuantizeOptions, TimeSignature, Track,
    TrackInput, TrackSend, WaveformOverview, adjust_note_velocities, conform_notes_to_scale,
    consolidate_project_media, decode_wav, decode_wav_overview, frequency_to_midi_note,
    groove_quantize_notes, humanize_notes, import_wav_with_overview, inspect_soundfont, load_midi,
    load_project, make_notes_legato, midi_note_frequency, midi_note_name, quantize_notes,
    remove_note_overlaps, rescale_frames_round, save_project, save_project_midi, set_note_lengths,
    swing_quantize_notes, transpose_notes, try_render_stereo, try_render_track_stems,
    write_stereo_f32_wav, write_stereo_i16_wav, write_stereo_i24_wav,
};
use eframe::egui::{self, Color32, RichText};

use crate::{
    midi_input::{LiveMidiMessage, MidiInputManager},
    midi_output::MidiOutputManager,
    piano_roll::{
        DeleteNoteRequest, MoveNoteRequest, PianoRollView, ResizeNoteRequest, WriteCcRequest,
    },
    timeline::{RULER_HEIGHT, TRACK_HEIGHT, TimelineView, track_color},
};

const WAVEFORM_PEAKS: usize = 2_048;
const RETROSPECTIVE_MIDI_SECONDS: u64 = 30;
const ARRANGER_COLORS: [Color32; 6] = [
    Color32::from_rgb(79, 121, 220),
    Color32::from_rgb(121, 91, 206),
    Color32::from_rgb(219, 148, 70),
    Color32::from_rgb(74, 167, 120),
    Color32::from_rgb(202, 91, 108),
    Color32::from_rgb(80, 154, 181),
];

pub struct DmoApp {
    project: Project,
    project_path: Option<PathBuf>,
    history: EditHistory,
    selected_track: Option<usize>,
    selected_clip: Option<(usize, usize)>,
    selected_note: Option<(usize, usize, usize)>,
    clip_clipboard: Option<Clip>,
    playhead_frame: u64,
    pixels_per_second: f32,
    playback: Option<Playback>,
    recording: Option<ActiveRecording>,
    recording_options: RecordingOptions,
    audio_input_devices: Vec<AudioInputDevice>,
    selected_audio_input_device: Option<String>,
    midi_tools: MidiToolsState,
    midi_input: MidiInputManager,
    midi_ports: Vec<String>,
    selected_midi_port: Option<usize>,
    midi_output: MidiOutputManager,
    midi_output_ports: Vec<String>,
    selected_midi_output_port: Option<usize>,
    midi_output_playback: MidiOutputPlayback,
    last_midi_activity: Option<Instant>,
    retrospective_midi: VecDeque<RetrospectiveMidiEvent>,
    metronome_enabled: bool,
    loop_enabled: bool,
    loop_start_frame: u64,
    loop_end_frame: u64,
    waveforms: HashMap<String, CachedWaveform>,
    waveform_errors: HashMap<String, String>,
    waveform_fingerprints: HashMap<String, AudioFileFingerprint>,
    soundfont_presets: HashMap<String, Vec<SoundFontPreset>>,
    piano_roll: PianoRollState,
    arrange: ArrangeState,
    view: AppView,
    dirty: bool,
    status: StatusMessage,
}

struct ActiveRecording {
    capture: Option<AudioRecording>,
    audio_takes: Vec<ActiveAudioTake>,
    midi_takes: Vec<ActiveMidiTake>,
    started_at: Instant,
    capture_start_frame: u64,
    record_start_frame: u64,
    end_frame: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct ActiveAudioTake {
    track_index: usize,
    input: TrackInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppView {
    Start,
    Editor,
}

#[derive(Debug, Clone)]
struct CachedWaveform {
    overview: WaveformOverview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AudioFileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

struct ActiveMidiTake {
    track_index: usize,
    input: TrackInput,
    active_notes: HashMap<(u8, u8), (u64, u8)>,
    notes: Vec<RecordedMidiNote>,
    controllers: Vec<MidiControlPoint>,
    pitch_bend: Vec<MidiPitchBendPoint>,
    channel_pressure: Vec<MidiChannelPressurePoint>,
    poly_pressure: Vec<MidiPolyPressurePoint>,
    program_changes: Vec<MidiProgramChangePoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RetrospectiveMidiEvent {
    received_at: Instant,
    event: ParsedLiveMidi,
}

#[derive(Debug, Clone)]
struct MidiOutputPlayback {
    last_frame: u64,
    active_notes: Vec<(u8, u8)>,
    initialized: bool,
}

impl MidiOutputPlayback {
    const fn new() -> Self {
        Self {
            last_frame: 0,
            active_notes: Vec::new(),
            initialized: false,
        }
    }

    fn reset_at(&mut self, frame: u64) {
        self.last_frame = frame;
        self.active_notes.clear();
        self.initialized = false;
    }
}

#[derive(Debug, Clone, Copy)]
struct RecordedMidiNote {
    start_frame: u64,
    length_frames: u64,
    midi_note: u8,
    velocity: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedLiveMidi {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    Control {
        channel: u8,
        controller: u8,
        value: u8,
    },
    PitchBend {
        channel: u8,
        value: i16,
    },
    ChannelPressure {
        channel: u8,
        value: u8,
    },
    PolyPressure {
        channel: u8,
        note: u8,
        value: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MidiPlaybackEvent {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    Control {
        channel: u8,
        controller: u8,
        value: u8,
    },
    PitchBend {
        channel: u8,
        value: i16,
    },
    ChannelPressure {
        channel: u8,
        value: u8,
    },
    PolyPressure {
        channel: u8,
        note: u8,
        value: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
}

#[derive(Debug, Clone, Copy)]
struct RecordingOptions {
    punch_enabled: bool,
    cycle_take_recording: bool,
    pre_roll_bars: u8,
}

impl Default for RecordingOptions {
    fn default() -> Self {
        Self {
            punch_enabled: false,
            cycle_take_recording: false,
            pre_roll_bars: 1,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MidiToolsState {
    quantize_strength: u8,
    quantize_ends: bool,
    swing_percent: u8,
    groove_template: GrooveTemplate,
    groove_amount: u8,
    scale_root: u8,
    scale: MidiScale,
    note_length_grid: SnapGrid,
    humanize_timing_ms: f32,
    humanize_velocity: u8,
    humanize_seed: u64,
    cc_controller: u8,
    cc_value: u8,
    pitch_bend: i16,
    channel_pressure: u8,
    program_change: u8,
}

impl Default for MidiToolsState {
    fn default() -> Self {
        Self {
            quantize_strength: 100,
            quantize_ends: false,
            swing_percent: 60,
            groove_template: GrooveTemplate::Mpc16,
            groove_amount: 60,
            scale_root: 0,
            scale: MidiScale::Major,
            note_length_grid: SnapGrid::Sixteenth,
            humanize_timing_ms: 8.0,
            humanize_velocity: 6,
            humanize_seed: 1,
            cc_controller: 11,
            cc_value: 127,
            pitch_bend: 0,
            channel_pressure: 0,
            program_change: 1,
        }
    }
}

#[derive(Debug, Clone)]
struct StatusMessage {
    text: String,
    error: bool,
}

#[derive(Debug, Clone, Copy)]
struct PianoRollState {
    open: bool,
    scroll_to_selection: bool,
}

#[derive(Debug, Clone, Copy)]
struct ArrangeState {
    tool: EditTool,
    snap_enabled: bool,
    snap_grid: SnapGrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditTool {
    Select,
    Draw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapGrid {
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
}

impl SnapGrid {
    const ALL: [Self; 4] = [
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
        }
    }

    const fn divisions_per_beat(self) -> u8 {
        match self {
            Self::Quarter => 1,
            Self::Eighth => 2,
            Self::Sixteenth => 4,
            Self::ThirtySecond => 8,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum UiAction {
    New,
    Open,
    Save,
    SaveAs,
    ExportMixdown(ExportAudioFormat),
    ExportStems,
    ConsolidateProject,
    ImportWav,
    ImportMidi,
    ExportMidi,
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy)]
enum ExportAudioFormat {
    Wav16,
    Wav24,
    WavFloat32,
}

impl ExportAudioFormat {
    const ALL: [Self; 3] = [Self::Wav16, Self::Wav24, Self::WavFloat32];

    const fn label(self) -> &'static str {
        match self {
            Self::Wav16 => "WAV 16-bit PCM",
            Self::Wav24 => "WAV 24-bit PCM",
            Self::WavFloat32 => "WAV 32-bit float",
        }
    }
}

impl DmoApp {
    pub fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        configure_theme(&creation_context.egui_ctx);
        let sample_rate = default_output_device_info().map_or(48_000, |info| info.sample_rate);
        let requested_project = std::env::args_os().nth(1).map(PathBuf::from);
        let view = if requested_project.is_none() {
            AppView::Start
        } else {
            AppView::Editor
        };
        let (project, project_path, status) = requested_project.map_or_else(
            || {
                (
                    demo_project(sample_rate),
                    None,
                    StatusMessage {
                        text: "Ready — Space toggles playback".into(),
                        error: false,
                    },
                )
            },
            |path| match load_project(&path) {
                Ok(project) => (
                    project,
                    Some(path.clone()),
                    StatusMessage {
                        text: format!("Opened {}", path.display()),
                        error: false,
                    },
                ),
                Err(error) => (
                    demo_project(sample_rate),
                    None,
                    StatusMessage {
                        text: format!("Could not open {}: {error}", path.display()),
                        error: true,
                    },
                ),
            },
        );
        let default_loop_end_frame = project.duration_frames().max(1);
        let loop_start_frame = project.cycle_range.map_or(0, |range| range.start_frame);
        let loop_end_frame = project
            .cycle_range
            .map_or(default_loop_end_frame, |range| range.end_frame);
        let loop_enabled = project.cycle_range.is_some();
        let selected_clip = first_clip_selection(&project);
        let selected_track = selected_clip
            .map(|(track_index, _)| track_index)
            .or_else(|| (!project.tracks.is_empty()).then_some(0));
        let selected_note = first_note_selection(&project, selected_clip);
        let audio_input_devices = available_input_devices().unwrap_or_default();
        let midi_ports = MidiInputManager::ports().unwrap_or_default();
        let midi_output_ports = MidiOutputManager::ports().unwrap_or_default();
        let mut app = Self {
            project,
            project_path,
            history: EditHistory::default(),
            selected_track,
            selected_clip,
            selected_note,
            clip_clipboard: None,
            playhead_frame: 0,
            pixels_per_second: 120.0,
            playback: None,
            recording: None,
            recording_options: RecordingOptions::default(),
            audio_input_devices,
            selected_audio_input_device: None,
            midi_tools: MidiToolsState::default(),
            midi_input: MidiInputManager::new(),
            midi_ports,
            selected_midi_port: None,
            midi_output: MidiOutputManager::new(),
            midi_output_ports,
            selected_midi_output_port: None,
            midi_output_playback: MidiOutputPlayback::new(),
            last_midi_activity: None,
            retrospective_midi: VecDeque::new(),
            metronome_enabled: false,
            loop_enabled,
            loop_start_frame,
            loop_end_frame,
            waveforms: HashMap::new(),
            waveform_errors: HashMap::new(),
            waveform_fingerprints: HashMap::new(),
            soundfont_presets: HashMap::new(),
            piano_roll: PianoRollState {
                open: true,
                scroll_to_selection: true,
            },
            arrange: ArrangeState {
                tool: EditTool::Select,
                snap_enabled: true,
                snap_grid: SnapGrid::Sixteenth,
            },
            view,
            dirty: false,
            status,
        };
        app.repair_edit_state();
        app.refresh_waveforms();
        app
    }

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        let mut create_project = false;
        let mut open_project = false;
        let mut open_demo = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_rgb(20, 23, 30)))
            .show(ui, |ui| {
                ui.add_space((ui.available_height() * 0.17).max(24.0));
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("DMO")
                            .size(44.0)
                            .strong()
                            .color(Color32::from_rgb(116, 151, 255)),
                    );
                    ui.label(
                        RichText::new("Digital Music Observatory")
                            .size(15.0)
                            .color(Color32::from_gray(180)),
                    );
                    ui.add_space(28.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(30, 34, 43))
                        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(58, 65, 80)))
                        .corner_radius(10.0)
                        .inner_margin(egui::Margin::symmetric(26, 22))
                        .show(ui, |ui| {
                            ui.set_width(330.0);
                            ui.heading("Start creating");
                            ui.add_space(8.0);
                            if ui
                                .add_sized([278.0, 38.0], egui::Button::new("New project"))
                                .clicked()
                            {
                                create_project = true;
                            }
                            if ui
                                .add_sized([278.0, 34.0], egui::Button::new("Open project…"))
                                .clicked()
                            {
                                open_project = true;
                            }
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(6.0);
                            if ui
                                .add_sized([278.0, 30.0], egui::Button::new("Explore demo session"))
                                .clicked()
                            {
                                open_demo = true;
                            }
                        });
                    ui.add_space(20.0);
                    ui.small("SF2 instruments · MIDI editing · audio recording · stem export");
                    ui.small("Open source DAW · Rust edition");
                });
            });
        if create_project {
            self.new_project();
        } else if open_project {
            self.open_project_dialog();
        } else if open_demo {
            let sample_rate = default_output_device_info().map_or(48_000, |info| info.sample_rate);
            self.replace_project(demo_project(sample_rate), None, false);
            self.view = AppView::Editor;
            self.status_ok("Opened the demo session");
        }
    }

    #[allow(clippy::too_many_lines)]
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let mut audio_input_choice = None;
        let mut refresh_audio_inputs = false;
        let mut midi_choice = None;
        let mut midi_output_choice = None;
        let mut refresh_midi = false;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                menu_action(ui, "New project", &mut action, UiAction::New);
                menu_action(ui, "Open…", &mut action, UiAction::Open);
                menu_action(ui, "Import WAV…", &mut action, UiAction::ImportWav);
                menu_action(ui, "Import MIDI…", &mut action, UiAction::ImportMidi);
                ui.separator();
                menu_action(ui, "Save", &mut action, UiAction::Save);
                menu_action(ui, "Save as…", &mut action, UiAction::SaveAs);
                ui.separator();
                ui.menu_button("Export mixdown", |ui| {
                    for format in ExportAudioFormat::ALL {
                        menu_action(
                            ui,
                            format.label(),
                            &mut action,
                            UiAction::ExportMixdown(format),
                        );
                    }
                });
                menu_action(
                    ui,
                    "Export stems (WAV 16-bit)…",
                    &mut action,
                    UiAction::ExportStems,
                );
                menu_action(ui, "Export MIDI…", &mut action, UiAction::ExportMidi);
                ui.separator();
                menu_action(
                    ui,
                    "Consolidate Project…",
                    &mut action,
                    UiAction::ConsolidateProject,
                );
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(self.history.can_undo(), egui::Button::new("Undo"))
                    .clicked()
                {
                    action = Some(UiAction::Undo);
                    ui.close();
                }
                if ui
                    .add_enabled(self.history.can_redo(), egui::Button::new("Redo"))
                    .clicked()
                {
                    action = Some(UiAction::Redo);
                    ui.close();
                }
                ui.separator();
                let has_clip = self.selected_clip.is_some();
                if ui
                    .add_enabled(has_clip, egui::Button::new("Copy clip    Ctrl+C"))
                    .clicked()
                {
                    self.copy_selected_clip();
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.clip_clipboard.is_some() && self.selected_track.is_some(),
                        egui::Button::new("Paste clip    Ctrl+V"),
                    )
                    .clicked()
                {
                    self.paste_clip();
                    ui.close();
                }
                if ui
                    .add_enabled(has_clip, egui::Button::new("Duplicate clip    Ctrl+D"))
                    .clicked()
                {
                    self.duplicate_selected_clip();
                    ui.close();
                }
                if ui
                    .add_enabled(has_clip, egui::Button::new("Split at playhead    Ctrl+B"))
                    .clicked()
                {
                    self.split_selected_clip();
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                if ui
                    .checkbox(&mut self.piano_roll.open, "Piano Roll")
                    .changed()
                    && self.piano_roll.open
                {
                    self.piano_roll.scroll_to_selection = true;
                }
            });
            ui.menu_button("Audio", |ui| {
                let selected_name = self
                    .selected_audio_input_device
                    .as_deref()
                    .and_then(|selected| {
                        self.audio_input_devices
                            .iter()
                            .find(|device| device.id == selected)
                    })
                    .map_or("System default", |device| device.name.as_str());
                ui.label(RichText::new("Recording input").color(Color32::from_rgb(151, 157, 169)));
                ui.label(selected_name);
                ui.separator();
                if ui
                    .selectable_label(self.selected_audio_input_device.is_none(), "System default")
                    .clicked()
                {
                    audio_input_choice = Some(None);
                    ui.close();
                }
                for device in &self.audio_input_devices {
                    let label = if device.is_default {
                        format!("{} (default)", device.name)
                    } else {
                        device.name.clone()
                    };
                    if ui
                        .selectable_label(
                            self.selected_audio_input_device.as_deref() == Some(device.id.as_str()),
                            label,
                        )
                        .clicked()
                    {
                        audio_input_choice = Some(Some(device.id.clone()));
                        ui.close();
                    }
                }
                if self.audio_input_devices.is_empty() {
                    ui.label("No audio input devices found");
                }
                ui.separator();
                if ui.button("Refresh devices").clicked() {
                    refresh_audio_inputs = true;
                }
            });
            ui.menu_button("MIDI", |ui| {
                ui.label(RichText::new("Input").color(Color32::from_rgb(151, 157, 169)));
                ui.label(
                    self.midi_input
                        .connected_name()
                        .map_or("Input: disconnected", |name| name),
                );
                if self.midi_input.connected_name().is_some()
                    && ui.button("Disconnect input").clicked()
                {
                    midi_choice = Some(None);
                    ui.close();
                }
                ui.separator();
                if self.midi_ports.is_empty() {
                    ui.label("No MIDI input ports found");
                }
                for (index, name) in self.midi_ports.iter().enumerate() {
                    if ui
                        .selectable_label(self.selected_midi_port == Some(index), name)
                        .clicked()
                    {
                        midi_choice = Some(Some(index));
                        ui.close();
                    }
                }
                ui.separator();
                ui.label(RichText::new("Output").color(Color32::from_rgb(151, 157, 169)));
                ui.label(
                    self.midi_output
                        .connected_name()
                        .map_or("Output: disconnected", |name| name),
                );
                if self.midi_output.connected_name().is_some()
                    && ui.button("Disconnect output").clicked()
                {
                    midi_output_choice = Some(None);
                    ui.close();
                }
                ui.separator();
                if self.midi_output_ports.is_empty() {
                    ui.label("No MIDI output ports found");
                }
                for (index, name) in self.midi_output_ports.iter().enumerate() {
                    if ui
                        .selectable_label(self.selected_midi_output_port == Some(index), name)
                        .clicked()
                    {
                        midi_output_choice = Some(Some(index));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Refresh devices").clicked() {
                    refresh_midi = true;
                }
            });

            ui.separator();
            let path = self
                .project_path
                .as_deref()
                .and_then(Path::file_name)
                .map_or_else(
                    || format!("{}.dmo", self.project.name),
                    |name| name.to_string_lossy().into_owned(),
                );
            let marker = if self.dirty { " •" } else { "" };
            ui.label(RichText::new(format!("{path}{marker}")).color(Color32::LIGHT_GRAY));
        });

        if let Some(action) = action {
            self.perform_action(action);
        }
        if refresh_audio_inputs {
            match available_input_devices() {
                Ok(devices) => {
                    if self
                        .selected_audio_input_device
                        .as_ref()
                        .is_some_and(|selected| {
                            !devices.iter().any(|device| &device.id == selected)
                        })
                    {
                        self.selected_audio_input_device = None;
                    }
                    self.audio_input_devices = devices;
                    self.status_ok(format!(
                        "Found {} audio input(s)",
                        self.audio_input_devices.len()
                    ));
                }
                Err(error) => self.status_error(error.to_string()),
            }
        }
        if let Some(choice) = audio_input_choice {
            self.selected_audio_input_device = choice;
            let name = self
                .selected_audio_input_device
                .as_deref()
                .and_then(|selected| {
                    self.audio_input_devices
                        .iter()
                        .find(|device| device.id == selected)
                })
                .map_or_else(|| "System default".to_owned(), |device| device.name.clone());
            self.status_ok(format!("Audio recording input: {name}"));
        }
        if refresh_midi {
            let mut errors = Vec::new();
            match MidiInputManager::ports() {
                Ok(ports) => self.midi_ports = ports,
                Err(error) => errors.push(error),
            }
            match MidiOutputManager::ports() {
                Ok(ports) => self.midi_output_ports = ports,
                Err(error) => errors.push(error),
            }
            if errors.is_empty() {
                self.status_ok(format!(
                    "Found {} MIDI input(s), {} output(s)",
                    self.midi_ports.len(),
                    self.midi_output_ports.len()
                ));
            } else {
                self.status_error(errors.join("; "));
            }
        }
        if let Some(choice) = midi_choice {
            if let Some(index) = choice {
                match self.midi_input.connect(index) {
                    Ok(name) => {
                        self.selected_midi_port = Some(index);
                        self.status_ok(format!("MIDI input connected: {name}"));
                    }
                    Err(error) => self.status_error(error),
                }
            } else {
                self.midi_input.disconnect();
                self.selected_midi_port = None;
                self.status_ok("MIDI input disconnected");
            }
        }
        if let Some(choice) = midi_output_choice {
            self.all_midi_notes_off();
            if let Some(index) = choice {
                match self.midi_output.connect(index) {
                    Ok(name) => {
                        self.selected_midi_output_port = Some(index);
                        self.midi_output_playback.reset_at(self.playhead_frame);
                        self.status_ok(format!("MIDI output connected: {name}"));
                    }
                    Err(error) => self.status_error(error),
                }
            } else {
                self.midi_output.disconnect();
                self.selected_midi_output_port = None;
                self.midi_output_playback.reset_at(self.playhead_frame);
                self.status_ok("MIDI output disconnected");
            }
        }
    }

    fn arrange_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("TOOLS")
                    .monospace()
                    .size(10.0)
                    .color(Color32::from_rgb(151, 157, 169)),
            );
            ui.selectable_value(&mut self.arrange.tool, EditTool::Select, "Select")
                .on_hover_text("Select and move events (1)");
            ui.selectable_value(&mut self.arrange.tool, EditTool::Draw, "Draw")
                .on_hover_text("Draw note events (2)");
            ui.separator();

            ui.toggle_value(&mut self.arrange.snap_enabled, "Snap");
            ui.add_enabled_ui(self.arrange.snap_enabled, |ui| {
                ui.label("Grid");
                egui::ComboBox::from_id_salt("arrange_snap_grid")
                    .selected_text(self.arrange.snap_grid.label())
                    .width(56.0)
                    .show_ui(ui, |ui| {
                        for grid in SnapGrid::ALL {
                            ui.selectable_value(&mut self.arrange.snap_grid, grid, grid.label());
                        }
                    });
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let editor_response = ui.toggle_value(&mut self.piano_roll.open, "Editor");
                if editor_response.changed() && self.piano_roll.open {
                    self.piano_roll.scroll_to_selection = true;
                }
                ui.separator();
                ui.add(
                    egui::Slider::new(&mut self.pixels_per_second, 40.0..=420.0)
                        .logarithmic(true)
                        .show_value(false),
                );
                ui.label("Zoom");
            });
        });
    }

    #[allow(clippy::too_many_lines)]
    fn transport_bar(&mut self, ui: &mut egui::Ui) {
        let mut tempo_change = None;
        let mut signature_change = None;
        ui.horizontal(|ui| {
            let status_color = if self.status.error {
                Color32::from_rgb(255, 125, 125)
            } else {
                Color32::from_rgb(150, 190, 170)
            };
            ui.add_sized(
                [230.0, 24.0],
                egui::Label::new(RichText::new(&self.status.text).color(status_color)).truncate(),
            )
            .on_hover_text(&self.status.text);
            ui.separator();

            if ui.button("|<").on_hover_text("Return to start").clicked() {
                self.seek(0);
            }
            if ui.button("Stop").clicked() {
                self.stop();
            }
            if self.is_playing() {
                if ui.button("Pause").clicked() {
                    self.pause();
                }
            } else if ui.button("Play").clicked() {
                self.play();
            }
            let is_recording = self.recording.is_some();
            let record_label = if is_recording {
                RichText::new("■ Finish").color(Color32::from_rgb(255, 170, 150))
            } else {
                RichText::new("● Rec").color(Color32::from_rgb(255, 105, 105))
            };
            if ui.button(record_label).clicked() {
                if is_recording {
                    self.finish_recording();
                } else {
                    self.start_recording();
                }
            }
            if ui
                .add_enabled(
                    !is_recording && !self.retrospective_midi.is_empty(),
                    egui::Button::new("Capture MIDI"),
                )
                .on_hover_text("Create a MIDI clip from the last 30 seconds of live input")
                .clicked()
            {
                self.capture_retrospective_midi();
            }
            let loop_response = ui.toggle_value(&mut self.loop_enabled, "↻ Loop");
            if loop_response.changed() {
                self.write_cycle_range_from_transport();
                self.sync_playback_loop();
            }
            if ui
                .toggle_value(&mut self.metronome_enabled, "♩ Metro")
                .changed()
            {
                self.invalidate_playback();
                self.status_ok(if self.metronome_enabled {
                    "Metronome on"
                } else {
                    "Metronome off"
                });
            }
            if ui
                .small_button("L<")
                .on_hover_text("Set loop start to playhead")
                .clicked()
            {
                let start = self
                    .playhead_frame
                    .min(self.loop_end_frame.saturating_sub(1));
                self.set_cycle_range(start, self.loop_end_frame);
                self.loop_enabled = true;
                self.status_ok("Loop start set");
            }
            if ui
                .small_button(">R")
                .on_hover_text("Set loop end to playhead")
                .clicked()
            {
                let end = self
                    .playhead_frame
                    .max(self.loop_start_frame.saturating_add(1));
                self.set_cycle_range(self.loop_start_frame, end);
                self.loop_enabled = true;
                self.status_ok("Loop end set");
            }
            if ui
                .small_button("|<M")
                .on_hover_text("Previous marker")
                .clicked()
            {
                self.seek_previous_marker();
            }
            if ui
                .small_button("M+")
                .on_hover_text("Add marker at playhead")
                .clicked()
            {
                self.add_marker_at_playhead();
            }
            if ui
                .small_button("M>|")
                .on_hover_text("Next marker")
                .clicked()
            {
                self.seek_next_marker();
            }
            ui.add_enabled_ui(self.loop_enabled && self.recording.is_none(), |ui| {
                ui.toggle_value(&mut self.recording_options.punch_enabled, "Punch")
                    .on_hover_text("Record only inside the L/R loop range");
                ui.toggle_value(
                    &mut self.recording_options.cycle_take_recording,
                    "Cycle Takes",
                )
                .on_hover_text("Record repeated L/R passes as alternate take lanes until stopped");
            });
            ui.add_enabled_ui(self.recording.is_none(), |ui| {
                egui::ComboBox::from_id_salt("pre_roll_bars")
                    .width(62.0)
                    .selected_text(if self.recording_options.pre_roll_bars == 0 {
                        "Pre off".into()
                    } else {
                        format!("Pre {}b", self.recording_options.pre_roll_bars)
                    })
                    .show_ui(ui, |ui| {
                        for bars in [0, 1, 2, 4] {
                            let label = if bars == 0 {
                                "Off".into()
                            } else if bars == 1 {
                                "1 bar".into()
                            } else {
                                format!("{bars} bars")
                            };
                            ui.selectable_value(
                                &mut self.recording_options.pre_roll_bars,
                                bars,
                                label,
                            );
                        }
                    })
                    .response
                    .on_hover_text("Playback/count-in before the recording start");
            });
            ui.separator();

            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format_musical_position(
                        self.playhead_frame,
                        self.project.sample_rate,
                        self.project.tempo_bpm,
                        self.project.time_signature,
                    ))
                    .monospace()
                    .size(17.0)
                    .color(Color32::from_rgb(224, 231, 241)),
                );
                ui.label(
                    RichText::new(format_time(self.playhead_frame, self.project.sample_rate))
                        .monospace()
                        .size(10.0)
                        .color(Color32::from_rgb(147, 155, 170)),
                );
            });
            ui.separator();
            ui.vertical(|ui| {
                let mut tempo = self.project.tempo_bpm;
                if ui
                    .add_sized(
                        [68.0, 18.0],
                        egui::DragValue::new(&mut tempo)
                            .range(20.0..=400.0)
                            .speed(0.1)
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    tempo_change = Some(tempo);
                }
                ui.small("TEMPO");
            });
            ui.vertical(|ui| {
                let mut numerator = self.project.time_signature.numerator;
                let mut denominator = self.project.time_signature.denominator;
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut numerator).range(1..=32).speed(1))
                        .changed()
                    {
                        signature_change = TimeSignature::new(numerator, denominator);
                    }
                    ui.label("/");
                    egui::ComboBox::from_id_salt("time_signature_denominator")
                        .selected_text(denominator.to_string())
                        .width(44.0)
                        .show_ui(ui, |ui| {
                            for candidate in TimeSignature::COMMON_DENOMINATORS {
                                if ui
                                    .selectable_value(
                                        &mut denominator,
                                        candidate,
                                        candidate.to_string(),
                                    )
                                    .changed()
                                {
                                    signature_change = TimeSignature::new(numerator, denominator);
                                }
                            }
                        });
                });
                ui.small("SIGNATURE");
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!(
                    "Undo {}  Redo {}",
                    self.history.undo_len(),
                    self.history.redo_len()
                ));
                ui.separator();
                ui.label(format!(
                    "{:.1} kHz",
                    f64::from(self.project.sample_rate) / 1_000.0
                ));
            });
        });
        if let Some(tempo_bpm) = tempo_change {
            self.apply_edit(EditCommand::SetTempo { tempo_bpm });
            self.status_ok(format!("Tempo {tempo_bpm:.2} BPM"));
        }
        if let Some(time_signature) = signature_change {
            self.apply_edit(EditCommand::SetTimeSignature { time_signature });
            self.status_ok(format!(
                "Time signature {}/{}",
                time_signature.numerator, time_signature.denominator
            ));
        }
    }

    #[allow(clippy::too_many_lines)]
    fn track_panel(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::Frame::new()
            .fill(Color32::from_rgb(18, 20, 26))
            .inner_margin(egui::Margin::symmetric(7, 0))
            .show(ui, |ui| {
                ui.set_min_height(RULER_HEIGHT);
                ui.horizontal(|ui| {
                    ui.strong("TRACKS");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("+").on_hover_text("Add track").clicked() {
                            let index = self.project.tracks.len();
                            let track = Track::new(format!("Track {}", index + 1));
                            self.apply_edit(EditCommand::AddTrack { index, track });
                            self.selected_track = Some(index);
                        }
                    });
                });
            });

        let mut delete_track = None;
        let mut bounce_track = None;
        let mut freeze_track = None;
        for index in 0..self.project.tracks.len() {
            let track = &self.project.tracks[index];
            let name = track.name.clone();
            let mut muted = track.muted;
            let mut soloed = track.soloed;
            let mut record_armed = track.recording.armed;
            let mut input_monitoring = track.recording.input_monitoring;
            let mut gain = track.gain;
            let mut pan = track.pan;
            let instrument = track.instrument;
            let midi_channel = track.midi_channel;
            let meter = estimate_track_meter(track, self.playhead_frame);
            let selected = self.selected_track == Some(index);
            let mut mute_changed = false;
            let mut solo_changed = false;
            let mut record_arm_changed = false;
            let mut input_monitor_changed = false;
            let mut gain_changed = false;
            let mut pan_changed = false;

            let row = egui::Frame::new()
                .fill(if selected {
                    Color32::from_rgb(35, 41, 52)
                } else if index.is_multiple_of(2) {
                    Color32::from_rgb(29, 33, 42)
                } else {
                    Color32::from_rgb(25, 29, 37)
                })
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(47, 52, 63)))
                .inner_margin(egui::Margin::symmetric(7, 5))
                .show(ui, |ui| {
                    ui.set_min_height(TRACK_HEIGHT - 12.0);
                    ui.spacing_mut().item_spacing = egui::vec2(5.0, 2.0);
                    ui.spacing_mut().interact_size.y = 16.0;
                    ui.spacing_mut().button_padding.y = 1.0;
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Label::new(
                                    RichText::new(name.clone()).strong().color(Color32::WHITE),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .clicked()
                        {
                            self.selected_track = Some(index);
                            self.selected_clip = None;
                            self.selected_note = None;
                        }
                        ui.label(
                            RichText::new(format!("{} · Ch {}", instrument.label(), midi_channel))
                                .size(9.0)
                                .color(Color32::from_gray(160)),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Delete track").clicked() {
                                delete_track = Some(index);
                            }
                            if ui
                                .small_button("B")
                                .on_hover_text("Bounce this track to a new audio track")
                                .clicked()
                            {
                                bounce_track = Some(index);
                            }
                            if ui
                                .small_button("F")
                                .on_hover_text("Freeze this track to audio and mute the source")
                                .clicked()
                            {
                                freeze_track = Some(index);
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        let record_text = RichText::new("R").color(if record_armed {
                            Color32::from_rgb(255, 95, 95)
                        } else {
                            Color32::from_gray(175)
                        });
                        let monitor_text = RichText::new("I").color(if input_monitoring {
                            Color32::from_rgb(90, 220, 190)
                        } else {
                            Color32::from_gray(175)
                        });
                        mute_changed = ui.toggle_value(&mut muted, "M").changed();
                        solo_changed = ui.toggle_value(&mut soloed, "S").changed();
                        record_arm_changed = ui
                            .toggle_value(&mut record_armed, record_text)
                            .on_hover_text("Arm track for recording")
                            .changed();
                        input_monitor_changed = ui
                            .toggle_value(&mut input_monitoring, monitor_text)
                            .on_hover_text("Monitor live input while recording")
                            .changed();
                        ui.label(RichText::new("VOL").monospace().size(9.0));
                        gain_changed = ui
                            .add_sized(
                                [55.0, 16.0],
                                egui::Slider::new(&mut gain, 0.0..=1.5).show_value(false),
                            )
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(27.0);
                        ui.label(RichText::new("PAN").monospace().size(9.0));
                        pan_changed = ui
                            .add_sized(
                                [105.0, 16.0],
                                egui::Slider::new(&mut pan, -1.0..=1.0).show_value(false),
                            )
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(27.0);
                        ui.label(RichText::new("LVL").monospace().size(9.0));
                        level_meter(ui, meter, 105.0);
                    });
                });
            let row_rect = row.response.rect;
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    row_rect.left_top(),
                    egui::pos2(row_rect.left() + 3.0, row_rect.bottom()),
                ),
                0.0,
                track_color(index),
            );

            if mute_changed {
                self.apply_edit(EditCommand::SetTrackMuted {
                    track_index: index,
                    muted,
                });
            }
            if solo_changed {
                self.apply_edit(EditCommand::SetTrackSolo {
                    track_index: index,
                    soloed,
                });
            }
            if record_arm_changed {
                self.apply_edit(EditCommand::SetTrackRecordArmed {
                    track_index: index,
                    record_armed,
                });
            }
            if input_monitor_changed {
                self.apply_edit(EditCommand::SetTrackInputMonitoring {
                    track_index: index,
                    input_monitoring,
                });
            }
            if gain_changed {
                self.apply_edit(EditCommand::SetTrackGain {
                    track_index: index,
                    gain,
                });
            }
            if pan_changed {
                self.apply_edit(EditCommand::SetTrackPan {
                    track_index: index,
                    pan,
                });
            }
        }

        if let Some(track_index) = bounce_track {
            self.render_track_to_new_audio_track(track_index, false);
        }

        if let Some(track_index) = freeze_track {
            self.render_track_to_new_audio_track(track_index, true);
        }

        if let Some(track_index) = delete_track {
            self.apply_edit(EditCommand::DeleteTrack { track_index });
            self.selected_track = self
                .project
                .tracks
                .len()
                .checked_sub(1)
                .map(|last| track_index.min(last));
            self.selected_clip = None;
        }

        let mut master_gain = self.project.master_gain;
        let mut master_changed = false;
        let master_meter = estimate_master_meter(&self.project, self.playhead_frame);
        egui::Frame::new()
            .fill(Color32::from_rgb(20, 24, 31))
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(58, 65, 78)))
            .inner_margin(egui::Margin::symmetric(7, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("MASTER");
                    ui.label(
                        RichText::new(format!("{} dB", format_gain_db(master_gain)))
                            .monospace()
                            .size(9.0)
                            .color(Color32::from_gray(175)),
                    );
                });
                master_changed = ui
                    .add_sized(
                        [170.0, 18.0],
                        egui::Slider::new(&mut master_gain, 0.0..=1.5).show_value(false),
                    )
                    .changed();
                level_meter(ui, master_meter, 170.0);
            });
        if master_changed {
            self.apply_edit(EditCommand::SetMasterGain { gain: master_gain });
        }

        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(7, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("+ Note").clicked() {
                        self.add_tone_clip();
                    }
                    if ui.button("Import…").clicked() {
                        self.import_wav_dialog();
                    }
                });
            });
    }

    #[allow(clippy::too_many_lines)]
    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Inspector");
        let mut duplicate_track = None;
        let mut load_soundfont_track = None;
        if let Some(track_index) = self.selected_track
            && let Some(track) = self.project.tracks.get(track_index)
        {
            let original = track.clone();
            let mut edited = original.clone();
            ui.label("Track name");
            ui.text_edit_singleline(&mut edited.name);
            ui.horizontal(|ui| {
                ui.label("Instrument");
                egui::ComboBox::from_id_salt(("track_instrument", track_index))
                    .selected_text(edited.instrument.label())
                    .show_ui(ui, |ui| {
                        for choice in Instrument::ALL {
                            ui.selectable_value(&mut edited.instrument, choice, choice.label());
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("SoundFont");
                if ui.button("Load SF2…").clicked() {
                    load_soundfont_track = Some(track_index);
                }
                if ui.small_button("Clear").clicked() {
                    edited.soundfont = None;
                }
            });
            if let Some(selected) = &edited.soundfont {
                let options = self.soundfont_presets.get(&selected.path);
                if let Some(options) = options {
                    egui::ComboBox::from_id_salt(("soundfont_preset", track_index))
                        .width(190.0)
                        .selected_text(selected.label())
                        .show_ui(ui, |ui| {
                            for preset in options {
                                ui.selectable_value(
                                    &mut edited.soundfont,
                                    Some(preset.clone()),
                                    preset.label(),
                                );
                            }
                        });
                } else {
                    ui.small(selected.label());
                }
            } else {
                ui.small("Built-in oscillator");
            }
            ui.horizontal(|ui| {
                ui.label("MIDI channel");
                ui.add(egui::DragValue::new(&mut edited.midi_channel).range(1..=16));
            });
            ui.horizontal(|ui| {
                ui.label("Record input");
                egui::ComboBox::from_id_salt(("record_input", track_index))
                    .selected_text(track_input_label(edited.input))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut edited.input,
                            TrackInput::Audio,
                            "Audio Stereo (1–2)",
                        );
                        ui.selectable_value(
                            &mut edited.input,
                            TrackInput::AudioMonoLeft,
                            "Audio Mono (Input 1)",
                        );
                        ui.selectable_value(
                            &mut edited.input,
                            TrackInput::AudioMonoRight,
                            "Audio Mono (Input 2)",
                        );
                        ui.selectable_value(&mut edited.input, TrackInput::MidiOmni, "MIDI Omni");
                        ui.separator();
                        for channel in 1..=16 {
                            ui.selectable_value(
                                &mut edited.input,
                                TrackInput::MidiChannel(channel),
                                format!("MIDI Ch {channel}"),
                            );
                        }
                    });
            });
            if edited.input.is_midi() {
                let active = self
                    .last_midi_activity
                    .is_some_and(|time| time.elapsed() < Duration::from_millis(180));
                ui.horizontal(|ui| {
                    ui.colored_label(
                        if active {
                            Color32::from_rgb(80, 235, 155)
                        } else {
                            Color32::from_gray(100)
                        },
                        "MIDI IN",
                    );
                    ui.small(
                        self.midi_input
                            .connected_name()
                            .map_or("Connect a device from MIDI menu", |name| name),
                    );
                });
            }
            ui.add_space(4.0);
            ui.collapsing("Routing, sends & inserts", |ui| {
                track_routing_editor(ui, track_index, &mut edited, &self.project.buses);
                ui.separator();
                insert_chain_editor(ui, ("track_inserts", track_index), &mut edited.inserts);
            });
            ui.add_space(4.0);
            ui.label("MIDI CC lane");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("midi_cc_controller", track_index))
                    .width(122.0)
                    .selected_text(midi_cc_label(self.midi_tools.cc_controller))
                    .show_ui(ui, |ui| {
                        for controller in [1, 7, 10, 11, 64, 71, 74] {
                            ui.selectable_value(
                                &mut self.midi_tools.cc_controller,
                                controller,
                                midi_cc_label(controller),
                            );
                        }
                    });
                ui.add(
                    egui::DragValue::new(&mut self.midi_tools.cc_value)
                        .range(0..=127)
                        .speed(1),
                );
            });
            let cc_index = edited.midi_cc.iter().position(|point| {
                point.frame == self.playhead_frame
                    && point.controller == self.midi_tools.cc_controller
            });
            if let Some(index) = cc_index {
                let mut value = edited.midi_cc[index].value;
                if ui
                    .add(egui::Slider::new(&mut value, 0..=127).text("At playhead"))
                    .changed()
                {
                    edited.midi_cc[index].value = value;
                    self.midi_tools.cc_value = value;
                }
            }
            ui.horizontal(|ui| {
                if ui
                    .button(if cc_index.is_some() {
                        "Update CC"
                    } else {
                        "Write CC"
                    })
                    .clicked()
                {
                    if let Some(index) = cc_index {
                        edited.midi_cc[index].value = self.midi_tools.cc_value;
                    } else {
                        edited.midi_cc.push(MidiControlPoint {
                            frame: self.playhead_frame,
                            controller: self.midi_tools.cc_controller,
                            value: self.midi_tools.cc_value,
                        });
                        edited
                            .midi_cc
                            .sort_by_key(|point| (point.frame, point.controller));
                    }
                }
                if ui
                    .add_enabled(cc_index.is_some(), egui::Button::new("Delete"))
                    .clicked()
                    && let Some(index) = cc_index
                {
                    edited.midi_cc.remove(index);
                }
                if ui
                    .add_enabled(
                        edited
                            .midi_cc
                            .iter()
                            .any(|point| point.controller == self.midi_tools.cc_controller),
                        egui::Button::new("Clear lane"),
                    )
                    .clicked()
                {
                    edited
                        .midi_cc
                        .retain(|point| point.controller != self.midi_tools.cc_controller);
                }
            });
            ui.small(format!("{} CC event(s)", edited.midi_cc.len()));
            ui.collapsing("Pitch bend & aftertouch", |ui| {
                let pitch_index = edited
                    .midi_pitch_bend
                    .iter()
                    .position(|point| point.frame == self.playhead_frame);
                let mut bend = pitch_index.map_or(self.midi_tools.pitch_bend, |index| {
                    edited.midi_pitch_bend[index].value
                });
                if ui
                    .add(egui::Slider::new(&mut bend, -8192..=8191).text("Pitch wheel"))
                    .changed()
                {
                    self.midi_tools.pitch_bend = bend;
                    if let Some(index) = pitch_index {
                        edited.midi_pitch_bend[index].value = bend;
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(if pitch_index.is_some() {
                            "Update bend"
                        } else {
                            "Write bend"
                        })
                        .clicked()
                    {
                        if let Some(index) = pitch_index {
                            edited.midi_pitch_bend[index].value = bend;
                        } else {
                            edited.midi_pitch_bend.push(MidiPitchBendPoint {
                                frame: self.playhead_frame,
                                value: bend,
                            });
                            edited.midi_pitch_bend.sort_by_key(|point| point.frame);
                        }
                    }
                    if ui
                        .add_enabled(pitch_index.is_some(), egui::Button::new("Delete"))
                        .clicked()
                        && let Some(index) = pitch_index
                    {
                        edited.midi_pitch_bend.remove(index);
                    }
                });

                let pressure_index = edited
                    .midi_channel_pressure
                    .iter()
                    .position(|point| point.frame == self.playhead_frame);
                let mut pressure = pressure_index
                    .map_or(self.midi_tools.channel_pressure, |index| {
                        edited.midi_channel_pressure[index].value
                    });
                if ui
                    .add(egui::Slider::new(&mut pressure, 0..=127).text("Aftertouch"))
                    .changed()
                {
                    self.midi_tools.channel_pressure = pressure;
                    if let Some(index) = pressure_index {
                        edited.midi_channel_pressure[index].value = pressure;
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(if pressure_index.is_some() {
                            "Update pressure"
                        } else {
                            "Write pressure"
                        })
                        .clicked()
                    {
                        if let Some(index) = pressure_index {
                            edited.midi_channel_pressure[index].value = pressure;
                        } else {
                            edited.midi_channel_pressure.push(MidiChannelPressurePoint {
                                frame: self.playhead_frame,
                                value: pressure,
                            });
                            edited
                                .midi_channel_pressure
                                .sort_by_key(|point| point.frame);
                        }
                    }
                    if ui
                        .add_enabled(pressure_index.is_some(), egui::Button::new("Delete"))
                        .clicked()
                        && let Some(index) = pressure_index
                    {
                        edited.midi_channel_pressure.remove(index);
                    }
                });

                let program_index = edited
                    .midi_program_changes
                    .iter()
                    .position(|point| point.frame == self.playhead_frame);
                let mut program = program_index.map_or(self.midi_tools.program_change, |index| {
                    edited.midi_program_changes[index].program
                });
                if ui
                    .add(egui::Slider::new(&mut program, 1..=128).text("Program"))
                    .changed()
                {
                    self.midi_tools.program_change = program;
                    if let Some(index) = program_index {
                        edited.midi_program_changes[index].program = program;
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(if program_index.is_some() {
                            "Update program"
                        } else {
                            "Write program"
                        })
                        .clicked()
                    {
                        if let Some(index) = program_index {
                            edited.midi_program_changes[index].program = program;
                        } else {
                            edited.midi_program_changes.push(MidiProgramChangePoint {
                                frame: self.playhead_frame,
                                program,
                            });
                            edited.midi_program_changes.sort_by_key(|point| point.frame);
                        }
                    }
                    if ui
                        .add_enabled(program_index.is_some(), egui::Button::new("Delete"))
                        .clicked()
                        && let Some(index) = program_index
                    {
                        edited.midi_program_changes.remove(index);
                    }
                });
            });
            if ui.button("Duplicate track").clicked() {
                let mut copy = edited.clone();
                copy.name = format!("{} Copy", copy.name);
                duplicate_track = Some((track_index + 1, copy));
            }
            ui.separator();
            ui.label("Volume automation");
            let point_at_playhead = edited
                .volume_automation
                .iter()
                .position(|point| point.frame == self.playhead_frame);
            let value_at_playhead = edited.automation_gain_at(self.playhead_frame);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        point_at_playhead.is_none(),
                        egui::Button::new("Write point"),
                    )
                    .on_hover_text("Write a volume automation point at the playhead")
                    .clicked()
                {
                    edited.volume_automation.push(AutomationPoint {
                        frame: self.playhead_frame,
                        value: value_at_playhead,
                    });
                    edited.volume_automation.sort_by_key(|point| point.frame);
                }
                if ui
                    .add_enabled(
                        !edited.volume_automation.is_empty(),
                        egui::Button::new("Clear"),
                    )
                    .clicked()
                {
                    edited.volume_automation.clear();
                }
            });
            if let Some(point_index) = edited
                .volume_automation
                .iter()
                .position(|point| point.frame == self.playhead_frame)
            {
                let mut delete_point = false;
                let point_value = edited.volume_automation[point_index].value;
                ui.horizontal(|ui| {
                    ui.label(format!("At playhead {point_value:.2}"));
                    delete_point = ui
                        .small_button("×")
                        .on_hover_text("Delete this automation point")
                        .clicked();
                    ui.add_sized(
                        [78.0, 18.0],
                        egui::Slider::new(
                            &mut edited.volume_automation[point_index].value,
                            0.0..=2.0,
                        )
                        .show_value(false),
                    );
                });
                if delete_point {
                    edited.volume_automation.remove(point_index);
                }
            } else {
                ui.small(format!(
                    "Playhead value {:.2} · {} points",
                    value_at_playhead,
                    edited.volume_automation.len()
                ));
            }
            if edited != original {
                self.apply_edit(EditCommand::ReplaceTrack {
                    track_index,
                    track: edited,
                });
            }
        }
        if let Some(track_index) = load_soundfont_track {
            self.import_soundfont_for_track(track_index);
        }
        if let Some((index, track)) = duplicate_track {
            self.apply_edit(EditCommand::AddTrack { index, track });
            self.selected_track = Some(index);
            self.selected_clip = None;
        }
        ui.separator();
        self.arranger_inspector(ui);
        ui.separator();
        self.marker_inspector(ui);
        ui.separator();
        self.mixer_inspector(ui);
        ui.separator();
        let mut changed = false;
        let mut trim_left = false;
        let mut trim_right = false;
        let mut delete_clip = false;
        let mut copy_clip = false;
        let mut duplicate_clip = false;
        let mut split_clip = false;
        let mut use_take = None;
        let mut delete_take = None;
        let mut relink_audio = false;
        let mut normalize_audio = false;
        let mut original_clip = None;
        if let Some((track_index, clip_index)) = self.selected_clip {
            let take_rows = self
                .project
                .tracks
                .get(track_index)
                .and_then(|track| {
                    let clip = track.clips.get(clip_index)?;
                    Some(
                        track
                            .take_lanes
                            .iter()
                            .enumerate()
                            .filter(|(_, take)| {
                                take.start_frame < clip.end_frame()
                                    && take.end_frame() > clip.start_frame
                            })
                            .map(|(index, take)| {
                                (
                                    index,
                                    take.clone(),
                                    take.start_frame <= clip.start_frame
                                        && take.end_frame() >= clip.end_frame(),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default();
            let Some(clip) = self
                .project
                .tracks
                .get_mut(track_index)
                .and_then(|track| track.clips.get_mut(clip_index))
            else {
                self.selected_clip = None;
                ui.label("Select a clip on the timeline.");
                return;
            };
            original_clip = Some(clip.clone());

            ui.label("Clip name");
            changed |= ui.text_edit_singleline(&mut clip.name).changed();
            ui.add_space(6.0);
            ui.label(format!(
                "Start: {}",
                format_time(clip.start_frame, self.project.sample_rate)
            ));
            ui.label(format!(
                "Length: {}",
                format_time(clip.length_frames, self.project.sample_rate)
            ));
            ui.add_space(8.0);
            ui.label("Clip envelope");
            changed |= ui
                .add(egui::Slider::new(&mut clip.gain, 0.0..=2.0).text("Clip gain"))
                .changed();
            ui.small(format!("{} dB", format_gain_db(clip.gain)));
            let sample_rate = f64::from(self.project.sample_rate);
            #[allow(clippy::cast_precision_loss)]
            let clip_seconds = clip.length_frames as f64 / sample_rate;
            #[allow(clippy::cast_precision_loss)]
            let mut fade_in_seconds = clip.fade_in_frames as f64 / sample_rate;
            #[allow(clippy::cast_precision_loss)]
            let mut fade_out_seconds = clip.fade_out_frames as f64 / sample_rate;
            ui.horizontal(|ui| {
                ui.label("Fade in");
                if ui
                    .add(
                        egui::DragValue::new(&mut fade_in_seconds)
                            .range(0.0..=clip_seconds)
                            .speed(0.01)
                            .fixed_decimals(3)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    {
                        clip.fade_in_frames = (fade_in_seconds * sample_rate).round() as u64;
                    }
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Fade out");
                if ui
                    .add(
                        egui::DragValue::new(&mut fade_out_seconds)
                            .range(0.0..=clip_seconds)
                            .speed(0.01)
                            .fixed_decimals(3)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    {
                        clip.fade_out_frames = (fade_out_seconds * sample_rate).round() as u64;
                    }
                    changed = true;
                }
            });
            egui::ComboBox::from_id_salt("clip_fade_curve")
                .selected_text(clip.fade_curve.label())
                .show_ui(ui, |ui| {
                    for curve in FadeCurve::ALL {
                        changed |= ui
                            .selectable_value(&mut clip.fade_curve, curve, curve.label())
                            .changed();
                    }
                });
            let playhead_inside =
                self.playhead_frame > clip.start_frame && self.playhead_frame < clip.end_frame();
            let is_audio_file = matches!(clip.source, ClipSource::AudioFile { .. });
            match &mut clip.source {
                ClipSource::Midi { notes, amplitude } => {
                    ui.add_space(8.0);
                    ui.label("MIDI part");
                    ui.label(format!("{} notes", notes.len()));
                    if let Some((selected_track, selected_clip, note_index)) = self.selected_note
                        && selected_track == track_index
                        && selected_clip == clip_index
                        && let Some(note) = notes.get_mut(note_index)
                    {
                        ui.label(format!("Selected note: {}", midi_note_name(note.midi_note)));
                        changed |= ui
                            .add(egui::Slider::new(&mut note.velocity, 1..=127).text("Velocity"))
                            .changed();
                    }
                    changed |= ui
                        .add(egui::Slider::new(amplitude, 0.0..=1.0).text("Level"))
                        .changed();
                    ui.small("Edit pitch and timing in the piano roll.");
                }
                ClipSource::Sine {
                    frequency_hz,
                    amplitude,
                } => {
                    ui.add_space(8.0);
                    ui.label("Built-in tone");
                    changed |= musical_pitch_editor(ui, (track_index, clip_index), frequency_hz);
                    changed |= ui
                        .add(egui::Slider::new(amplitude, 0.0..=1.0).text("Amplitude"))
                        .changed();
                }
                ClipSource::AudioFile {
                    path,
                    source_offset_frames,
                    source_sample_rate,
                    channels,
                    reversed,
                } => {
                    ui.add_space(8.0);
                    ui.label("Linked WAV audio");
                    ui.monospace(
                        Path::new(path)
                            .file_name()
                            .map_or_else(|| path.as_str().into(), |name| name.to_string_lossy()),
                    );
                    ui.label(format!(
                        "{source_sample_rate} Hz · {channels} ch · source offset {source_offset_frames}"
                    ));
                    changed |= ui
                        .toggle_value(reversed, "Reverse")
                        .on_hover_text("Play this audio clip backwards without rewriting the WAV")
                        .changed();
                    let source_frames = self
                        .waveforms
                        .get(path)
                        .map(|cached| cached.overview.source_frames);
                    let max_offset = source_frames.map_or(u64::MAX / 2, |frames| {
                        frames.saturating_sub(1).max(*source_offset_frames)
                    });
                    let slip_project_frames = grid_note_frames(
                        self.project.sample_rate,
                        self.project.tempo_bpm,
                        self.arrange.snap_grid,
                    );
                    ui.label(RichText::new("Slip edit").strong());
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("Slip -grid")
                            .on_hover_text(
                                "Move the visible audio content earlier without moving the clip",
                            )
                            .clicked()
                        {
                            *source_offset_frames = slipped_source_offset(
                                *source_offset_frames,
                                -i64::try_from(slip_project_frames).unwrap_or(i64::MAX),
                                self.project.sample_rate,
                                *source_sample_rate,
                                source_frames,
                            );
                            changed = true;
                        }
                        if ui
                            .small_button("Slip +grid")
                            .on_hover_text(
                                "Move the visible audio content later without moving the clip",
                            )
                            .clicked()
                        {
                            *source_offset_frames = slipped_source_offset(
                                *source_offset_frames,
                                i64::try_from(slip_project_frames).unwrap_or(i64::MAX),
                                self.project.sample_rate,
                                *source_sample_rate,
                                source_frames,
                            );
                            changed = true;
                        }
                    });
                    changed |= ui
                        .add(
                            egui::DragValue::new(source_offset_frames)
                                .range(0..=max_offset)
                                .speed(f64::from(*source_sample_rate) / 100.0)
                                .prefix("Offset ")
                                .suffix(" src frames"),
                        )
                        .changed();
                    if let Some(error) = self.waveform_errors.get(path) {
                        ui.colored_label(Color32::from_rgb(255, 125, 125), error);
                    }
                    relink_audio = ui
                        .button("Relink WAV…")
                        .on_hover_text("Replace the referenced WAV while keeping clip timing")
                        .clicked();
                    normalize_audio = ui
                        .button("Normalize gain")
                        .on_hover_text(
                            "Set clip gain from the selected audio range peak without rewriting the WAV",
                        )
                        .clicked();
                }
            }
            if is_audio_file && !take_rows.is_empty() {
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new("Take lanes / Comp").strong());
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(115, 205, 145), "● Active");
                    ui.label(&clip.name);
                });
                for (take_index, take, covers_section) in take_rows {
                    ui.horizontal(|ui| {
                        ui.label(format!("T{}", take_index + 1));
                        ui.add_sized([78.0, 18.0], egui::Label::new(&take.name).truncate())
                            .on_hover_text(&take.path);
                        if ui
                            .add_enabled(covers_section, egui::Button::new("Use"))
                            .on_hover_text(if covers_section {
                                "Use this take for the selected clip section"
                            } else {
                                "This take does not cover the whole selected section"
                            })
                            .clicked()
                        {
                            use_take = Some(take_index);
                        }
                        if ui
                            .small_button("×")
                            .on_hover_text("Delete take lane")
                            .clicked()
                        {
                            delete_take = Some(take_index);
                        }
                    });
                }
                ui.small("Split the active clip, then Use a take to comp only that section.");
            }
            ui.add_space(8.0);
            ui.label("Non-destructive trim at playhead");
            ui.horizontal(|ui| {
                trim_left = ui
                    .add_enabled(
                        playhead_inside && is_audio_file,
                        egui::Button::new("Trim left"),
                    )
                    .on_hover_text("Keep audio to the right of the playhead")
                    .clicked();
                trim_right = ui
                    .add_enabled(playhead_inside, egui::Button::new("Trim right"))
                    .on_hover_text("Keep audio to the left of the playhead")
                    .clicked();
            });
            if !is_audio_file {
                ui.small("Left trim is available for imported audio clips.");
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                copy_clip = ui.button("Copy").clicked();
                duplicate_clip = ui.button("Duplicate").clicked();
                split_clip = ui
                    .add_enabled(playhead_inside, egui::Button::new("Split"))
                    .on_hover_text("Split the clip at the playhead")
                    .clicked();
            });
            delete_clip = ui.button("Delete clip").clicked();
        } else {
            ui.label("Select a clip on the timeline.");
            ui.add_space(12.0);
            ui.label("Tip: click an empty timeline area to move the playhead, or drag a clip to move it.");
        }

        if changed
            && let Some((track_index, clip_index)) = self.selected_clip
            && let Some(original) = original_clip
            && let Some(slot) = self
                .project
                .tracks
                .get_mut(track_index)
                .and_then(|track| track.clips.get_mut(clip_index))
        {
            let edited = std::mem::replace(slot, original);
            self.apply_edit(EditCommand::ReplaceClip {
                track_index,
                clip_index,
                clip: edited,
            });
        }
        if relink_audio {
            self.relink_selected_audio_clip();
        }
        if normalize_audio {
            self.normalize_selected_audio_clip();
        }
        if trim_left
            && let Some(command) = clip_trim_command(
                &self.project,
                self.selected_clip,
                self.playhead_frame,
                TrimSide::Left,
            )
        {
            self.apply_edit(command);
        } else if trim_right
            && let Some(command) = clip_trim_command(
                &self.project,
                self.selected_clip,
                self.playhead_frame,
                TrimSide::Right,
            )
        {
            self.apply_edit(command);
        }
        if delete_clip && let Some((track_index, clip_index)) = self.selected_clip {
            self.apply_edit(EditCommand::DeleteClip {
                track_index,
                clip_index,
            });
            self.selected_clip = None;
        }
        if copy_clip {
            self.copy_selected_clip();
        }
        if duplicate_clip {
            self.duplicate_selected_clip();
        }
        if split_clip {
            self.split_selected_clip();
        }
        if let Some(take_index) = use_take {
            self.comp_selected_take(take_index);
        } else if let Some(take_index) = delete_take
            && let Some((track_index, _)) = self.selected_clip
        {
            self.apply_edit(EditCommand::DeleteTakeLane {
                track_index,
                take_index,
            });
            self.status_ok("Deleted alternate take");
        }
        if ui
            .add_enabled(
                self.clip_clipboard.is_some() && self.selected_track.is_some(),
                egui::Button::new("Paste clip at playhead"),
            )
            .clicked()
        {
            self.paste_clip();
        }
    }

    fn copy_selected_clip(&mut self) {
        let Some((track_index, clip_index)) = self.selected_clip else {
            return;
        };
        let Some(clip) = self
            .project
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.get(clip_index))
            .cloned()
        else {
            return;
        };
        self.status_ok(format!("Copied {}", clip.name));
        self.clip_clipboard = Some(clip);
    }

    fn duplicate_selected_clip(&mut self) {
        let Some((track_index, clip_index)) = self.selected_clip else {
            return;
        };
        let Some(mut clip) = self
            .project
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.get(clip_index))
            .cloned()
        else {
            return;
        };
        clip.name = format!("{} Copy", clip.name);
        clip.start_frame = clip.end_frame();
        let next_index = clip_index + 1;
        self.apply_edit(EditCommand::AddClip {
            track_index,
            clip_index: next_index,
            clip,
        });
        self.selected_clip = Some((track_index, next_index));
        self.status_ok("Duplicated clip");
    }

    fn paste_clip(&mut self) {
        let (Some(track_index), Some(mut clip)) =
            (self.selected_track, self.clip_clipboard.clone())
        else {
            return;
        };
        let Some(track) = self.project.tracks.get(track_index) else {
            return;
        };
        clip.name = format!("{} Copy", clip.name);
        clip.start_frame = snap_frame(
            self.playhead_frame,
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.arrange.snap_enabled,
            self.arrange.snap_grid,
        );
        let clip_index = track.clips.len();
        self.apply_edit(EditCommand::AddClip {
            track_index,
            clip_index,
            clip,
        });
        self.selected_clip = Some((track_index, clip_index));
        self.status_ok("Pasted clip at playhead");
    }

    fn split_selected_clip(&mut self) {
        let Some((track_index, clip_index)) = self.selected_clip else {
            return;
        };
        let Some(mut track) = self.project.tracks.get(track_index).cloned() else {
            return;
        };
        let Some((left, right)) = track.clips.get(clip_index).and_then(|clip| {
            split_clip_at_frame(clip, self.playhead_frame, self.project.sample_rate)
        }) else {
            self.status_error("Move the playhead inside the clip before splitting");
            return;
        };
        track.clips[clip_index] = left;
        track.clips.insert(clip_index + 1, right);
        self.apply_edit(EditCommand::ReplaceTrack { track_index, track });
        self.selected_clip = Some((track_index, clip_index + 1));
        self.status_ok("Split clip at playhead");
    }

    fn comp_selected_take(&mut self, take_index: usize) {
        let Some((track_index, clip_index)) = self.selected_clip else {
            return;
        };
        let Some(mut track) = self.project.tracks.get(track_index).cloned() else {
            return;
        };
        let Some(active_clip) = track.clips.get(clip_index).cloned() else {
            return;
        };
        let Some(active_take) = AudioTake::from_clip(&active_clip) else {
            self.status_error("Only audio clips can be comped");
            return;
        };
        let Some(alternate) = track.take_lanes.get(take_index) else {
            self.status_error("The selected take no longer exists");
            return;
        };
        let Some(comped) = clip_from_take_section(
            alternate,
            active_clip.start_frame,
            active_clip.end_frame(),
            self.project.sample_rate,
        ) else {
            self.status_error("That take does not cover the selected clip section");
            return;
        };
        track.take_lanes.push(active_take);
        track.clips[clip_index] = comped;
        self.apply_edit(EditCommand::ReplaceTrack { track_index, track });
        self.refresh_waveforms();
        self.status_ok("Comped selected section from alternate take");
    }

    fn render_track_to_new_audio_track(&mut self, track_index: usize, freeze_source: bool) {
        let Some(source_track) = self.project.tracks.get(track_index) else {
            return;
        };
        if source_track.muted {
            self.status_error("Unmute the track before rendering it to audio");
            return;
        }
        if source_track.clips.is_empty() || self.project.duration_frames() == 0 {
            self.status_error("The selected track has no audio to render");
            return;
        }
        let stems = match try_render_track_stems(&self.project) {
            Ok(stems) => stems,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        let Some(stem) = stems
            .into_iter()
            .find(|stem| stem.track_index == track_index)
        else {
            self.status_error("Could not render the selected track stem");
            return;
        };
        let path = match bounce_output_path(
            self.project_path.as_deref(),
            &self.project.name,
            &stem.track_name,
        ) {
            Ok(path) => path,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        if let Err(error) = write_stereo_i16_wav(&path, self.project.sample_rate, &stem.samples) {
            self.status_error(format!(
                "Could not write bounce {}: {error}",
                path.display()
            ));
            return;
        }

        let render_label = if freeze_source { "Freeze" } else { "Bounce" };
        let mut bounced = Track::new(format!("{} {render_label}", stem.track_name));
        bounced.clips.push(Clip {
            name: format!("{} {render_label}", stem.track_name),
            start_frame: 0,
            length_frames: self.project.duration_frames(),
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::AudioFile {
                path: path.to_string_lossy().into_owned(),
                source_offset_frames: 0,
                source_sample_rate: self.project.sample_rate,
                channels: 2,
                reversed: false,
            },
        });
        let index = self.project.tracks.len();
        self.apply_edit(EditCommand::AddTrack {
            index,
            track: bounced,
        });
        if freeze_source {
            self.apply_edit(EditCommand::SetTrackMuted {
                track_index,
                muted: true,
            });
        }
        self.selected_track = Some(index);
        self.selected_clip = Some((index, 0));
        self.selected_note = None;
        self.refresh_waveforms();
        if freeze_source {
            self.status_ok(format!("Froze {} to {}", stem.track_name, path.display()));
        } else {
            self.status_ok(format!("Bounced {} to {}", stem.track_name, path.display()));
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let interaction = TimelineView {
                    pixels_per_second: self.pixels_per_second,
                    playhead_frame: self.playhead_frame,
                    cycle_range: self
                        .loop_enabled
                        .then_some((self.loop_start_frame, self.loop_end_frame)),
                    selected_track: self.selected_track,
                    selected_clip: self.selected_clip,
                    draw_mode: self.arrange.tool == EditTool::Draw,
                }
                .show(
                    ui,
                    &self.project,
                    &self.waveform_overviews(),
                    &self.waveform_errors,
                );

                if let Some(frame) = interaction.seek_frame {
                    if interaction.selected_clip.is_none() {
                        self.selected_clip = None;
                    }
                    self.seek(frame);
                }
                if let Some(selection) = interaction.selected_clip {
                    if self.selected_clip != Some(selection) {
                        self.piano_roll.scroll_to_selection = true;
                        self.selected_note = None;
                    }
                    self.selected_clip = Some(selection);
                    self.selected_track = Some(selection.0);
                }
                if let Some((track_index, clip_index, start_frame)) = interaction.move_clip {
                    self.apply_edit(EditCommand::MoveClip {
                        track_index,
                        clip_index,
                        start_frame: snap_frame(
                            start_frame,
                            self.project.sample_rate,
                            self.project.tempo_bpm,
                            self.arrange.snap_enabled,
                            self.arrange.snap_grid,
                        ),
                    });
                }
                if let Some((track_index, start_frame)) = interaction.draw_note {
                    let midi_note = self.selected_midi_note().unwrap_or(60);
                    self.add_grid_note(track_index, start_frame, midi_note);
                }
            });
    }

    #[allow(clippy::too_many_lines)]
    fn piano_roll(&mut self, ui: &mut egui::Ui) {
        let mut quantize = false;
        let mut swing_quantize = false;
        let mut groove_quantize = false;
        let mut humanize = false;
        let mut legato = false;
        let mut cleanup = false;
        let mut scale_quantize = false;
        let mut set_length = false;
        let mut transpose = None;
        let mut velocity_change = None;
        ui.horizontal(|ui| {
            ui.heading("Piano Roll");
            if let Some(track_index) = self.selected_track
                && let Some(track) = self.project.tracks.get(track_index)
            {
                ui.label(RichText::new(&track.name).color(Color32::LIGHT_GRAY));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("Close piano roll")
                    .clicked()
                {
                    self.piano_roll.open = false;
                }
                ui.label("Drag note: move/pitch · Right edge: length · Double-click: delete");
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("MIDI").monospace().strong());
            ui.menu_button("Quantize", |ui| {
                ui.add(
                    egui::Slider::new(&mut self.midi_tools.quantize_strength, 1..=100)
                        .text("Strength %"),
                );
                ui.checkbox(&mut self.midi_tools.quantize_ends, "Quantize note ends");
                ui.add(
                    egui::Slider::new(&mut self.midi_tools.swing_percent, 50..=75).text("Swing %"),
                );
                egui::ComboBox::from_label("Groove")
                    .selected_text(self.midi_tools.groove_template.label())
                    .show_ui(ui, |ui| {
                        for template in GrooveTemplate::ALL {
                            ui.selectable_value(
                                &mut self.midi_tools.groove_template,
                                template,
                                template.label(),
                            );
                        }
                    });
                ui.add(
                    egui::Slider::new(&mut self.midi_tools.groove_amount, 0..=100)
                        .text("Groove amt %"),
                );
                if ui.button("Apply straight").clicked() {
                    quantize = true;
                    ui.close();
                }
                if ui.button("Apply swing").clicked() {
                    swing_quantize = true;
                    ui.close();
                }
                if ui.button("Apply groove").clicked() {
                    groove_quantize = true;
                    ui.close();
                }
            });
            for (label, amount) in [("-12", -12), ("-1", -1), ("+1", 1), ("+12", 12)] {
                if ui
                    .small_button(label)
                    .on_hover_text(
                        "Transpose selected note, or the whole part if no note is selected",
                    )
                    .clicked()
                {
                    transpose = Some(amount);
                }
            }
            if ui.small_button("Vel -5").clicked() {
                velocity_change = Some(-5);
            }
            if ui.small_button("Vel +5").clicked() {
                velocity_change = Some(5);
            }
            ui.menu_button("Scale", |ui| {
                egui::ComboBox::from_label("Root")
                    .selected_text(pitch_class_name(self.midi_tools.scale_root))
                    .show_ui(ui, |ui| {
                        for root in 0..12 {
                            ui.selectable_value(
                                &mut self.midi_tools.scale_root,
                                root,
                                pitch_class_name(root),
                            );
                        }
                    });
                egui::ComboBox::from_label("Scale")
                    .selected_text(self.midi_tools.scale.label())
                    .show_ui(ui, |ui| {
                        for scale in MidiScale::ALL {
                            ui.selectable_value(&mut self.midi_tools.scale, scale, scale.label());
                        }
                    });
                if ui.button("Conform selected/part").clicked() {
                    scale_quantize = true;
                    ui.close();
                }
            });
            ui.menu_button("Length", |ui| {
                egui::ComboBox::from_label("Note length")
                    .selected_text(self.midi_tools.note_length_grid.label())
                    .show_ui(ui, |ui| {
                        for grid in SnapGrid::ALL {
                            ui.selectable_value(
                                &mut self.midi_tools.note_length_grid,
                                grid,
                                grid.label(),
                            );
                        }
                    });
                if ui.button("Set selected/part").clicked() {
                    set_length = true;
                    ui.close();
                }
            });
            if ui
                .button("Legato")
                .on_hover_text("Extend notes/chords to the next note start")
                .clicked()
            {
                legato = true;
            }
            ui.menu_button("Humanize", |ui| {
                ui.add(
                    egui::Slider::new(&mut self.midi_tools.humanize_timing_ms, 0.0..=50.0)
                        .text("Timing ms"),
                );
                ui.add(
                    egui::Slider::new(&mut self.midi_tools.humanize_velocity, 0..=32)
                        .text("Velocity ±"),
                );
                if ui.button("Apply to part").clicked() {
                    humanize = true;
                    ui.close();
                }
            });
            if ui
                .button("Clean overlaps")
                .on_hover_text("Merge duplicates and trim overlapping notes of the same pitch")
                .clicked()
            {
                cleanup = true;
            }
        });
        ui.separator();

        if quantize {
            self.quantize_selected_midi_clip();
        }
        if swing_quantize {
            self.swing_quantize_selected_midi_clip();
        }
        if groove_quantize {
            self.groove_quantize_selected_midi_clip();
        }
        if let Some(semitones) = transpose {
            self.transpose_selected_midi(semitones);
        }
        if let Some(amount) = velocity_change {
            self.adjust_selected_midi_velocity(amount);
        }
        if scale_quantize {
            self.conform_selected_midi_to_scale();
        }
        if set_length {
            self.set_selected_midi_note_lengths();
        }
        if legato {
            self.edit_selected_midi_clip("Made MIDI part legato", |notes| {
                make_notes_legato(notes);
            });
        }
        if humanize {
            self.humanize_selected_midi_clip();
        }
        if cleanup {
            self.edit_selected_midi_clip("Cleaned MIDI overlaps", remove_note_overlaps);
        }

        let interaction = PianoRollView {
            pixels_per_second: self.pixels_per_second,
            selected_track: self.selected_track,
            selected_clip: self.selected_clip,
            selected_note: self.selected_note,
            playhead_frame: self.playhead_frame,
            scroll_to_selection: self.piano_roll.scroll_to_selection,
            snap_enabled: self.arrange.snap_enabled,
            grid_divisions_per_beat: self.arrange.snap_grid.divisions_per_beat(),
            cc_controller: self.midi_tools.cc_controller,
        }
        .show(ui, &self.project);
        self.piano_roll.scroll_to_selection = false;

        if let Some((track_index, clip_index)) = interaction.selected_clip {
            self.selected_track = Some(track_index);
            self.selected_clip = Some((track_index, clip_index));
        }
        if let Some(selection) = interaction.selected_note {
            self.selected_note = Some(selection);
        }
        if let Some(frame) = interaction.seek_frame {
            self.seek(frame);
        }
        if let Some(request) = interaction.add_note {
            self.add_grid_note(request.track_index, request.start_frame, request.midi_note);
        }
        if let Some(request) = interaction.delete_note {
            self.delete_midi_note(request);
        }
        if let Some(request) = interaction.resize_note {
            self.resize_midi_note(request);
        }
        if let Some(request) = interaction.move_note {
            self.move_midi_note(request);
        }
        if let Some(request) = interaction.write_cc {
            self.write_midi_cc(request);
        }
    }

    fn write_midi_cc(&mut self, request: WriteCcRequest) {
        let Some(mut track) = self.project.tracks.get(request.track_index).cloned() else {
            return;
        };
        let frame = snap_frame(
            request.frame,
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.arrange.snap_enabled,
            self.arrange.snap_grid,
        );
        if let Some(point) = track
            .midi_cc
            .iter_mut()
            .find(|point| point.frame == frame && point.controller == request.controller)
        {
            point.value = request.value;
        } else {
            track.midi_cc.push(MidiControlPoint {
                frame,
                controller: request.controller,
                value: request.value,
            });
            track
                .midi_cc
                .sort_by_key(|point| (point.frame, point.controller));
        }
        self.midi_tools.cc_value = request.value;
        self.apply_edit(EditCommand::ReplaceTrack {
            track_index: request.track_index,
            track,
        });
        self.status_ok(format!(
            "Wrote CC{} = {}",
            request.controller, request.value
        ));
    }

    fn edit_selected_midi_clip(
        &mut self,
        status: &'static str,
        edit: impl FnOnce(&mut Vec<MidiNote>),
    ) {
        let Some((track_index, clip_index)) = self.selected_clip else {
            self.status_error("Select a MIDI part first");
            return;
        };
        let Some(mut clip) = self
            .project
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.get(clip_index))
            .cloned()
        else {
            return;
        };
        let ClipSource::Midi { notes, .. } = &mut clip.source else {
            self.status_error("The selected clip is not a MIDI part");
            return;
        };
        edit(notes);
        clip.length_frames = notes
            .iter()
            .map(|note| note.end_frame())
            .max()
            .unwrap_or(1)
            .max(1);
        self.apply_edit(EditCommand::ReplaceClip {
            track_index,
            clip_index,
            clip,
        });
        self.selected_note = None;
        self.status_ok(status);
    }

    fn quantize_selected_midi_clip(&mut self) {
        let options = QuantizeOptions {
            grid_frames: grid_note_frames(
                self.project.sample_rate,
                self.project.tempo_bpm,
                self.arrange.snap_grid,
            ),
            strength: self.midi_tools.quantize_strength,
            quantize_ends: self.midi_tools.quantize_ends,
        };
        self.edit_selected_midi_clip("Quantized MIDI part", |notes| {
            quantize_notes(notes, options);
        });
    }

    fn swing_quantize_selected_midi_clip(&mut self) {
        let options = SwingQuantizeOptions {
            grid_frames: grid_note_frames(
                self.project.sample_rate,
                self.project.tempo_bpm,
                self.arrange.snap_grid,
            ),
            strength: self.midi_tools.quantize_strength,
            swing_percent: self.midi_tools.swing_percent,
            quantize_ends: self.midi_tools.quantize_ends,
        };
        self.edit_selected_midi_clip("Swing-quantized MIDI part", |notes| {
            swing_quantize_notes(notes, options);
        });
    }

    fn groove_quantize_selected_midi_clip(&mut self) {
        let options = GrooveQuantizeOptions {
            grid_frames: grid_note_frames(
                self.project.sample_rate,
                self.project.tempo_bpm,
                self.arrange.snap_grid,
            ),
            strength: self.midi_tools.quantize_strength,
            amount: self.midi_tools.groove_amount,
            template: self.midi_tools.groove_template,
            quantize_ends: self.midi_tools.quantize_ends,
        };
        self.edit_selected_midi_clip("Groove-quantized MIDI part", |notes| {
            groove_quantize_notes(notes, options);
        });
    }

    fn humanize_selected_midi_clip(&mut self) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let timing_frames = (f64::from(self.project.sample_rate)
            * f64::from(self.midi_tools.humanize_timing_ms)
            / 1_000.0)
            .round() as u64;
        let velocity = self.midi_tools.humanize_velocity;
        let seed = self.midi_tools.humanize_seed;
        self.midi_tools.humanize_seed = self.midi_tools.humanize_seed.wrapping_add(1);
        self.edit_selected_midi_clip("Humanized MIDI part", |notes| {
            humanize_notes(notes, timing_frames, velocity, seed);
        });
    }

    fn transpose_selected_midi(&mut self, semitones: i16) {
        self.edit_selected_midi_subset("Transposed MIDI", |notes| {
            transpose_notes(notes, semitones);
        });
    }

    fn adjust_selected_midi_velocity(&mut self, amount: i16) {
        self.edit_selected_midi_subset("Adjusted MIDI velocity", |notes| {
            adjust_note_velocities(notes, amount);
        });
    }

    fn conform_selected_midi_to_scale(&mut self) {
        let root = self.midi_tools.scale_root;
        let scale = self.midi_tools.scale;
        self.edit_selected_midi_subset("Conformed MIDI to scale", |notes| {
            conform_notes_to_scale(notes, root, scale);
        });
    }

    fn set_selected_midi_note_lengths(&mut self) {
        let length_frames = grid_note_frames(
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.midi_tools.note_length_grid,
        );
        self.edit_selected_midi_subset("Set MIDI note lengths", |notes| {
            set_note_lengths(notes, length_frames);
        });
    }

    fn edit_selected_midi_subset(
        &mut self,
        status: &'static str,
        edit: impl FnOnce(&mut [MidiNote]),
    ) {
        let Some((track_index, clip_index)) = self.selected_clip else {
            self.status_error("Select a MIDI part first");
            return;
        };
        let Some(mut clip) = self
            .project
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.get(clip_index))
            .cloned()
        else {
            return;
        };
        let ClipSource::Midi { notes, .. } = &mut clip.source else {
            self.status_error("The selected clip is not a MIDI part");
            return;
        };
        if let Some((selected_track, selected_clip, note_index)) = self.selected_note
            && selected_track == track_index
            && selected_clip == clip_index
            && let Some(note) = notes.get_mut(note_index)
        {
            edit(std::slice::from_mut(note));
        } else {
            edit(notes);
        }
        clip.length_frames = notes
            .iter()
            .map(|note| note.end_frame())
            .max()
            .unwrap_or(1)
            .max(1);
        self.apply_edit(EditCommand::ReplaceClip {
            track_index,
            clip_index,
            clip,
        });
        self.status_ok(status);
    }

    fn delete_midi_note(&mut self, request: DeleteNoteRequest) {
        let Some(mut clip) = self
            .project
            .tracks
            .get(request.track)
            .and_then(|track| track.clips.get(request.clip))
            .cloned()
        else {
            return;
        };
        let ClipSource::Midi { notes, .. } = &mut clip.source else {
            return;
        };
        if request.note >= notes.len() {
            return;
        }
        let removed = notes.remove(request.note);
        if notes.is_empty() {
            self.apply_edit(EditCommand::DeleteClip {
                track_index: request.track,
                clip_index: request.clip,
            });
            self.selected_clip = None;
        } else {
            self.apply_edit(EditCommand::ReplaceClip {
                track_index: request.track,
                clip_index: request.clip,
                clip,
            });
            self.selected_clip = Some((request.track, request.clip));
        }
        self.selected_note = None;
        self.status_ok(format!("Deleted {}", midi_note_name(removed.midi_note)));
    }

    fn resize_midi_note(&mut self, request: ResizeNoteRequest) {
        let Some(mut clip) = self
            .project
            .tracks
            .get(request.track_index)
            .and_then(|track| track.clips.get(request.clip_index))
            .cloned()
        else {
            return;
        };
        let ClipSource::Midi { notes, .. } = &mut clip.source else {
            return;
        };
        let Some(note) = notes.get_mut(request.note_index) else {
            return;
        };
        note.length_frames = request.length_frames.max(1);
        let note_end = note.end_frame();
        let note_name = midi_note_name(note.midi_note);
        clip.length_frames = clip.length_frames.max(note_end);
        self.apply_edit(EditCommand::ReplaceClip {
            track_index: request.track_index,
            clip_index: request.clip_index,
            clip,
        });
        self.selected_track = Some(request.track_index);
        self.selected_clip = Some((request.track_index, request.clip_index));
        self.status_ok(format!("Resized {note_name}"));
    }

    fn move_midi_note(&mut self, request: MoveNoteRequest) {
        let Some(mut clip) = self
            .project
            .tracks
            .get(request.track_index)
            .and_then(|track| track.clips.get(request.clip_index))
            .cloned()
        else {
            return;
        };
        let ClipSource::Midi { notes, .. } = &mut clip.source else {
            return;
        };
        let old_clip_start = clip.start_frame;
        if request.start_frame < old_clip_start {
            let shift = old_clip_start - request.start_frame;
            for note in notes.iter_mut() {
                note.start_frame = note.start_frame.saturating_add(shift);
            }
            clip.start_frame = request.start_frame;
        }
        let Some(note) = notes.get_mut(request.note_index) else {
            return;
        };
        let previous_name = midi_note_name(note.midi_note);
        note.start_frame = request.start_frame.saturating_sub(clip.start_frame);
        note.midi_note = request.midi_note;
        let note_end = note.end_frame();
        let next_name = midi_note_name(note.midi_note);
        clip.length_frames = clip.length_frames.max(note_end);
        notes.sort_by_key(|note| (note.start_frame, note.midi_note));
        self.apply_edit(EditCommand::ReplaceClip {
            track_index: request.track_index,
            clip_index: request.clip_index,
            clip,
        });
        self.selected_track = Some(request.track_index);
        self.selected_clip = Some((request.track_index, request.clip_index));
        self.selected_note = None;
        self.status_ok(format!("Moved {previous_name} → {next_name}"));
    }

    fn add_grid_note(&mut self, track_index: usize, raw_start_frame: u64, midi_note: u8) {
        let Some(track) = self.project.tracks.get(track_index) else {
            self.status_error("The target track no longer exists");
            return;
        };
        let start_frame = snap_frame(
            raw_start_frame,
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.arrange.snap_enabled,
            self.arrange.snap_grid,
        );
        let length_frames = grid_note_frames(
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.arrange.snap_grid,
        );
        let note_name = midi_note_name(midi_note);
        let midi_clip_index = self
            .selected_clip
            .filter(|(selected_track, clip_index)| {
                *selected_track == track_index
                    && track
                        .clips
                        .get(*clip_index)
                        .is_some_and(|clip| matches!(clip.source, ClipSource::Midi { .. }))
            })
            .map(|(_, clip_index)| clip_index)
            .or_else(|| {
                track
                    .clips
                    .iter()
                    .position(|clip| matches!(clip.source, ClipSource::Midi { .. }))
            });

        let clip_index = if let Some(clip_index) = midi_clip_index {
            let mut clip = track.clips[clip_index].clone();
            let old_start = clip.start_frame;
            let old_end = clip.end_frame();
            let new_start = old_start.min(start_frame);
            if let ClipSource::Midi { notes, .. } = &mut clip.source {
                let shift = old_start - new_start;
                for note in notes.iter_mut() {
                    note.start_frame = note.start_frame.saturating_add(shift);
                }
                notes.push(MidiNote {
                    start_frame: start_frame - new_start,
                    length_frames,
                    midi_note,
                    velocity: 100,
                });
                notes.sort_by_key(|note| (note.start_frame, note.midi_note));
            }
            clip.start_frame = new_start;
            clip.length_frames = old_end
                .max(start_frame.saturating_add(length_frames))
                .saturating_sub(new_start);
            self.apply_edit(EditCommand::ReplaceClip {
                track_index,
                clip_index,
                clip,
            });
            clip_index
        } else {
            let clip_index = track.clips.len();
            self.apply_edit(EditCommand::AddClip {
                track_index,
                clip_index,
                clip: Clip {
                    name: "MIDI Part".into(),
                    start_frame,
                    length_frames,
                    gain: 1.0,
                    fade_in_frames: 0,
                    fade_out_frames: 0,
                    fade_curve: FadeCurve::Linear,
                    source: ClipSource::Midi {
                        notes: vec![MidiNote {
                            start_frame: 0,
                            length_frames,
                            midi_note,
                            velocity: 100,
                        }],
                        amplitude: 0.8,
                    },
                },
            });
            clip_index
        };
        self.selected_track = Some(track_index);
        self.selected_clip = Some((track_index, clip_index));
        self.selected_note = None;
        self.piano_roll.scroll_to_selection = true;
        self.seek(start_frame);
        self.status_ok(format!(
            "Added {note_name} at {}",
            self.arrange.snap_grid.label()
        ));
    }

    fn selected_midi_note(&self) -> Option<u8> {
        let (track_index, clip_index) = self.selected_clip?;
        let clip = self
            .project
            .tracks
            .get(track_index)?
            .clips
            .get(clip_index)?;
        match &clip.source {
            ClipSource::Midi { notes, .. } => notes.last().map(|note| note.midi_note),
            ClipSource::Sine { frequency_hz, .. } => Some(frequency_to_midi_note(*frequency_hz)),
            ClipSource::AudioFile { .. } => None,
        }
    }

    fn keyboard_shortcuts(&mut self, ui: &egui::Ui) {
        let shortcuts = ui.input(|input| {
            (
                input.key_pressed(egui::Key::Space),
                input.modifiers.command && input.key_pressed(egui::Key::S),
                input.modifiers.command && input.key_pressed(egui::Key::Z),
                input.modifiers.command && input.key_pressed(egui::Key::Y),
                input.key_pressed(egui::Key::Delete),
                input.key_pressed(egui::Key::Num1),
                input.key_pressed(egui::Key::Num2),
                input.modifiers.command && input.key_pressed(egui::Key::C),
                input.modifiers.command && input.key_pressed(egui::Key::V),
                input.modifiers.command && input.key_pressed(egui::Key::D),
                input.modifiers.command && input.key_pressed(egui::Key::B),
            )
        });
        if shortcuts.0 && !ui.egui_wants_keyboard_input() {
            if self.is_playing() {
                self.pause();
            } else {
                self.play();
            }
        }
        if shortcuts.1 {
            self.perform_action(UiAction::Save);
        }
        if shortcuts.2 {
            self.perform_action(UiAction::Undo);
        }
        if shortcuts.3 {
            self.perform_action(UiAction::Redo);
        }
        if shortcuts.4
            && !ui.egui_wants_keyboard_input()
            && let Some((track_index, clip_index)) = self.selected_clip
        {
            self.apply_edit(EditCommand::DeleteClip {
                track_index,
                clip_index,
            });
            self.selected_clip = None;
        }
        if !ui.egui_wants_keyboard_input() {
            if shortcuts.5 {
                self.arrange.tool = EditTool::Select;
                self.status_ok("Select tool");
            }
            if shortcuts.6 {
                self.arrange.tool = EditTool::Draw;
                self.status_ok("Draw tool — click a track lane to add C4");
            }
            if shortcuts.7 {
                self.copy_selected_clip();
            }
            if shortcuts.8 {
                self.paste_clip();
            }
            if shortcuts.9 {
                self.duplicate_selected_clip();
            }
            if shortcuts.10 {
                self.split_selected_clip();
            }
        }
    }

    fn update_playback(&mut self, ui: &egui::Ui) {
        let Some(playback) = &self.playback else {
            return;
        };
        let handle = playback.handle();
        self.playhead_frame = u64::try_from(handle.position_frames()).unwrap_or(u64::MAX);
        if handle.has_stream_error() {
            self.status = StatusMessage {
                text: "Audio device reported a stream error".into(),
                error: true,
            };
            self.all_midi_notes_off();
            self.playback = None;
            return;
        }
        if handle.is_playing() {
            self.update_midi_output_playback();
            ui.request_repaint_after(std::time::Duration::from_millis(16));
        } else {
            self.all_midi_notes_off();
        }
    }

    fn update_live_midi(&mut self, ui: &egui::Ui) {
        if self.midi_input.connected_name().is_some() {
            ui.request_repaint_after(Duration::from_millis(8));
        }
        for message in self.midi_input.drain() {
            self.last_midi_activity = Some(message.received_at);
            let Some(event) = parse_live_midi(message) else {
                continue;
            };
            self.push_retrospective_midi_event(message.received_at, event);
            let sample_rate = self.project.sample_rate;
            if let Some(recording) = &mut self.recording {
                let frame = recording
                    .capture_start_frame
                    .saturating_add(duration_to_frames(
                        message
                            .received_at
                            .saturating_duration_since(recording.started_at),
                        sample_rate,
                    ));
                let frame = recording.end_frame.map_or(frame, |end| frame.min(end));
                for take in &mut recording.midi_takes {
                    record_midi_event(take, event, frame, recording.record_start_frame);
                }
            }
        }
    }

    fn push_retrospective_midi_event(&mut self, received_at: Instant, event: ParsedLiveMidi) {
        self.retrospective_midi
            .push_back(RetrospectiveMidiEvent { received_at, event });
        prune_retrospective_midi(
            &mut self.retrospective_midi,
            received_at,
            Duration::from_secs(RETROSPECTIVE_MIDI_SECONDS),
        );
    }

    fn update_recording(&mut self, ui: &egui::Ui) {
        let Some(recording) = &self.recording else {
            return;
        };
        let captured_frames = recording.capture.as_ref().map_or_else(
            || elapsed_frames(recording.started_at, self.project.sample_rate),
            |capture| u64::try_from(capture.recorded_frames()).unwrap_or(u64::MAX),
        );
        self.playhead_frame = recording
            .capture_start_frame
            .saturating_add(captured_frames);
        let should_finish = recording
            .capture
            .as_ref()
            .is_some_and(AudioRecording::has_stream_error)
            || recording
                .end_frame
                .is_some_and(|end_frame| self.playhead_frame >= end_frame);
        ui.request_repaint_after(std::time::Duration::from_millis(16));
        if should_finish {
            self.finish_recording();
        }
    }

    #[allow(clippy::too_many_lines)]
    fn start_recording(&mut self) {
        let armed_tracks = self
            .project
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(index, track)| track.recording.armed.then_some(index))
            .collect::<Vec<_>>();
        if armed_tracks.is_empty() {
            self.status_error("Arm at least one track with R before recording");
            return;
        }
        let audio_takes = armed_tracks
            .iter()
            .copied()
            .filter_map(|track_index| {
                let input = self.project.tracks[track_index].input;
                input
                    .is_audio()
                    .then_some(ActiveAudioTake { track_index, input })
            })
            .collect::<Vec<_>>();
        let midi_takes = armed_tracks
            .iter()
            .copied()
            .filter_map(|track_index| {
                let input = self.project.tracks[track_index].input;
                input.is_midi().then_some(ActiveMidiTake {
                    track_index,
                    input,
                    active_notes: HashMap::new(),
                    notes: Vec::new(),
                    controllers: Vec::new(),
                    pitch_bend: Vec::new(),
                    channel_pressure: Vec::new(),
                    poly_pressure: Vec::new(),
                    program_changes: Vec::new(),
                })
            })
            .collect::<Vec<_>>();
        if !midi_takes.is_empty() && self.midi_input.connected_name().is_none() {
            self.status_error("Connect a MIDI input from the MIDI menu before recording");
            return;
        }
        if self.recording_options.punch_enabled && !self.loop_enabled {
            self.status_error("Enable Loop and set L/R before punch recording");
            return;
        }
        if self.recording_options.cycle_take_recording && !self.loop_enabled {
            self.status_error("Enable Loop and set L/R before cycle take recording");
            return;
        }
        if self.recording_options.punch_enabled && self.recording_options.cycle_take_recording {
            self.status_error("Use Punch or Cycle Takes, not both at once");
            return;
        }
        let monitoring = audio_takes.iter().any(|take| {
            self.project.tracks[take.track_index]
                .recording
                .input_monitoring
        });
        let record_start_frame = if self.recording_options.punch_enabled
            || self.recording_options.cycle_take_recording
        {
            self.loop_start_frame
        } else {
            self.playhead_frame
        };
        let end_frame = self
            .recording_options
            .punch_enabled
            .then_some(self.loop_end_frame);
        let requested_pre_roll = bars_to_frames(
            self.recording_options.pre_roll_bars,
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.project.time_signature,
        );
        let capture_start_frame = record_start_frame.saturating_sub(requested_pre_roll);
        self.invalidate_playback();
        let prepared_playback =
            if self.project.duration_frames() > capture_start_frame || self.metronome_enabled {
                match self.prepare_playback_at(capture_start_frame) {
                    Ok(playback) => Some(playback),
                    Err(error) => {
                        self.status_error(error);
                        return;
                    }
                }
            } else {
                None
            };
        let capture = if audio_takes.is_empty() {
            None
        } else {
            match AudioRecording::start_on_device(
                self.project.sample_rate,
                monitoring,
                self.selected_audio_input_device.as_deref(),
            ) {
                Ok(capture) => Some(capture),
                Err(error) => {
                    self.status_error(error);
                    return;
                }
            }
        };
        let source = match (&capture, midi_takes.is_empty()) {
            (Some(capture), false) => format!(
                "{} + {}",
                capture.device_info().name,
                self.midi_input.connected_name().unwrap_or("MIDI")
            ),
            (Some(capture), true) => capture.device_info().name.clone(),
            (None, _) => self
                .midi_input
                .connected_name()
                .unwrap_or("MIDI")
                .to_owned(),
        };
        self.playhead_frame = capture_start_frame;
        self.recording = Some(ActiveRecording {
            capture,
            audio_takes,
            midi_takes,
            started_at: Instant::now(),
            capture_start_frame,
            record_start_frame,
            end_frame,
        });
        self.playback = prepared_playback;
        if let Some(playback) = &self.playback {
            let handle = playback.handle();
            if self.recording_options.cycle_take_recording {
                let start = usize::try_from(self.loop_start_frame).unwrap_or(0);
                let end = usize::try_from(self.loop_end_frame).unwrap_or(handle.duration_frames());
                let _ = handle.set_loop(start, end.min(handle.duration_frames()));
            } else {
                handle.clear_loop();
            }
            handle.play();
        }
        let pre_roll_bars = self.recording_options.pre_roll_bars;
        self.status_ok(if self.recording_options.punch_enabled {
            format!("Punch recording from {source} · pre-roll {pre_roll_bars} bar(s)")
        } else if self.recording_options.cycle_take_recording {
            format!("Cycle-take recording from {source} · pre-roll {pre_roll_bars} bar(s)")
        } else {
            format!("Recording from {source} · pre-roll {pre_roll_bars} bar(s)")
        });
    }

    #[allow(clippy::too_many_lines)]
    fn finish_recording(&mut self) {
        let Some(mut recording) = self.recording.take() else {
            return;
        };
        if let Some(playback) = self.playback.take() {
            playback.handle().pause();
        }
        let mut first_selection = None;
        let mut recorded_sources = Vec::new();
        let stream_failed = recording
            .capture
            .as_ref()
            .is_some_and(AudioRecording::has_stream_error);
        let finish_frame = recording
            .end_frame
            .unwrap_or(self.playhead_frame)
            .max(recording.record_start_frame.saturating_add(1));

        if let Some(capture) = recording.capture.take() {
            let mut captured = capture.finish();
            let pre_roll_frames = usize::try_from(
                recording
                    .record_start_frame
                    .saturating_sub(recording.capture_start_frame),
            )
            .unwrap_or(usize::MAX);
            discard_stereo_prefix(&mut captured.samples, pre_roll_frames);
            let maximum_frames =
                usize::try_from(finish_frame.saturating_sub(recording.record_start_frame))
                    .unwrap_or(usize::MAX);
            truncate_stereo_recording(&mut captured.samples, maximum_frames);
            let frame_count = captured.samples.len() / 2;
            if frame_count > 0 {
                let length_frames = u64::try_from(frame_count).unwrap_or(u64::MAX);
                let mut wrote_audio = false;
                for input in [
                    TrackInput::Audio,
                    TrackInput::AudioMonoLeft,
                    TrackInput::AudioMonoRight,
                ] {
                    let track_indices = recording
                        .audio_takes
                        .iter()
                        .filter(|take| take.input == input)
                        .map(|take| take.track_index)
                        .collect::<Vec<_>>();
                    let Some(first_track_index) = track_indices.first().copied() else {
                        continue;
                    };
                    let route_name = format!(
                        "{}_{}",
                        self.project.tracks[first_track_index].name,
                        audio_input_file_suffix(input)
                    );
                    let path = match recording_output_path(
                        self.project_path.as_deref(),
                        &self.project.name,
                        &route_name,
                    ) {
                        Ok(path) => path,
                        Err(error) => {
                            self.status_error(error);
                            return;
                        }
                    };
                    let routed_samples = routed_stereo_input(&captured.samples, input);
                    if let Err(error) = write_stereo_i16_wav(
                        &path,
                        captured.device_info.sample_rate,
                        routed_samples.as_ref(),
                    ) {
                        self.status_error(error);
                        return;
                    }
                    let overview = match decode_wav_overview(&path, WAVEFORM_PEAKS) {
                        Ok(overview) => overview,
                        Err(error) => {
                            self.status_error(error);
                            return;
                        }
                    };
                    let stored_path = path.to_string_lossy().into_owned();
                    self.cache_waveform(&stored_path, &path, overview);
                    let clip_name = path.file_stem().map_or_else(
                        || "Recorded Take".into(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                    for track_index in track_indices {
                        let mut track = self.project.tracks[track_index].clone();
                        let clip_index = if self.recording_options.cycle_take_recording {
                            comp_loop_recording_into_track(
                                &mut track,
                                LoopRecordingSource {
                                    base_name: &clip_name,
                                    path: &stored_path,
                                    source_sample_rate: captured.device_info.sample_rate,
                                    project_sample_rate: self.project.sample_rate,
                                    channels: 2,
                                    record_start_frame: recording.record_start_frame,
                                    recorded_length_frames: length_frames,
                                    loop_start_frame: self.loop_start_frame,
                                    loop_end_frame: self.loop_end_frame,
                                },
                            )
                        } else {
                            let clip = Clip {
                                name: clip_name.clone(),
                                start_frame: recording.record_start_frame,
                                length_frames,
                                gain: 1.0,
                                fade_in_frames: 0,
                                fade_out_frames: 0,
                                fade_curve: FadeCurve::Linear,
                                source: ClipSource::AudioFile {
                                    path: stored_path.clone(),
                                    source_offset_frames: 0,
                                    source_sample_rate: captured.device_info.sample_rate,
                                    channels: 2,
                                    reversed: false,
                                },
                            };
                            comp_recording_into_track(&mut track, clip, self.project.sample_rate)
                        };
                        self.apply_edit(EditCommand::ReplaceTrack { track_index, track });
                        first_selection.get_or_insert((track_index, clip_index));
                    }
                    wrote_audio = true;
                }
                if wrote_audio {
                    recorded_sources.push("audio");
                }
            }
        }

        for mut take in recording.midi_takes {
            finish_active_midi_notes(&mut take, finish_frame, recording.record_start_frame);
            if take.notes.is_empty()
                && take.controllers.is_empty()
                && take.pitch_bend.is_empty()
                && take.channel_pressure.is_empty()
                && take.poly_pressure.is_empty()
                && take.program_changes.is_empty()
            {
                continue;
            }
            let track_index = take.track_index;
            let mut track = self.project.tracks[track_index].clone();
            let clip_index = track.clips.len();
            take.notes
                .sort_by_key(|note| (note.start_frame, note.midi_note));
            track.midi_cc.append(&mut take.controllers);
            track
                .midi_cc
                .sort_by_key(|point| (point.frame, point.controller));
            track.midi_pitch_bend.append(&mut take.pitch_bend);
            track.midi_pitch_bend.sort_by_key(|point| point.frame);
            track
                .midi_channel_pressure
                .append(&mut take.channel_pressure);
            track.midi_channel_pressure.sort_by_key(|point| point.frame);
            track.midi_poly_pressure.append(&mut take.poly_pressure);
            track
                .midi_poly_pressure
                .sort_by_key(|point| (point.frame, point.note));
            track.midi_program_changes.append(&mut take.program_changes);
            track.midi_program_changes.sort_by_key(|point| point.frame);
            track.clips.push(Clip {
                name: format!("MIDI Take {}", clip_index + 1),
                start_frame: recording.record_start_frame,
                length_frames: finish_frame.saturating_sub(recording.record_start_frame),
                gain: 1.0,
                fade_in_frames: 0,
                fade_out_frames: 0,
                fade_curve: FadeCurve::Linear,
                source: ClipSource::Midi {
                    notes: take
                        .notes
                        .into_iter()
                        .map(|note| MidiNote {
                            start_frame: note
                                .start_frame
                                .saturating_sub(recording.record_start_frame),
                            length_frames: note.length_frames,
                            midi_note: note.midi_note,
                            velocity: note.velocity,
                        })
                        .collect(),
                    amplitude: 0.8,
                },
            });
            self.apply_edit(EditCommand::ReplaceTrack { track_index, track });
            first_selection.get_or_insert((track_index, clip_index));
            recorded_sources.push("MIDI");
        }

        self.selected_clip = first_selection;
        self.selected_track = first_selection.map(|(track_index, _)| track_index);
        self.selected_note = first_note_selection(&self.project, self.selected_clip);
        self.playhead_frame = finish_frame;
        if stream_failed {
            self.status_error("Audio input failed; any captured partial takes were kept");
        } else if recorded_sources.is_empty() {
            self.status_error("Recording stopped before any input events were captured");
        } else {
            recorded_sources.sort_unstable();
            recorded_sources.dedup();
            self.status_ok(format!("Recorded {} take", recorded_sources.join(" + ")));
        }
    }

    fn play(&mut self) {
        if let Some(playback) = &self.playback {
            let handle = playback.handle();
            if handle.position_frames() < handle.duration_frames() && !handle.has_stream_error() {
                handle.play();
                self.status_ok("Playback resumed");
                return;
            }
        }

        let samples = match self.render_playback_samples(self.playhead_frame) {
            Ok(samples) => samples,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        if samples.is_empty() {
            self.status_error("The project has no audible timeline content");
            return;
        }
        let duration_frames = samples.len() / 2;
        let start_frame = usize::try_from(self.playhead_frame)
            .unwrap_or(duration_frames)
            .min(duration_frames.saturating_sub(1));
        match Playback::start_at(samples, self.project.sample_rate, start_frame) {
            Ok(playback) => {
                let device = playback.device_info().clone();
                self.playback = Some(playback);
                self.sync_playback_loop();
                self.status_ok(format!(
                    "Playing through {} · {} Hz · {}",
                    device.name, device.sample_rate, device.sample_format
                ));
            }
            Err(error) => self.status_error(error),
        }
    }

    fn render_playback_samples(&self, start_frame: u64) -> Result<Vec<f32>, String> {
        let mut samples = try_render_stereo(&self.project).map_err(|error| error.to_string())?;
        if self.metronome_enabled {
            let preview_frames =
                metronome_preview_frames(self.project.sample_rate, self.project.tempo_bpm, 16);
            let start = usize::try_from(start_frame)
                .map_err(|_| "Playback position is too large".to_owned())?;
            let minimum_frames = start.saturating_add(preview_frames);
            let minimum_samples = minimum_frames.saturating_mul(2);
            if samples.len() < minimum_samples {
                samples
                    .try_reserve_exact(minimum_samples - samples.len())
                    .map_err(|_| "Metronome preview is too large".to_owned())?;
                samples.resize(minimum_samples, 0.0);
            }
            mix_metronome(
                &mut samples,
                self.project.sample_rate,
                self.project.tempo_bpm,
            );
        }
        Ok(samples)
    }

    fn prepare_playback_at(&self, frame: u64) -> Result<Playback, String> {
        let samples = self.render_playback_samples(frame)?;
        if samples.is_empty() {
            return Err("The project has no audible timeline content".into());
        }
        let duration_frames = samples.len() / 2;
        let start_frame = usize::try_from(frame)
            .unwrap_or(duration_frames)
            .min(duration_frames.saturating_sub(1));
        Playback::start_at_paused(samples, self.project.sample_rate, start_frame)
            .map_err(|error| error.to_string())
    }

    fn pause(&mut self) {
        if let Some(playback) = &self.playback {
            playback.handle().pause();
            self.status_ok("Playback paused");
        }
    }

    fn stop(&mut self) {
        if self.recording.is_some() {
            self.finish_recording();
        }
        if let Some(playback) = self.playback.take() {
            playback.handle().stop();
        }
        self.playhead_frame = 0;
        self.status_ok("Playback stopped");
    }

    fn seek(&mut self, frame: u64) {
        self.playhead_frame = frame;
        if let Some(playback) = &self.playback {
            let handle = playback.handle();
            let target = usize::try_from(frame)
                .unwrap_or(handle.duration_frames())
                .min(handle.duration_frames());
            if let Err(error) = handle.seek(target) {
                self.status_error(error);
            }
        }
    }

    fn is_playing(&self) -> bool {
        self.playback
            .as_ref()
            .is_some_and(|playback| playback.handle().is_playing())
    }

    fn sync_playback_loop(&mut self) {
        let Some(playback) = &self.playback else {
            return;
        };
        let handle = playback.handle();
        if !self.loop_enabled {
            handle.clear_loop();
            return;
        }
        let start = usize::try_from(self.loop_start_frame).unwrap_or(0);
        let end = usize::try_from(self.loop_end_frame).unwrap_or(handle.duration_frames());
        if let Err(error) = handle.set_loop(start, end.min(handle.duration_frames())) {
            self.status_error(error);
            self.loop_enabled = false;
        }
    }

    fn set_cycle_range(&mut self, start_frame: u64, end_frame: u64) {
        let Some(cycle_range) = CycleRange::new(start_frame, end_frame) else {
            self.status_error("Loop range must be at least one frame");
            return;
        };
        self.loop_start_frame = cycle_range.start_frame;
        self.loop_end_frame = cycle_range.end_frame;
        self.loop_enabled = true;
        self.apply_edit(EditCommand::SetCycleRange {
            cycle_range: Some(cycle_range),
        });
        self.sync_playback_loop();
    }

    fn write_cycle_range_from_transport(&mut self) {
        let cycle_range = self
            .loop_enabled
            .then(|| CycleRange::new(self.loop_start_frame, self.loop_end_frame))
            .flatten();
        self.apply_edit(EditCommand::SetCycleRange { cycle_range });
    }

    fn add_marker_at_playhead(&mut self) {
        let marker_number = self.project.markers.len() + 1;
        let marker = Marker::new(format!("Marker {marker_number}"), self.playhead_frame);
        let index = marker_insert_index(&self.project.markers, marker.frame);
        self.apply_edit(EditCommand::AddMarker { index, marker });
        self.status_ok("Marker added");
    }

    fn seek_previous_marker(&mut self) {
        let Some(marker) = self
            .project
            .markers
            .iter()
            .filter(|marker| marker.frame < self.playhead_frame)
            .max_by_key(|marker| marker.frame)
            .cloned()
        else {
            self.status_error("No previous marker");
            return;
        };
        self.seek(marker.frame);
        self.status_ok(format!("Marker: {}", marker.name));
    }

    fn seek_next_marker(&mut self) {
        let Some(marker) = self
            .project
            .markers
            .iter()
            .filter(|marker| marker.frame > self.playhead_frame)
            .min_by_key(|marker| marker.frame)
            .cloned()
        else {
            self.status_error("No next marker");
            return;
        };
        self.seek(marker.frame);
        self.status_ok(format!("Marker: {}", marker.name));
    }

    fn add_arranger_section_at_playhead(&mut self) {
        let section_number = self.project.arranger_sections.len() + 1;
        let length_frames = bars_to_frames(
            4,
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.project.time_signature,
        )
        .max(grid_note_frames(
            self.project.sample_rate,
            self.project.tempo_bpm,
            SnapGrid::Quarter,
        ));
        let mut section = ArrangerSection::new(
            format!("Section {section_number}"),
            self.playhead_frame,
            length_frames,
        );
        section.color_index = u8::try_from(section_number % ARRANGER_COLORS.len()).unwrap_or(0);
        let index = arranger_insert_index(&self.project.arranger_sections, section.start_frame);
        self.apply_edit(EditCommand::AddArrangerSection { index, section });
        self.status_ok("Arranger section added");
    }

    fn arranger_inspector(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Arranger", |ui| {
            ui.horizontal(|ui| {
                if ui.button("+ Section").clicked() {
                    self.add_arranger_section_at_playhead();
                }
                ui.small(format!(
                    "{} section(s)",
                    self.project.arranger_sections.len()
                ));
            });
            let mut delete_section = None;
            let mut seek_section = None;
            for section_index in 0..self.project.arranger_sections.len() {
                let original = self.project.arranger_sections[section_index].clone();
                let mut edited = original.clone();
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut edited.name);
                        if ui.small_button("Go").clicked() {
                            seek_section = Some(edited.start_frame);
                        }
                        if ui
                            .small_button("×")
                            .on_hover_text("Delete arranger section")
                            .clicked()
                        {
                            delete_section = Some(section_index);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Start");
                        let mut start_seconds =
                            frames_to_seconds(edited.start_frame, self.project.sample_rate);
                        if ui
                            .add(
                                egui::DragValue::new(&mut start_seconds)
                                    .range(0.0..=3_600.0)
                                    .speed(0.01)
                                    .fixed_decimals(3)
                                    .suffix(" s"),
                            )
                            .changed()
                        {
                            edited.start_frame = self.project.seconds_to_frames(start_seconds);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Length");
                        let mut length_seconds =
                            frames_to_seconds(edited.length_frames, self.project.sample_rate);
                        if ui
                            .add(
                                egui::DragValue::new(&mut length_seconds)
                                    .range(0.001..=3_600.0)
                                    .speed(0.01)
                                    .fixed_decimals(3)
                                    .suffix(" s"),
                            )
                            .changed()
                        {
                            edited.length_frames =
                                self.project.seconds_to_frames(length_seconds).max(1);
                        }
                        color_swatch(ui, arranger_color(edited.color_index));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Color");
                        egui::ComboBox::from_id_salt(("arranger_color", section_index))
                            .width(72.0)
                            .selected_text(format!("{}", edited.color_index))
                            .show_ui(ui, |ui| {
                                for index in 0..ARRANGER_COLORS.len() {
                                    ui.selectable_value(
                                        &mut edited.color_index,
                                        u8::try_from(index).unwrap_or(0),
                                        format!("Color {index}"),
                                    );
                                }
                            });
                    });
                });
                if edited != original {
                    self.apply_edit(EditCommand::ReplaceArrangerSection {
                        index: section_index,
                        section: edited,
                    });
                }
            }
            if let Some(index) = delete_section {
                self.apply_edit(EditCommand::DeleteArrangerSection { index });
            }
            if let Some(frame) = seek_section {
                self.seek(frame);
            }
        });
    }

    fn marker_inspector(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Markers", |ui| {
            ui.horizontal(|ui| {
                if ui.button("+ Marker").clicked() {
                    self.add_marker_at_playhead();
                }
                ui.small(format!("{} marker(s)", self.project.markers.len()));
            });
            let mut delete_marker = None;
            let mut seek_marker = None;
            for marker_index in 0..self.project.markers.len() {
                let original = self.project.markers[marker_index].clone();
                let mut edited = original.clone();
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut edited.name);
                        if ui.small_button("Go").clicked() {
                            seek_marker = Some(edited.frame);
                        }
                        if ui
                            .small_button("×")
                            .on_hover_text("Delete marker")
                            .clicked()
                        {
                            delete_marker = Some(marker_index);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Position");
                        let mut seconds = frames_to_seconds(edited.frame, self.project.sample_rate);
                        if ui
                            .add(
                                egui::DragValue::new(&mut seconds)
                                    .range(0.0..=3_600.0)
                                    .speed(0.01)
                                    .fixed_decimals(3)
                                    .suffix(" s"),
                            )
                            .changed()
                        {
                            edited.frame = self.project.seconds_to_frames(seconds);
                        }
                        ui.small(format_musical_position(
                            edited.frame,
                            self.project.sample_rate,
                            self.project.tempo_bpm,
                            self.project.time_signature,
                        ));
                    });
                });
                if edited != original {
                    self.apply_edit(EditCommand::ReplaceMarker {
                        index: marker_index,
                        marker: edited,
                    });
                }
            }
            if let Some(index) = delete_marker {
                self.apply_edit(EditCommand::DeleteMarker { index });
            }
            if let Some(frame) = seek_marker {
                self.seek(frame);
            }
        });
    }

    fn mixer_inspector(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Mixer", |ui| {
            let mut master_inserts = self.project.master_inserts.clone();
            ui.label("Master inserts");
            insert_chain_editor(ui, "master_inserts", &mut master_inserts);
            if master_inserts != self.project.master_inserts {
                self.apply_edit(EditCommand::SetMasterInserts {
                    inserts: master_inserts,
                });
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.strong("Buses");
                if ui.button("+ Bus").clicked() {
                    let index = self.project.buses.len();
                    self.apply_edit(EditCommand::AddBus {
                        index,
                        bus: Bus::new(format!("Bus {}", index + 1)),
                    });
                }
            });

            let mut delete_bus = None;
            for bus_index in 0..self.project.buses.len() {
                let original = self.project.buses[bus_index].clone();
                let mut edited = original.clone();
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut edited.name);
                        if ui.small_button("×").on_hover_text("Delete bus").clicked() {
                            delete_bus = Some(bus_index);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.toggle_value(&mut edited.muted, "M");
                        ui.toggle_value(&mut edited.soloed, "S");
                        ui.label("Gain");
                        ui.add_sized(
                            [82.0, 18.0],
                            egui::Slider::new(&mut edited.gain, 0.0..=1.5).show_value(false),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Pan");
                        ui.add_sized(
                            [128.0, 18.0],
                            egui::Slider::new(&mut edited.pan, -1.0..=1.0).show_value(false),
                        );
                    });
                    insert_chain_editor(ui, ("bus_inserts", bus_index), &mut edited.inserts);
                });
                if edited != original {
                    self.apply_edit(EditCommand::ReplaceBus {
                        index: bus_index,
                        bus: edited,
                    });
                }
            }
            if let Some(index) = delete_bus {
                self.apply_edit(EditCommand::DeleteBus { index });
            }
        });
    }

    fn add_tone_clip(&mut self) {
        let Some(track_index) = self.selected_track else {
            self.status_error("Select a track before adding a clip");
            return;
        };
        self.add_grid_note(track_index, self.playhead_frame, 69);
    }

    fn capture_retrospective_midi(&mut self) {
        let now = Instant::now();
        prune_retrospective_midi(
            &mut self.retrospective_midi,
            now,
            Duration::from_secs(RETROSPECTIVE_MIDI_SECONDS),
        );
        let Some(mut clip) = retrospective_midi_clip(
            &self.retrospective_midi,
            now,
            Duration::from_secs(RETROSPECTIVE_MIDI_SECONDS),
            self.project.sample_rate,
        ) else {
            self.status_error("No complete recent MIDI notes to capture");
            return;
        };
        let track_index = self
            .selected_track
            .filter(|index| *index < self.project.tracks.len())
            .unwrap_or_else(|| {
                let index = self.project.tracks.len();
                let mut track = Track::new(format!("MIDI Track {}", index + 1));
                track.input = TrackInput::MidiOmni;
                self.apply_edit(EditCommand::AddTrack { index, track });
                self.selected_track = Some(index);
                index
            });
        clip.start_frame = snap_frame(
            self.playhead_frame.saturating_sub(clip.length_frames),
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.arrange.snap_enabled,
            self.arrange.snap_grid,
        );
        let clip_index = self.project.tracks[track_index].clips.len();
        self.apply_edit(EditCommand::AddClip {
            track_index,
            clip_index,
            clip,
        });
        self.selected_track = Some(track_index);
        self.selected_clip = Some((track_index, clip_index));
        self.selected_note = first_note_selection(&self.project, self.selected_clip);
        self.piano_roll.open = true;
        self.piano_roll.scroll_to_selection = true;
        self.status_ok("Captured retrospective MIDI");
    }

    fn import_wav_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav", "wave"])
            .set_title("Import WAV audio")
            .pick_file()
        else {
            return;
        };

        let imported = match import_wav_with_overview(&path, WAVEFORM_PEAKS) {
            Ok(imported) => imported,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        let length_frames = match imported.project_length_frames(self.project.sample_rate) {
            Ok(frames) => frames.max(1),
            Err(error) => {
                self.status_error(error);
                return;
            }
        };

        let track_index = self
            .selected_track
            .filter(|index| *index < self.project.tracks.len())
            .unwrap_or_else(|| {
                let index = self.project.tracks.len();
                self.apply_edit(EditCommand::AddTrack {
                    index,
                    track: Track::new(format!("Track {}", index + 1)),
                });
                self.selected_track = Some(index);
                index
            });
        let clip_index = self.project.tracks[track_index].clips.len();
        let stored_path = match &imported.source {
            ClipSource::AudioFile { path, .. } => path.clone(),
            ClipSource::Midi { .. } | ClipSource::Sine { .. } => {
                unreachable!("WAV import always creates an audio source")
            }
        };
        let name = path.file_stem().map_or_else(
            || "Imported audio".into(),
            |name| name.to_string_lossy().into_owned(),
        );
        let clip = Clip {
            name,
            start_frame: snap_frame(
                self.playhead_frame,
                self.project.sample_rate,
                self.project.tempo_bpm,
                self.arrange.snap_enabled,
                self.arrange.snap_grid,
            ),
            length_frames,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: imported.source,
        };
        self.cache_waveform(&stored_path, &path, imported.overview);
        self.waveform_errors.remove(&stored_path);
        self.apply_edit(EditCommand::AddClip {
            track_index,
            clip_index,
            clip,
        });
        self.selected_track = Some(track_index);
        self.selected_clip = Some((track_index, clip_index));
        self.status_ok(format!("Imported {}", path.display()));
    }

    fn import_soundfont_for_track(&mut self, track_index: usize) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SoundFont", &["sf2"])
            .set_title("Load SoundFont instrument")
            .pick_file()
        else {
            return;
        };
        let info = match inspect_soundfont(&path) {
            Ok(info) => info,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        let Some(first_preset) = info.presets.first().cloned() else {
            self.status_error("SoundFont has no selectable presets");
            return;
        };
        let count = info.presets.len();
        self.soundfont_presets
            .insert(first_preset.path.clone(), info.presets);
        let Some(track) = self.project.tracks.get(track_index).cloned() else {
            return;
        };
        let mut edited = track;
        edited.soundfont = Some(first_preset.clone());
        self.apply_edit(EditCommand::ReplaceTrack {
            track_index,
            track: edited,
        });
        self.status_ok(format!(
            "Loaded {count} SoundFont preset(s): {}",
            first_preset.name
        ));
    }

    fn relink_selected_audio_clip(&mut self) {
        let Some((track_index, clip_index)) = self.selected_clip else {
            self.status_error("Select an audio clip before relinking");
            return;
        };
        let Some(current_clip) = self
            .project
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.get(clip_index))
            .cloned()
        else {
            self.status_error("Selected clip no longer exists");
            return;
        };
        if !matches!(current_clip.source, ClipSource::AudioFile { .. }) {
            self.status_error("Relink is only available for imported audio clips");
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav", "wave"])
            .set_title("Relink WAV audio")
            .pick_file()
        else {
            return;
        };

        let imported = match import_wav_with_overview(&path, WAVEFORM_PEAKS) {
            Ok(imported) => imported,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        let stored_path = match &imported.source {
            ClipSource::AudioFile { path, .. } => path.clone(),
            ClipSource::Midi { .. } | ClipSource::Sine { .. } => {
                unreachable!("WAV import always creates an audio source")
            }
        };
        let old_offset = match current_clip.source {
            ClipSource::AudioFile {
                source_offset_frames,
                ..
            } => source_offset_frames,
            ClipSource::Midi { .. } | ClipSource::Sine { .. } => 0,
        };
        let new_offset = old_offset.min(imported.info.frames.saturating_sub(1));
        let mut relinked = current_clip;
        relinked.source = ClipSource::AudioFile {
            path: stored_path.clone(),
            source_offset_frames: new_offset,
            source_sample_rate: imported.info.sample_rate,
            channels: imported.info.channels,
            reversed: false,
        };
        self.cache_waveform(&stored_path, &path, imported.overview);
        self.apply_edit(EditCommand::ReplaceClip {
            track_index,
            clip_index,
            clip: relinked,
        });
        self.refresh_waveforms();
        self.status_ok(format!("Relinked WAV {}", path.display()));
    }

    fn normalize_selected_audio_clip(&mut self) {
        let Some((track_index, clip_index)) = self.selected_clip else {
            self.status_error("Select an audio clip before normalizing");
            return;
        };
        let Some(clip) = self
            .project
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.get(clip_index))
            .cloned()
        else {
            self.status_error("Selected clip no longer exists");
            return;
        };
        let ClipSource::AudioFile {
            path,
            source_offset_frames,
            source_sample_rate,
            ..
        } = &clip.source
        else {
            self.status_error("Normalize is only available for imported audio clips");
            return;
        };
        let resolved = self.resolve_audio_path(path);
        let decoded = match decode_wav(&resolved) {
            Ok(decoded) => decoded,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        let Some(gain) = normalized_clip_gain(
            &decoded.samples,
            *source_offset_frames,
            clip.length_frames,
            self.project.sample_rate,
            *source_sample_rate,
        ) else {
            self.status_error("Selected audio range is silent");
            return;
        };
        let mut normalized = clip;
        normalized.gain = gain;
        self.apply_edit(EditCommand::ReplaceClip {
            track_index,
            clip_index,
            clip: normalized,
        });
        self.status_ok(format!("Normalized clip gain to {}", format_gain_db(gain)));
    }

    #[allow(clippy::too_many_lines)]
    fn import_midi_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Standard MIDI File", &["mid", "midi"])
            .set_title("Import MIDI")
            .pick_file()
        else {
            return;
        };
        let imported = match load_midi(&path, self.project.sample_rate, self.project.tempo_bpm) {
            Ok(imported) => imported,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        if imported.tracks.is_empty() {
            self.status_error("The MIDI file contains no note or CC events");
            return;
        }
        if let Some(tempo_bpm) = imported.tempo_bpm
            && (20.0..=400.0).contains(&tempo_bpm)
        {
            self.apply_edit(EditCommand::SetTempo { tempo_bpm });
        }
        let insertion_frame = snap_frame(
            self.playhead_frame,
            self.project.sample_rate,
            self.project.tempo_bpm,
            self.arrange.snap_enabled,
            self.arrange.snap_grid,
        );
        let first_track = self.project.tracks.len();
        let track_count = imported.tracks.len();
        for imported_track in imported.tracks {
            let mut track = Track::new(imported_track.name.clone());
            track.midi_channel = imported_track.channel;
            track.input = TrackInput::MidiChannel(imported_track.channel);
            track.midi_cc = imported_track
                .controllers
                .into_iter()
                .map(|point| MidiControlPoint {
                    frame: insertion_frame.saturating_add(point.frame),
                    ..point
                })
                .collect();
            track.midi_pitch_bend = imported_track
                .pitch_bend
                .into_iter()
                .map(|point| MidiPitchBendPoint {
                    frame: insertion_frame.saturating_add(point.frame),
                    ..point
                })
                .collect();
            track.midi_channel_pressure = imported_track
                .channel_pressure
                .into_iter()
                .map(|point| MidiChannelPressurePoint {
                    frame: insertion_frame.saturating_add(point.frame),
                    ..point
                })
                .collect();
            track.midi_poly_pressure = imported_track
                .poly_pressure
                .into_iter()
                .map(|point| MidiPolyPressurePoint {
                    frame: insertion_frame.saturating_add(point.frame),
                    ..point
                })
                .collect();
            track.midi_program_changes = imported_track
                .program_changes
                .into_iter()
                .map(|point| MidiProgramChangePoint {
                    frame: insertion_frame.saturating_add(point.frame),
                    ..point
                })
                .collect();
            if !imported_track.notes.is_empty() {
                let length_frames = imported_track
                    .notes
                    .iter()
                    .map(|note| note.end_frame())
                    .max()
                    .unwrap_or(imported.duration_frames)
                    .max(1);
                track.clips.push(Clip {
                    name: format!("{} Part", imported_track.name),
                    start_frame: insertion_frame,
                    length_frames,
                    gain: 1.0,
                    fade_in_frames: 0,
                    fade_out_frames: 0,
                    fade_curve: FadeCurve::Linear,
                    source: ClipSource::Midi {
                        notes: imported_track.notes,
                        amplitude: 0.8,
                    },
                });
            }
            let index = self.project.tracks.len();
            self.apply_edit(EditCommand::AddTrack { index, track });
        }
        self.selected_track = Some(first_track);
        self.selected_clip =
            (!self.project.tracks[first_track].clips.is_empty()).then_some((first_track, 0));
        self.selected_note = first_note_selection(&self.project, self.selected_clip);
        self.piano_roll.open = true;
        self.piano_roll.scroll_to_selection = true;
        self.status_ok(format!(
            "Imported {track_count} MIDI track(s) from {}",
            path.display()
        ));
    }

    fn export_midi_dialog(&mut self) {
        let suggested = self
            .project_path
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|name| name.to_str())
            .map_or_else(|| "untitled.mid".into(), |name| format!("{name}.mid"));
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Standard MIDI File", &["mid", "midi"])
            .set_file_name(suggested)
            .set_title("Export MIDI")
            .save_file()
        else {
            return;
        };
        let path = with_extension_if_missing(path, "mid");
        match save_project_midi(&path, &self.project) {
            Ok(()) => self.status_ok(format!("Exported MIDI {}", path.display())),
            Err(error) => self.status_error(error),
        }
    }

    fn apply_edit(&mut self, command: EditCommand) {
        match self.history.apply(&mut self.project, command) {
            Ok(()) => {
                self.dirty = true;
                self.invalidate_playback();
                self.status_ok("Project edited");
            }
            Err(error) => self.status_error(error),
        }
    }

    fn invalidate_playback(&mut self) {
        self.all_midi_notes_off();
        if let Some(playback) = self.playback.take() {
            playback.handle().pause();
        }
    }

    fn update_midi_output_playback(&mut self) {
        if self.midi_output.connected_name().is_none() {
            return;
        }
        let current_frame = self.playhead_frame;
        if current_frame < self.midi_output_playback.last_frame {
            self.all_midi_notes_off();
            self.midi_output_playback.reset_at(current_frame);
        }
        let events = if self.midi_output_playback.initialized {
            midi_output_events_between(
                &self.project,
                self.midi_output_playback.last_frame,
                current_frame,
            )
        } else {
            self.midi_output_playback.initialized = true;
            midi_output_events_at(&self.project, current_frame)
        };
        self.midi_output_playback.last_frame = current_frame;
        if let Err(error) = self.send_midi_output_events(&events) {
            self.midi_output.disconnect();
            self.selected_midi_output_port = None;
            self.midi_output_playback.reset_at(current_frame);
            self.status_error(format!("MIDI output failed: {error}"));
        }
    }

    fn send_midi_output_events(&mut self, events: &[MidiPlaybackEvent]) -> Result<(), String> {
        for event in events {
            let bytes = midi_playback_event_bytes(*event);
            self.midi_output.send(&bytes)?;
            match *event {
                MidiPlaybackEvent::NoteOn { channel, note, .. } => {
                    if !self
                        .midi_output_playback
                        .active_notes
                        .contains(&(channel, note))
                    {
                        self.midi_output_playback.active_notes.push((channel, note));
                    }
                }
                MidiPlaybackEvent::NoteOff { channel, note } => {
                    self.midi_output_playback
                        .active_notes
                        .retain(|active| *active != (channel, note));
                }
                MidiPlaybackEvent::Control { .. }
                | MidiPlaybackEvent::PitchBend { .. }
                | MidiPlaybackEvent::ChannelPressure { .. }
                | MidiPlaybackEvent::PolyPressure { .. }
                | MidiPlaybackEvent::ProgramChange { .. } => {}
            }
        }
        Ok(())
    }

    fn all_midi_notes_off(&mut self) {
        if self.midi_output.connected_name().is_none() {
            self.midi_output_playback.reset_at(self.playhead_frame);
            return;
        }
        for (channel, note) in self.midi_output_playback.active_notes.drain(..) {
            let _ = self
                .midi_output
                .send(&midi_playback_event_bytes(MidiPlaybackEvent::NoteOff {
                    channel,
                    note,
                }));
        }
        for channel in 1..=16 {
            let _ = self
                .midi_output
                .send(&midi_playback_event_bytes(MidiPlaybackEvent::Control {
                    channel,
                    controller: 123,
                    value: 0,
                }));
        }
        self.midi_output_playback.reset_at(self.playhead_frame);
    }

    fn perform_action(&mut self, action: UiAction) {
        match action {
            UiAction::New => self.new_project(),
            UiAction::Open => self.open_project_dialog(),
            UiAction::Save => self.save(),
            UiAction::SaveAs => self.save_as_dialog(),
            UiAction::ExportMixdown(format) => self.export_dialog(format),
            UiAction::ExportStems => self.export_stems_dialog(),
            UiAction::ConsolidateProject => self.consolidate_project_dialog(),
            UiAction::ImportWav => self.import_wav_dialog(),
            UiAction::ImportMidi => self.import_midi_dialog(),
            UiAction::ExportMidi => self.export_midi_dialog(),
            UiAction::Undo => self.undo(),
            UiAction::Redo => self.redo(),
        }
    }

    fn new_project(&mut self) {
        if !self.confirm_discard_changes() {
            return;
        }
        let sample_rate = default_output_device_info().map_or(48_000, |info| info.sample_rate);
        let mut project = Project::new("Untitled", sample_rate, 120.0)
            .unwrap_or_else(|_| demo_project(sample_rate));
        project.tracks.push(Track::new("Track 1"));
        self.replace_project(project, None, false);
        self.view = AppView::Editor;
        self.status_ok("Created a new project");
    }

    fn open_project_dialog(&mut self) {
        if !self.confirm_discard_changes() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("DMO project", &["dmo"])
            .set_title("Open DMO project")
            .pick_file()
        else {
            return;
        };
        match load_project(&path) {
            Ok(project) => {
                self.replace_project(project, Some(path.clone()), false);
                self.view = AppView::Editor;
                self.status_ok(format!("Opened {}", path.display()));
            }
            Err(error) => self.status_error(error),
        }
    }

    fn save(&mut self) {
        let Some(path) = self.project_path.clone() else {
            self.save_as_dialog();
            return;
        };
        self.save_to(&with_extension_if_missing(path, "dmo"));
    }

    fn save_as_dialog(&mut self) {
        let suggested = self
            .project_path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("untitled.dmo");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("DMO project", &["dmo"])
            .set_file_name(suggested)
            .set_title("Save DMO project")
            .save_file()
        else {
            return;
        };
        self.save_to(&with_extension_if_missing(path, "dmo"));
    }

    fn save_to(&mut self, path: &Path) {
        match save_project(path, &self.project) {
            Ok(()) => {
                self.project_path = Some(path.to_path_buf());
                self.dirty = false;
                self.status_ok(format!("Saved {}", path.display()));
            }
            Err(error) => self.status_error(error),
        }
    }

    fn export_dialog(&mut self, format: ExportAudioFormat) {
        let suggested = self
            .project_path
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|name| name.to_str())
            .map_or_else(|| "dmo-export.wav".into(), |name| format!("{name}.wav"));
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_file_name(suggested)
            .set_title(format!("Export stereo {}", format.label()))
            .save_file()
        else {
            return;
        };
        let path = with_extension_if_missing(path, "wav");
        let samples = match try_render_stereo(&self.project) {
            Ok(samples) => samples,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        let result = match format {
            ExportAudioFormat::Wav16 => {
                write_stereo_i16_wav(&path, self.project.sample_rate, &samples)
            }
            ExportAudioFormat::Wav24 => {
                write_stereo_i24_wav(&path, self.project.sample_rate, &samples)
            }
            ExportAudioFormat::WavFloat32 => {
                write_stereo_f32_wav(&path, self.project.sample_rate, &samples)
            }
        };
        match result {
            Ok(()) => self.status_ok(format!("Exported {}", path.display())),
            Err(error) => self.status_error(error),
        }
    }

    fn export_stems_dialog(&mut self) {
        let Some(output_dir) = rfd::FileDialog::new()
            .set_title("Export track stems")
            .pick_folder()
        else {
            return;
        };
        let stems = match try_render_track_stems(&self.project) {
            Ok(stems) => stems,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        if stems.is_empty() {
            self.status_error("No unmuted tracks to export");
            return;
        }
        if let Err(error) = fs::create_dir_all(&output_dir) {
            self.status_error(format!(
                "Could not create {}: {error}",
                output_dir.display()
            ));
            return;
        }
        for stem in &stems {
            let path = output_dir.join(format!(
                "{:02}_{}.wav",
                stem.track_index + 1,
                sanitize_file_component(&stem.track_name)
            ));
            if let Err(error) = write_stereo_i16_wav(&path, self.project.sample_rate, &stem.samples)
            {
                self.status_error(format!("Could not export {}: {error}", path.display()));
                return;
            }
        }
        self.status_ok(format!(
            "Exported {} stem(s) to {}",
            stems.len(),
            output_dir.display()
        ));
    }

    fn consolidate_project_dialog(&mut self) {
        let Some(output_dir) = rfd::FileDialog::new()
            .set_title("Consolidate project")
            .pick_folder()
        else {
            return;
        };
        let media_dir = output_dir.join("Media");
        let consolidated = match consolidate_project_media(
            &self.project,
            self.project_path.as_deref(),
            &media_dir,
        ) {
            Ok(consolidated) => consolidated,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };
        let project_name = self
            .project_path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .map_or_else(
                || format!("{}.dmo", sanitize_file_component(&self.project.name)),
                ToOwned::to_owned,
            );
        let output_project = with_extension_if_missing(output_dir.join(project_name), "dmo");
        match save_project(&output_project, &consolidated.project) {
            Ok(()) => {
                let copied_files = consolidated.copied_files;
                self.replace_project(consolidated.project, Some(output_project.clone()), false);
                self.status_ok(format!(
                    "Consolidated {} media file(s) to {}",
                    copied_files,
                    output_project.display()
                ));
            }
            Err(error) => self.status_error(error),
        }
    }

    fn undo(&mut self) {
        match self.history.undo(&mut self.project) {
            Ok(true) => {
                self.dirty = true;
                self.invalidate_playback();
                self.repair_edit_state();
                self.status_ok("Undid edit");
            }
            Ok(false) => self.status_ok("Nothing to undo"),
            Err(error) => self.status_error(error),
        }
    }

    fn redo(&mut self) {
        match self.history.redo(&mut self.project) {
            Ok(true) => {
                self.dirty = true;
                self.invalidate_playback();
                self.repair_edit_state();
                self.status_ok("Redid edit");
            }
            Ok(false) => self.status_ok("Nothing to redo"),
            Err(error) => self.status_error(error),
        }
    }

    fn replace_project(&mut self, project: Project, path: Option<PathBuf>, dirty: bool) {
        if self.recording.is_some() {
            self.finish_recording();
        }
        self.invalidate_playback();
        self.project = project;
        self.project_path = path;
        self.history.clear();
        self.selected_clip = first_clip_selection(&self.project);
        self.selected_note = first_note_selection(&self.project, self.selected_clip);
        self.clip_clipboard = None;
        self.selected_track = self
            .selected_clip
            .map(|(track_index, _)| track_index)
            .or_else(|| (!self.project.tracks.is_empty()).then_some(0));
        self.playhead_frame = 0;
        self.sync_cycle_controls_from_project();
        self.recording_options.punch_enabled = false;
        self.dirty = dirty;
        self.piano_roll.scroll_to_selection = true;
        self.refresh_waveforms();
    }

    fn repair_edit_state(&mut self) {
        self.selected_track = self
            .selected_track
            .filter(|index| *index < self.project.tracks.len());
        self.selected_clip = self.selected_clip.filter(|(track_index, clip_index)| {
            self.project
                .tracks
                .get(*track_index)
                .is_some_and(|track| *clip_index < track.clips.len())
        });
        self.selected_note = self
            .selected_note
            .filter(|(track_index, clip_index, note_index)| {
                self.project
                    .tracks
                    .get(*track_index)
                    .and_then(|track| track.clips.get(*clip_index))
                    .is_some_and(|clip| match &clip.source {
                        ClipSource::Midi { notes, .. } => *note_index < notes.len(),
                        ClipSource::Sine { .. } | ClipSource::AudioFile { .. } => false,
                    })
            });
        self.sync_cycle_controls_from_project();
        self.playhead_frame = self.playhead_frame.min(self.loop_end_frame);
        self.sync_playback_loop();
    }

    fn sync_cycle_controls_from_project(&mut self) {
        if let Some(range) = self.project.cycle_range {
            self.loop_start_frame = range.start_frame;
            self.loop_end_frame = range.end_frame;
            self.loop_enabled = true;
        } else {
            self.loop_start_frame = 0;
            self.loop_end_frame = self.project.duration_frames().max(1);
            self.loop_enabled = false;
        }
    }

    fn refresh_waveforms(&mut self) {
        let mut sources = self
            .project
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .filter_map(|clip| match &clip.source {
                ClipSource::AudioFile { path, .. } => Some(path.clone()),
                ClipSource::Midi { .. } | ClipSource::Sine { .. } => None,
            })
            .collect::<Vec<_>>();
        sources.extend(
            self.project
                .tracks
                .iter()
                .flat_map(|track| &track.take_lanes)
                .map(|take| take.path.clone()),
        );
        sources.sort();
        sources.dedup();
        let active_sources = sources.iter().cloned().collect::<HashSet<_>>();
        self.waveforms
            .retain(|stored_path, _| active_sources.contains(stored_path));
        self.waveform_errors
            .retain(|stored_path, _| active_sources.contains(stored_path));
        self.waveform_fingerprints
            .retain(|stored_path, _| active_sources.contains(stored_path));

        for stored_path in sources {
            let resolved = self.resolve_audio_path(&stored_path);
            let fingerprint = audio_file_fingerprint(&resolved).ok();
            if self.waveform_fingerprints.get(&stored_path) == fingerprint.as_ref()
                && (self.waveforms.contains_key(&stored_path)
                    || self.waveform_errors.contains_key(&stored_path))
            {
                continue;
            }
            self.waveforms.remove(&stored_path);
            self.waveform_errors.remove(&stored_path);
            self.waveform_fingerprints.remove(&stored_path);
            match decode_wav_overview(&resolved, WAVEFORM_PEAKS) {
                Ok(overview) => {
                    let fingerprint = fingerprint
                        .or_else(|| audio_file_fingerprint(&resolved).ok())
                        .unwrap_or(AudioFileFingerprint {
                            len: 0,
                            modified: None,
                        });
                    self.waveforms
                        .insert(stored_path.clone(), CachedWaveform { overview });
                    self.waveform_fingerprints.insert(stored_path, fingerprint);
                }
                Err(error) => {
                    if let Some(fingerprint) = fingerprint {
                        self.waveform_fingerprints
                            .insert(stored_path.clone(), fingerprint);
                    }
                    self.waveform_errors.insert(stored_path, error.to_string());
                }
            }
        }
    }

    fn waveform_overviews(&self) -> HashMap<String, WaveformOverview> {
        self.waveforms
            .iter()
            .map(|(path, cached)| (path.clone(), cached.overview.clone()))
            .collect()
    }

    fn cache_waveform(
        &mut self,
        stored_path: &str,
        resolved_path: &Path,
        overview: WaveformOverview,
    ) {
        let fingerprint = audio_file_fingerprint(resolved_path).unwrap_or(AudioFileFingerprint {
            len: 0,
            modified: None,
        });
        self.waveforms
            .insert(stored_path.to_owned(), CachedWaveform { overview });
        self.waveform_fingerprints
            .insert(stored_path.to_owned(), fingerprint);
        self.waveform_errors.remove(stored_path);
    }

    fn resolve_audio_path(&self, stored_path: &str) -> PathBuf {
        let path = PathBuf::from(stored_path);
        if path.is_absolute() {
            return path;
        }
        self.project_path
            .as_deref()
            .and_then(Path::parent)
            .map_or(path.clone(), |parent| parent.join(path))
    }

    fn confirm_discard_changes(&self) -> bool {
        if !self.dirty {
            return true;
        }
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Unsaved DMO project")
            .set_description("Discard the unsaved changes in the current project?")
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    }

    fn status_ok(&mut self, message: impl Into<String>) {
        self.status = StatusMessage {
            text: message.into(),
            error: false,
        };
    }

    fn status_error(&mut self, error: impl std::fmt::Display) {
        self.status = StatusMessage {
            text: error.to_string(),
            error: true,
        };
    }
}

#[derive(Debug, Clone, Copy)]
enum TrimSide {
    Left,
    Right,
}

fn first_clip_selection(project: &Project) -> Option<(usize, usize)> {
    project
        .tracks
        .iter()
        .enumerate()
        .find_map(|(track_index, track)| (!track.clips.is_empty()).then_some((track_index, 0)))
}

fn first_note_selection(
    project: &Project,
    clip_selection: Option<(usize, usize)>,
) -> Option<(usize, usize, usize)> {
    let (track_index, clip_index) = clip_selection?;
    let clip = project.tracks.get(track_index)?.clips.get(clip_index)?;
    match &clip.source {
        ClipSource::Midi { notes, .. } if !notes.is_empty() => Some((track_index, clip_index, 0)),
        _ => None,
    }
}

fn split_clip_at_frame(
    clip: &Clip,
    split_frame: u64,
    project_sample_rate: u32,
) -> Option<(Clip, Clip)> {
    if split_frame <= clip.start_frame || split_frame >= clip.end_frame() {
        return None;
    }
    let split_offset = split_frame - clip.start_frame;
    let mut left = clip.clone();
    let mut right = clip.clone();
    left.length_frames = split_offset;
    left.fade_in_frames = left.fade_in_frames.min(left.length_frames);
    left.fade_out_frames = 0;
    right.name = format!("{} B", clip.name);
    right.start_frame = split_frame;
    right.length_frames = clip.length_frames - split_offset;
    right.fade_in_frames = 0;
    right.fade_out_frames = right.fade_out_frames.min(right.length_frames);

    match &clip.source {
        ClipSource::Midi { notes, amplitude } => {
            let mut left_notes = Vec::new();
            let mut right_notes = Vec::new();
            for note in notes {
                let note_end = note.end_frame();
                if note.start_frame < split_offset {
                    left_notes.push(MidiNote {
                        start_frame: note.start_frame,
                        length_frames: note_end.min(split_offset) - note.start_frame,
                        midi_note: note.midi_note,
                        velocity: note.velocity,
                    });
                }
                if note_end > split_offset {
                    let kept_start = note.start_frame.max(split_offset);
                    right_notes.push(MidiNote {
                        start_frame: kept_start - split_offset,
                        length_frames: note_end - kept_start,
                        midi_note: note.midi_note,
                        velocity: note.velocity,
                    });
                }
            }
            left.source = ClipSource::Midi {
                notes: left_notes,
                amplitude: *amplitude,
            };
            right.source = ClipSource::Midi {
                notes: right_notes,
                amplitude: *amplitude,
            };
        }
        ClipSource::AudioFile {
            path,
            source_offset_frames,
            source_sample_rate,
            channels,
            reversed,
        } => {
            right.source = ClipSource::AudioFile {
                path: path.clone(),
                source_offset_frames: source_offset_frames.saturating_add(rescale_frames_round(
                    split_offset,
                    project_sample_rate,
                    *source_sample_rate,
                )),
                source_sample_rate: *source_sample_rate,
                channels: *channels,
                reversed: *reversed,
            };
        }
        ClipSource::Sine { .. } => {}
    }
    Some((left, right))
}

fn slipped_source_offset(
    current_offset: u64,
    delta_project_frames: i64,
    project_sample_rate: u32,
    source_sample_rate: u32,
    source_frames: Option<u64>,
) -> u64 {
    let magnitude = rescale_frames_round(
        delta_project_frames.unsigned_abs(),
        project_sample_rate,
        source_sample_rate,
    );
    let slipped = if delta_project_frames.is_negative() {
        current_offset.saturating_sub(magnitude)
    } else {
        current_offset.saturating_add(magnitude)
    };
    source_frames.map_or(slipped, |frames| slipped.min(frames.saturating_sub(1)))
}

fn metronome_preview_frames(sample_rate: u32, tempo_bpm: f64, beats: u64) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let beat_frames = (f64::from(sample_rate) * 60.0 / tempo_bpm).round() as u64;
    usize::try_from(beat_frames.saturating_mul(beats)).unwrap_or(usize::MAX)
}

fn mix_metronome(samples: &mut [f32], sample_rate: u32, tempo_bpm: f64) {
    if sample_rate == 0 || !tempo_bpm.is_finite() || tempo_bpm <= 0.0 {
        return;
    }
    let frame_count = samples.len() / 2;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let beat_frames = ((f64::from(sample_rate) * 60.0 / tempo_bpm).round() as usize).max(1);
    let click_frames = usize::try_from(sample_rate / 40).unwrap_or(1).max(1);
    let mut beat = 0_usize;
    let mut start = 0_usize;
    while start < frame_count {
        let frequency = if beat.is_multiple_of(4) {
            1_760.0
        } else {
            1_120.0
        };
        for local_frame in 0..click_frames.min(frame_count - start) {
            #[allow(clippy::cast_precision_loss)]
            let time = local_frame as f32 / sample_rate as f32;
            #[allow(clippy::cast_precision_loss)]
            let envelope = 1.0 - local_frame as f32 / click_frames as f32;
            let click = (std::f32::consts::TAU * frequency * time).sin() * envelope * 0.22;
            let frame = start + local_frame;
            samples[frame * 2] = (samples[frame * 2] + click).clamp(-1.0, 1.0);
            samples[frame * 2 + 1] = (samples[frame * 2 + 1] + click).clamp(-1.0, 1.0);
        }
        beat = beat.saturating_add(1);
        start = start.saturating_add(beat_frames);
    }
}

fn clip_trim_command(
    project: &Project,
    selection: Option<(usize, usize)>,
    playhead_frame: u64,
    side: TrimSide,
) -> Option<EditCommand> {
    let (track_index, clip_index) = selection?;
    let clip = project.tracks.get(track_index)?.clips.get(clip_index)?;
    if playhead_frame <= clip.start_frame || playhead_frame >= clip.end_frame() {
        return None;
    }

    match side {
        TrimSide::Left => {
            let ClipSource::AudioFile {
                source_offset_frames,
                source_sample_rate,
                ..
            } = &clip.source
            else {
                return None;
            };
            let removed_project_frames = playhead_frame - clip.start_frame;
            let removed_source_frames = rescale_frames_round(
                removed_project_frames,
                project.sample_rate,
                *source_sample_rate,
            );
            Some(EditCommand::SetClipTiming {
                track_index,
                clip_index,
                start_frame: playhead_frame,
                length_frames: clip.end_frame() - playhead_frame,
                source_offset_frames: Some(
                    source_offset_frames.saturating_add(removed_source_frames),
                ),
            })
        }
        TrimSide::Right => Some(EditCommand::SetClipTiming {
            track_index,
            clip_index,
            start_frame: clip.start_frame,
            length_frames: playhead_frame - clip.start_frame,
            source_offset_frames: match &clip.source {
                ClipSource::AudioFile {
                    source_offset_frames,
                    ..
                } => Some(*source_offset_frames),
                ClipSource::Midi { .. } | ClipSource::Sine { .. } => None,
            },
        }),
    }
}

impl eframe::App for DmoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let marker = if self.dirty { "*" } else { "" };
        ui.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "DMO — {}{marker}",
            self.project.name
        )));
        if self.view == AppView::Start {
            self.start_screen(ui);
            return;
        }
        self.update_playback(ui);
        self.update_live_midi(ui);
        self.update_recording(ui);
        self.keyboard_shortcuts(ui);

        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("arrange_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(37, 40, 47))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(52, 56, 65)))
                    .inner_margin(5.0),
            )
            .show(ui, |ui| self.arrange_toolbar(ui));
        egui::Panel::bottom("transport_bar")
            .default_size(52.0)
            .size_range(52.0..=52.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(28, 31, 37))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(50, 54, 63)))
                    .inner_margin(6.0),
            )
            .show(ui, |ui| self.transport_bar(ui));
        egui::Panel::left("track_panel")
            .default_size(230.0)
            .size_range(190.0..=320.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 27, 34))
                    .inner_margin(0.0),
            )
            .show(ui, |ui| self.track_panel(ui));
        egui::Panel::right("inspector")
            .default_size(240.0)
            .size_range(200.0..=340.0)
            .resizable(true)
            .show(ui, |ui| self.inspector(ui));
        if self.piano_roll.open {
            egui::Panel::bottom("piano_roll")
                .default_size(280.0)
                .size_range(180.0..=520.0)
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(Color32::from_rgb(21, 24, 31))
                        .inner_margin(7.0),
                )
                .show(ui, |ui| self.piano_roll(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_rgb(22, 25, 32)))
            .show(ui, |ui| self.timeline(ui));
    }
}

fn menu_action(ui: &mut egui::Ui, label: &str, action: &mut Option<UiAction>, value: UiAction) {
    if ui.button(label).clicked() {
        *action = Some(value);
        ui.close();
    }
}

fn track_routing_editor(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    track: &mut Track,
    buses: &[Bus],
) {
    ui.horizontal(|ui| {
        ui.label("Output");
        egui::ComboBox::from_id_salt(("track_output", &id))
            .selected_text(channel_output_label(track.output, buses))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut track.output, ChannelOutput::Master, "Master");
                for (index, bus) in buses.iter().enumerate() {
                    ui.selectable_value(
                        &mut track.output,
                        ChannelOutput::Bus(index),
                        bus.name.as_str(),
                    );
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("Sends");
        if ui
            .add_enabled(!buses.is_empty(), egui::Button::new("+ Send"))
            .clicked()
        {
            track.sends.push(TrackSend::new(0));
        }
    });
    if buses.is_empty() {
        ui.small("Add a bus before creating sends");
    }

    let mut remove_send = None;
    for send_index in 0..track.sends.len() {
        ui.horizontal(|ui| {
            let send = &mut track.sends[send_index];
            ui.checkbox(&mut send.enabled, "");
            egui::ComboBox::from_id_salt(("send_bus", &id, send_index))
                .width(86.0)
                .selected_text(
                    buses
                        .get(send.bus_index)
                        .map_or("Missing bus", |bus| bus.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for (bus_index, bus) in buses.iter().enumerate() {
                        ui.selectable_value(&mut send.bus_index, bus_index, bus.name.as_str());
                    }
                });
            ui.toggle_value(&mut send.pre_fader, "Pre");
            ui.add_sized(
                [62.0, 18.0],
                egui::Slider::new(&mut send.gain, 0.0..=1.5).show_value(false),
            );
            if ui.small_button("×").on_hover_text("Delete send").clicked() {
                remove_send = Some(send_index);
            }
        });
    }
    if let Some(index) = remove_send {
        track.sends.remove(index);
    }
}

fn channel_output_label(output: ChannelOutput, buses: &[Bus]) -> String {
    match output {
        ChannelOutput::Master => "Master".into(),
        ChannelOutput::Bus(index) => buses
            .get(index)
            .map_or_else(|| format!("Missing bus {index}"), |bus| bus.name.clone()),
    }
}

fn insert_chain_editor(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    inserts: &mut Vec<ChannelInsert>,
) {
    ui.horizontal(|ui| {
        ui.label("Inserts");
        if ui.button("+").on_hover_text("Add insert").clicked() {
            inserts.push(ChannelInsert::new(InsertEffect::Gain { gain_db: 0.0 }));
        }
        egui::ComboBox::from_id_salt(("insert_preset", &id))
            .width(116.0)
            .selected_text("Preset")
            .show_ui(ui, |ui| {
                for preset in ChannelStripPreset::ALL {
                    if ui.button(preset.label()).clicked() {
                        *inserts = preset.inserts();
                        ui.close();
                    }
                }
            });
    });
    let mut remove_insert = None;
    for (index, insert) in inserts.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut insert.enabled, "");
                let mut selected = insert.effect;
                egui::ComboBox::from_id_salt(("insert_effect", &id, index))
                    .width(122.0)
                    .selected_text(selected.label())
                    .show_ui(ui, |ui| {
                        for effect in InsertEffect::ALL_DEFAULTS {
                            ui.selectable_value(&mut selected, effect, effect.label());
                        }
                    });
                if selected.label() != insert.effect.label() {
                    insert.effect = selected;
                }
                if ui
                    .small_button("×")
                    .on_hover_text("Delete insert")
                    .clicked()
                {
                    remove_insert = Some(index);
                }
            });
            insert_effect_editor(ui, &mut insert.effect);
        });
    }
    if let Some(index) = remove_insert {
        inserts.remove(index);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelStripPreset {
    CleanVocal,
    TightBass,
    DrumPunch,
    MasterGlue,
    LoFiColor,
}

impl ChannelStripPreset {
    const ALL: [Self; 5] = [
        Self::CleanVocal,
        Self::TightBass,
        Self::DrumPunch,
        Self::MasterGlue,
        Self::LoFiColor,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::CleanVocal => "Clean Vocal",
            Self::TightBass => "Tight Bass",
            Self::DrumPunch => "Drum Punch",
            Self::MasterGlue => "Master Glue",
            Self::LoFiColor => "Lo-fi Color",
        }
    }

    fn inserts(self) -> Vec<ChannelInsert> {
        match self {
            Self::CleanVocal => vec![
                ChannelInsert::new(InsertEffect::ThreeBandEq {
                    low_db: -2.0,
                    mid_db: 1.5,
                    high_db: 3.0,
                }),
                ChannelInsert::new(InsertEffect::Compressor {
                    threshold_db: -20.0,
                    ratio: 3.0,
                    attack_ms: 8.0,
                    release_ms: 120.0,
                    makeup_db: 2.0,
                }),
            ],
            Self::TightBass => vec![
                ChannelInsert::new(InsertEffect::ThreeBandEq {
                    low_db: 3.0,
                    mid_db: -2.5,
                    high_db: -1.0,
                }),
                ChannelInsert::new(InsertEffect::Compressor {
                    threshold_db: -24.0,
                    ratio: 5.0,
                    attack_ms: 18.0,
                    release_ms: 160.0,
                    makeup_db: 3.0,
                }),
                ChannelInsert::new(InsertEffect::Saturation {
                    drive: 1.4,
                    mix: 0.35,
                }),
            ],
            Self::DrumPunch => vec![
                ChannelInsert::new(InsertEffect::Compressor {
                    threshold_db: -18.0,
                    ratio: 6.0,
                    attack_ms: 28.0,
                    release_ms: 80.0,
                    makeup_db: 4.0,
                }),
                ChannelInsert::new(InsertEffect::ThreeBandEq {
                    low_db: 2.0,
                    mid_db: -1.0,
                    high_db: 2.5,
                }),
            ],
            Self::MasterGlue => vec![
                ChannelInsert::new(InsertEffect::Compressor {
                    threshold_db: -12.0,
                    ratio: 2.0,
                    attack_ms: 30.0,
                    release_ms: 220.0,
                    makeup_db: 1.0,
                }),
                ChannelInsert::new(InsertEffect::Gain { gain_db: -1.0 }),
            ],
            Self::LoFiColor => vec![
                ChannelInsert::new(InsertEffect::ThreeBandEq {
                    low_db: -1.0,
                    mid_db: 2.0,
                    high_db: -4.0,
                }),
                ChannelInsert::new(InsertEffect::Saturation {
                    drive: 3.0,
                    mix: 0.65,
                }),
                ChannelInsert::new(InsertEffect::Gain { gain_db: -2.0 }),
            ],
        }
    }
}

fn insert_effect_editor(ui: &mut egui::Ui, effect: &mut InsertEffect) {
    match effect {
        InsertEffect::Gain { gain_db } => {
            ui.add(egui::Slider::new(gain_db, -24.0..=24.0).text("Gain dB"));
        }
        InsertEffect::ThreeBandEq {
            low_db,
            mid_db,
            high_db,
        } => {
            ui.add(egui::Slider::new(low_db, -18.0..=18.0).text("Low"));
            ui.add(egui::Slider::new(mid_db, -18.0..=18.0).text("Mid"));
            ui.add(egui::Slider::new(high_db, -18.0..=18.0).text("High"));
        }
        InsertEffect::Compressor {
            threshold_db,
            ratio,
            attack_ms,
            release_ms,
            makeup_db,
        } => {
            ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("Threshold"));
            ui.add(egui::Slider::new(ratio, 1.0..=20.0).text("Ratio"));
            ui.add(egui::Slider::new(attack_ms, 0.1..=100.0).text("Attack ms"));
            ui.add(egui::Slider::new(release_ms, 5.0..=1_000.0).text("Release ms"));
            ui.add(egui::Slider::new(makeup_db, -12.0..=24.0).text("Makeup"));
        }
        InsertEffect::Saturation { drive, mix } => {
            ui.add(egui::Slider::new(drive, 0.0..=8.0).text("Drive"));
            ui.add(egui::Slider::new(mix, 0.0..=1.0).text("Mix"));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LevelMeter {
    left: f32,
    right: f32,
}

impl LevelMeter {
    const SILENCE: Self = Self {
        left: 0.0,
        right: 0.0,
    };

    fn scaled(self, gain: f32) -> Self {
        Self {
            left: (self.left * gain).max(0.0),
            right: (self.right * gain).max(0.0),
        }
    }

    fn add(self, other: Self) -> Self {
        Self {
            left: (self.left + other.left).min(1.0),
            right: (self.right + other.right).min(1.0),
        }
    }
}

fn estimate_master_meter(project: &Project, frame: u64) -> LevelMeter {
    project
        .tracks
        .iter()
        .filter(|track| !track.muted)
        .map(|track| estimate_track_meter(track, frame))
        .fold(LevelMeter::SILENCE, LevelMeter::add)
        .scaled(project.master_gain)
}

fn estimate_track_meter(track: &Track, frame: u64) -> LevelMeter {
    if track.muted {
        return LevelMeter::SILENCE;
    }
    let clip_level = track
        .clips
        .iter()
        .map(|clip| estimate_clip_level(clip, frame))
        .fold(0.0_f32, |sum, level| (sum + level).min(1.0));
    let level = clip_level * track.gain.max(0.0) * track.automation_gain_at(frame).max(0.0);
    let pan = track.pan.clamp(-1.0, 1.0);
    let left = level * if pan > 0.0 { 1.0 - pan * 0.65 } else { 1.0 };
    let right = level * if pan < 0.0 { 1.0 + pan * 0.65 } else { 1.0 };
    LevelMeter {
        left: left.min(1.0),
        right: right.min(1.0),
    }
}

fn estimate_clip_level(clip: &Clip, frame: u64) -> f32 {
    if frame < clip.start_frame || frame >= clip.end_frame() {
        return 0.0;
    }
    let envelope = clip_envelope_gain(clip, frame);
    let source_level = match &clip.source {
        ClipSource::Midi { notes, amplitude } => notes
            .iter()
            .filter(|note| {
                let start = clip.start_frame.saturating_add(note.start_frame);
                frame >= start && frame < start.saturating_add(note.length_frames)
            })
            .map(|note| f32::from(note.velocity) / 127.0 * *amplitude)
            .fold(0.0_f32, |sum, level| (sum + level).min(1.0)),
        ClipSource::Sine { amplitude, .. } => *amplitude,
        ClipSource::AudioFile { .. } => 0.75,
    };
    (source_level * clip.gain.max(0.0) * envelope).clamp(0.0, 1.0)
}

fn clip_envelope_gain(clip: &Clip, frame: u64) -> f32 {
    let relative = frame.saturating_sub(clip.start_frame);
    let fade_in = if clip.fade_in_frames == 0 {
        1.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            (relative as f32 / clip.fade_in_frames as f32).clamp(0.0, 1.0)
        }
    };
    let remaining = clip.end_frame().saturating_sub(frame);
    let fade_out = if clip.fade_out_frames == 0 {
        1.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            (remaining as f32 / clip.fade_out_frames as f32).clamp(0.0, 1.0)
        }
    };
    fade_in.min(fade_out)
}

fn normalized_clip_gain(
    stereo_samples: &[f32],
    source_offset_frames: u64,
    clip_length_frames: u64,
    project_sample_rate: u32,
    source_sample_rate: u32,
) -> Option<f32> {
    if project_sample_rate == 0 || source_sample_rate == 0 || clip_length_frames == 0 {
        return None;
    }
    let source_frames = u64::try_from(stereo_samples.len() / 2).ok()?;
    let source_length_frames =
        rescale_frames_round(clip_length_frames, project_sample_rate, source_sample_rate).max(1);
    let start = source_offset_frames.min(source_frames);
    let end = start
        .saturating_add(source_length_frames)
        .min(source_frames);
    if end <= start {
        return None;
    }
    let start_sample = usize::try_from(start.saturating_mul(2)).ok()?;
    let end_sample = usize::try_from(end.saturating_mul(2)).ok()?;
    let peak = stereo_samples[start_sample..end_sample]
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    if peak <= f32::EPSILON {
        return None;
    }
    Some((0.98 / peak).min(16.0))
}

fn level_meter(ui: &mut egui::Ui, meter: LevelMeter, width: f32) {
    let desired = egui::vec2(width, 13.0);
    let (rect, _response) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, Color32::from_rgb(13, 16, 20));
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, Color32::from_rgb(49, 55, 66)),
        egui::StrokeKind::Inside,
    );
    let gap = 1.0;
    let lane_height = (rect.height() - gap) * 0.5;
    let top = egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), lane_height));
    let bottom = egui::Rect::from_min_size(
        egui::pos2(rect.left(), top.bottom() + gap),
        egui::vec2(rect.width(), lane_height),
    );
    paint_meter_lane(painter, top.shrink(1.0), meter.left);
    paint_meter_lane(painter, bottom.shrink(1.0), meter.right);
}

fn paint_meter_lane(painter: &egui::Painter, rect: egui::Rect, value: f32) {
    let value = value.clamp(0.0, 1.0);
    let fill = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(rect.left() + rect.width() * value, rect.bottom()),
    );
    let color = if value > 0.92 {
        Color32::from_rgb(255, 92, 92)
    } else if value > 0.72 {
        Color32::from_rgb(244, 185, 77)
    } else {
        Color32::from_rgb(82, 214, 142)
    };
    painter.rect_filled(fill, 1.0, color);
    painter.vline(
        rect.left() + rect.width() * 0.72,
        rect.y_range(),
        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(240, 240, 240, 80)),
    );
    painter.vline(
        rect.left() + rect.width() * 0.92,
        rect.y_range(),
        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(240, 240, 240, 100)),
    );
}

fn track_input_label(input: TrackInput) -> String {
    match input {
        TrackInput::Audio => "Audio Stereo 1–2".into(),
        TrackInput::AudioMonoLeft => "Audio Mono 1".into(),
        TrackInput::AudioMonoRight => "Audio Mono 2".into(),
        TrackInput::MidiOmni => "MIDI Omni".into(),
        TrackInput::MidiChannel(channel) => format!("MIDI Ch {channel}"),
    }
}

fn parse_live_midi(message: LiveMidiMessage) -> Option<ParsedLiveMidi> {
    let (bytes, len) = message.data();
    let status = bytes[0];
    if !(0x80..0xF0).contains(&status) {
        return None;
    }
    let kind = status & 0xF0;
    let channel = (status & 0x0F) + 1;
    let first = bytes[1].min(127);
    let second = bytes[2].min(127);
    match kind {
        0x80 if len >= 3 => Some(ParsedLiveMidi::NoteOff {
            channel,
            note: first,
        }),
        0x90 if len >= 3 && second == 0 => Some(ParsedLiveMidi::NoteOff {
            channel,
            note: first,
        }),
        0x90 if len >= 3 => Some(ParsedLiveMidi::NoteOn {
            channel,
            note: first,
            velocity: second,
        }),
        0xA0 if len >= 3 => Some(ParsedLiveMidi::PolyPressure {
            channel,
            note: first,
            value: second,
        }),
        0xB0 if len >= 3 => Some(ParsedLiveMidi::Control {
            channel,
            controller: first,
            value: second,
        }),
        0xC0 if len >= 2 => Some(ParsedLiveMidi::ProgramChange {
            channel,
            program: first.saturating_add(1),
        }),
        0xD0 if len >= 2 => Some(ParsedLiveMidi::ChannelPressure {
            channel,
            value: first,
        }),
        0xE0 if len >= 3 => {
            let unsigned = u16::from(first) | (u16::from(second) << 7);
            Some(ParsedLiveMidi::PitchBend {
                channel,
                value: i16::try_from(unsigned).unwrap_or(16_383) - 8192,
            })
        }
        _ => None,
    }
}

fn midi_output_events_at(project: &Project, frame: u64) -> Vec<MidiPlaybackEvent> {
    let mut events = Vec::new();
    for track in &project.tracks {
        let channel = track.midi_channel.clamp(1, 16);
        for point in track
            .midi_cc
            .iter()
            .filter(|point| point.frame <= frame)
            .fold(
                HashMap::<u8, &MidiControlPoint>::new(),
                |mut latest, point| {
                    latest
                        .entry(point.controller)
                        .and_modify(|previous| {
                            if point.frame >= previous.frame {
                                *previous = point;
                            }
                        })
                        .or_insert(point);
                    latest
                },
            )
            .values()
        {
            events.push(MidiPlaybackEvent::Control {
                channel,
                controller: point.controller,
                value: point.value,
            });
        }
        if let Some(point) = track
            .midi_pitch_bend
            .iter()
            .filter(|point| point.frame <= frame)
            .max_by_key(|point| point.frame)
        {
            events.push(MidiPlaybackEvent::PitchBend {
                channel,
                value: point.value,
            });
        }
        if let Some(point) = track
            .midi_channel_pressure
            .iter()
            .filter(|point| point.frame <= frame)
            .max_by_key(|point| point.frame)
        {
            events.push(MidiPlaybackEvent::ChannelPressure {
                channel,
                value: point.value,
            });
        }
        if let Some(point) = track
            .midi_program_changes
            .iter()
            .filter(|point| point.frame <= frame)
            .max_by_key(|point| point.frame)
        {
            events.push(MidiPlaybackEvent::ProgramChange {
                channel,
                program: point.program,
            });
        }
        for clip in &track.clips {
            let ClipSource::Midi { notes, .. } = &clip.source else {
                continue;
            };
            for note in notes {
                let start = clip.start_frame.saturating_add(note.start_frame);
                let end = start.saturating_add(note.length_frames);
                if start <= frame && frame < end {
                    events.push(MidiPlaybackEvent::NoteOn {
                        channel,
                        note: note.midi_note,
                        velocity: note.velocity,
                    });
                }
            }
        }
    }
    events
}

#[allow(clippy::too_many_lines)]
fn midi_output_events_between(
    project: &Project,
    start_frame: u64,
    end_frame: u64,
) -> Vec<MidiPlaybackEvent> {
    if end_frame <= start_frame {
        return Vec::new();
    }
    let mut timed = Vec::<(u64, u8, MidiPlaybackEvent)>::new();
    for track in &project.tracks {
        let channel = track.midi_channel.clamp(1, 16);
        for point in &track.midi_cc {
            if point.frame > start_frame && point.frame <= end_frame {
                timed.push((
                    point.frame,
                    1,
                    MidiPlaybackEvent::Control {
                        channel,
                        controller: point.controller,
                        value: point.value,
                    },
                ));
            }
        }
        for point in &track.midi_pitch_bend {
            if point.frame > start_frame && point.frame <= end_frame {
                timed.push((
                    point.frame,
                    1,
                    MidiPlaybackEvent::PitchBend {
                        channel,
                        value: point.value,
                    },
                ));
            }
        }
        for point in &track.midi_channel_pressure {
            if point.frame > start_frame && point.frame <= end_frame {
                timed.push((
                    point.frame,
                    1,
                    MidiPlaybackEvent::ChannelPressure {
                        channel,
                        value: point.value,
                    },
                ));
            }
        }
        for point in &track.midi_poly_pressure {
            if point.frame > start_frame && point.frame <= end_frame {
                timed.push((
                    point.frame,
                    1,
                    MidiPlaybackEvent::PolyPressure {
                        channel,
                        note: point.note,
                        value: point.value,
                    },
                ));
            }
        }
        for point in &track.midi_program_changes {
            if point.frame > start_frame && point.frame <= end_frame {
                timed.push((
                    point.frame,
                    1,
                    MidiPlaybackEvent::ProgramChange {
                        channel,
                        program: point.program,
                    },
                ));
            }
        }
        for clip in &track.clips {
            let ClipSource::Midi { notes, .. } = &clip.source else {
                continue;
            };
            for note in notes {
                let note_start = clip.start_frame.saturating_add(note.start_frame);
                let note_end = note_start.saturating_add(note.length_frames);
                if note_end > start_frame && note_end <= end_frame {
                    timed.push((
                        note_end,
                        0,
                        MidiPlaybackEvent::NoteOff {
                            channel,
                            note: note.midi_note,
                        },
                    ));
                }
                if note_start > start_frame && note_start <= end_frame {
                    timed.push((
                        note_start,
                        2,
                        MidiPlaybackEvent::NoteOn {
                            channel,
                            note: note.midi_note,
                            velocity: note.velocity,
                        },
                    ));
                }
            }
        }
    }
    timed.sort_by_key(|(frame, order, _event)| (*frame, *order));
    timed.into_iter().map(|(_, _, event)| event).collect()
}

fn midi_playback_event_bytes(event: MidiPlaybackEvent) -> [u8; 3] {
    match event {
        MidiPlaybackEvent::NoteOn {
            channel,
            note,
            velocity,
        } => [
            0x90 | midi_status_channel(channel),
            note.min(127),
            velocity.min(127),
        ],
        MidiPlaybackEvent::NoteOff { channel, note } => {
            [0x80 | midi_status_channel(channel), note.min(127), 0]
        }
        MidiPlaybackEvent::Control {
            channel,
            controller,
            value,
        } => [
            0xB0 | midi_status_channel(channel),
            controller.min(127),
            value.min(127),
        ],
        MidiPlaybackEvent::PitchBend { channel, value } => {
            let wheel = u16::try_from(i32::from(value.clamp(-8192, 8191)) + 8192).unwrap_or(0);
            [
                0xE0 | midi_status_channel(channel),
                (wheel & 0x7F) as u8,
                ((wheel >> 7) & 0x7F) as u8,
            ]
        }
        MidiPlaybackEvent::ChannelPressure { channel, value } => {
            [0xD0 | midi_status_channel(channel), value.min(127), 0]
        }
        MidiPlaybackEvent::PolyPressure {
            channel,
            note,
            value,
        } => [
            0xA0 | midi_status_channel(channel),
            note.min(127),
            value.min(127),
        ],
        MidiPlaybackEvent::ProgramChange { channel, program } => [
            0xC0 | midi_status_channel(channel),
            program.clamp(1, 128) - 1,
            0,
        ],
    }
}

const fn midi_status_channel(channel: u8) -> u8 {
    if channel == 0 {
        0
    } else if channel > 16 {
        15
    } else {
        channel - 1
    }
}

fn record_midi_event(
    take: &mut ActiveMidiTake,
    event: ParsedLiveMidi,
    frame: u64,
    record_start_frame: u64,
) {
    let channel = match event {
        ParsedLiveMidi::NoteOn { channel, .. }
        | ParsedLiveMidi::NoteOff { channel, .. }
        | ParsedLiveMidi::Control { channel, .. }
        | ParsedLiveMidi::PitchBend { channel, .. }
        | ParsedLiveMidi::ChannelPressure { channel, .. }
        | ParsedLiveMidi::PolyPressure { channel, .. }
        | ParsedLiveMidi::ProgramChange { channel, .. } => channel,
    };
    if !take.input.accepts_midi_channel(channel) {
        return;
    }
    match event {
        ParsedLiveMidi::NoteOn {
            channel,
            note,
            velocity,
        } => {
            if let Some((start, previous_velocity)) =
                take.active_notes.insert((channel, note), (frame, velocity))
            {
                push_recorded_note(
                    take,
                    start,
                    frame,
                    note,
                    previous_velocity,
                    record_start_frame,
                );
            }
        }
        ParsedLiveMidi::NoteOff { channel, note } => {
            if let Some((start, velocity)) = take.active_notes.remove(&(channel, note)) {
                push_recorded_note(take, start, frame, note, velocity, record_start_frame);
            }
        }
        ParsedLiveMidi::Control {
            controller, value, ..
        } if frame >= record_start_frame => take.controllers.push(MidiControlPoint {
            frame,
            controller,
            value,
        }),
        ParsedLiveMidi::PitchBend { value, .. } if frame >= record_start_frame => {
            take.pitch_bend.push(MidiPitchBendPoint { frame, value });
        }
        ParsedLiveMidi::ChannelPressure { value, .. } if frame >= record_start_frame => {
            take.channel_pressure
                .push(MidiChannelPressurePoint { frame, value });
        }
        ParsedLiveMidi::PolyPressure { note, value, .. } if frame >= record_start_frame => {
            take.poly_pressure
                .push(MidiPolyPressurePoint { frame, note, value });
        }
        ParsedLiveMidi::ProgramChange { program, .. } if frame >= record_start_frame => {
            take.program_changes
                .push(MidiProgramChangePoint { frame, program });
        }
        ParsedLiveMidi::Control { .. }
        | ParsedLiveMidi::PitchBend { .. }
        | ParsedLiveMidi::ChannelPressure { .. }
        | ParsedLiveMidi::PolyPressure { .. }
        | ParsedLiveMidi::ProgramChange { .. } => {}
    }
}

fn prune_retrospective_midi(
    events: &mut VecDeque<RetrospectiveMidiEvent>,
    now: Instant,
    window: Duration,
) {
    while events
        .front()
        .is_some_and(|event| now.saturating_duration_since(event.received_at) > window)
    {
        events.pop_front();
    }
}

fn retrospective_midi_clip(
    events: &VecDeque<RetrospectiveMidiEvent>,
    now: Instant,
    window: Duration,
    sample_rate: u32,
) -> Option<Clip> {
    let cutoff = now.checked_sub(window).unwrap_or(now);
    let mut active_notes = HashMap::<(u8, u8), (u64, u8)>::new();
    let mut notes = Vec::<MidiNote>::new();
    let mut first_frame = u64::MAX;
    let mut last_frame = 0;
    for event in events.iter().filter(|event| event.received_at >= cutoff) {
        let frame = duration_to_frames(
            event.received_at.saturating_duration_since(cutoff),
            sample_rate,
        );
        match event.event {
            ParsedLiveMidi::NoteOn {
                channel,
                note,
                velocity,
            } => {
                active_notes.insert((channel, note), (frame, velocity));
            }
            ParsedLiveMidi::NoteOff { channel, note } => {
                if let Some((start_frame, velocity)) = active_notes.remove(&(channel, note)) {
                    let end_frame = frame.max(start_frame.saturating_add(1));
                    first_frame = first_frame.min(start_frame);
                    last_frame = last_frame.max(end_frame);
                    notes.push(MidiNote {
                        start_frame,
                        length_frames: end_frame - start_frame,
                        midi_note: note,
                        velocity,
                    });
                }
            }
            ParsedLiveMidi::Control { .. }
            | ParsedLiveMidi::PitchBend { .. }
            | ParsedLiveMidi::ChannelPressure { .. }
            | ParsedLiveMidi::PolyPressure { .. }
            | ParsedLiveMidi::ProgramChange { .. } => {}
        }
    }
    for ((_channel, note), (start_frame, velocity)) in active_notes {
        let end_frame = duration_to_frames(now.saturating_duration_since(cutoff), sample_rate)
            .max(start_frame.saturating_add(1));
        first_frame = first_frame.min(start_frame);
        last_frame = last_frame.max(end_frame);
        notes.push(MidiNote {
            start_frame,
            length_frames: end_frame - start_frame,
            midi_note: note,
            velocity,
        });
    }
    if notes.is_empty() || first_frame == u64::MAX || last_frame <= first_frame {
        return None;
    }
    for note in &mut notes {
        note.start_frame = note.start_frame.saturating_sub(first_frame);
    }
    notes.sort_by_key(|note| (note.start_frame, note.midi_note));
    Some(Clip {
        name: "Retrospective MIDI".into(),
        start_frame: 0,
        length_frames: last_frame - first_frame,
        gain: 1.0,
        fade_in_frames: 0,
        fade_out_frames: 0,
        fade_curve: FadeCurve::Linear,
        source: ClipSource::Midi {
            notes,
            amplitude: 0.8,
        },
    })
}

fn push_recorded_note(
    take: &mut ActiveMidiTake,
    start_frame: u64,
    end_frame: u64,
    midi_note: u8,
    velocity: u8,
    record_start_frame: u64,
) {
    if end_frame <= record_start_frame {
        return;
    }
    let start_frame = start_frame.max(record_start_frame);
    take.notes.push(RecordedMidiNote {
        start_frame,
        length_frames: end_frame.saturating_sub(start_frame).max(1),
        midi_note,
        velocity: velocity.max(1),
    });
}

fn finish_active_midi_notes(take: &mut ActiveMidiTake, finish_frame: u64, record_start_frame: u64) {
    let active = take.active_notes.drain().collect::<Vec<_>>();
    for ((_channel, note), (start, velocity)) in active {
        push_recorded_note(
            take,
            start,
            finish_frame,
            note,
            velocity,
            record_start_frame,
        );
    }
}

fn elapsed_frames(started_at: Instant, sample_rate: u32) -> u64 {
    duration_to_frames(started_at.elapsed(), sample_rate)
}

fn audio_file_fingerprint(path: &Path) -> std::io::Result<AudioFileFingerprint> {
    let metadata = fs::metadata(path)?;
    Ok(AudioFileFingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn duration_to_frames(duration: Duration, sample_rate: u32) -> u64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (duration.as_secs_f64() * f64::from(sample_rate)).round() as u64
    }
}

fn configure_theme(context: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(24, 27, 34);
    visuals.window_fill = Color32::from_rgb(28, 31, 39);
    visuals.selection.bg_fill = Color32::from_rgb(68, 103, 207);
    visuals.widgets.active.bg_fill = Color32::from_rgb(74, 109, 214);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(55, 63, 82);
    context.set_visuals(visuals);

    context.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

fn demo_project(sample_rate: u32) -> Project {
    let mut project = Project {
        name: "First DMO Song".into(),
        sample_rate,
        tempo_bpm: 120.0,
        time_signature: TimeSignature::default(),
        cycle_range: None,
        markers: Vec::new(),
        arranger_sections: Vec::new(),
        master_gain: 1.0,
        master_inserts: Vec::new(),
        buses: Vec::new(),
        tracks: Vec::new(),
    };
    let beat = u64::from(sample_rate) / 2;
    project.cycle_range = CycleRange::new(0, beat * 4);
    project.markers = vec![Marker::new("Intro", 0), Marker::new("Loop", beat * 4)];
    project.arranger_sections = vec![ArrangerSection {
        name: "Intro".into(),
        start_frame: 0,
        length_frames: beat * 4,
        color_index: 0,
    }];

    let mut melody = Track::new("Melody");
    melody.gain = 0.35;
    melody.instrument = Instrument::Triangle;
    melody.midi_channel = 1;
    melody.input = TrackInput::MidiOmni;
    melody.volume_automation = vec![
        AutomationPoint {
            frame: 0,
            value: 0.72,
        },
        AutomationPoint {
            frame: beat * 2,
            value: 1.15,
        },
        AutomationPoint {
            frame: beat * 4,
            value: 0.82,
        },
    ];
    melody.clips.push(Clip {
        name: "Melody Part".into(),
        start_frame: 0,
        length_frames: beat * 4,
        gain: 1.0,
        fade_in_frames: beat / 8,
        fade_out_frames: beat / 8,
        fade_curve: FadeCurve::Linear,
        source: ClipSource::Midi {
            notes: [60, 64, 67, 72]
                .into_iter()
                .enumerate()
                .map(|(index, midi_note)| MidiNote {
                    start_frame: u64::try_from(index).unwrap_or(0) * beat,
                    length_frames: beat,
                    midi_note,
                    velocity: 108,
                })
                .collect(),
            amplitude: 0.8,
        },
    });

    let mut bass = Track::new("Bass");
    bass.gain = 0.2;
    bass.pan = -0.15;
    bass.instrument = Instrument::Square;
    bass.midi_channel = 2;
    bass.input = TrackInput::MidiChannel(2);
    bass.clips.push(Clip {
        name: "Bass C".into(),
        start_frame: 0,
        length_frames: beat * 4,
        gain: 1.0,
        fade_in_frames: beat / 8,
        fade_out_frames: beat / 8,
        fade_curve: FadeCurve::Linear,
        source: ClipSource::Midi {
            notes: vec![MidiNote {
                start_frame: 0,
                length_frames: beat * 4,
                midi_note: 48,
                velocity: 116,
            }],
            amplitude: 0.8,
        },
    });
    project.tracks.extend([melody, bass]);
    project
}

fn snap_frame(
    frame: u64,
    sample_rate: u32,
    tempo_bpm: f64,
    snap_enabled: bool,
    snap_grid: SnapGrid,
) -> u64 {
    if !snap_enabled {
        return frame;
    }
    let grid = grid_note_frames(sample_rate, tempo_bpm, snap_grid);
    let lower = frame / grid * grid;
    let upper = lower.saturating_add(grid);
    if frame - lower < upper - frame {
        lower
    } else {
        upper
    }
}

fn grid_note_frames(sample_rate: u32, tempo_bpm: f64, snap_grid: SnapGrid) -> u64 {
    let grid =
        (f64::from(sample_rate) * 60.0 / tempo_bpm / f64::from(snap_grid.divisions_per_beat()))
            .round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let grid = grid.max(1.0) as u64;
    grid
}

fn pitch_class_name(root: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#/Db", "D", "D#/Eb", "E", "F", "F#/Gb", "G", "G#/Ab", "A", "A#/Bb", "B",
    ];
    NAMES[usize::from(root % 12)]
}

fn format_time(frame: u64, sample_rate: u32) -> String {
    let total_millis = frame.saturating_mul(1_000) / u64::from(sample_rate);
    let minutes = total_millis / 60_000;
    let seconds = total_millis / 1_000 % 60;
    let millis = total_millis % 1_000;
    format!("{minutes:02}:{seconds:02}.{millis:03}")
}

fn format_gain_db(gain: f32) -> String {
    if gain <= 0.000_001 {
        "-inf".into()
    } else {
        format!("{:.1}", 20.0 * gain.log10())
    }
}

fn midi_cc_label(controller: u8) -> String {
    let name = match controller {
        1 => "Modulation",
        7 => "Volume",
        10 => "Pan",
        11 => "Expression",
        64 => "Sustain",
        71 => "Resonance",
        74 => "Brightness",
        _ => "Controller",
    };
    format!("CC{controller} {name}")
}

fn format_musical_position(
    frame: u64,
    sample_rate: u32,
    tempo_bpm: f64,
    time_signature: TimeSignature,
) -> String {
    if sample_rate == 0 || !tempo_bpm.is_finite() || tempo_bpm <= 0.0 {
        return "001.01.000".into();
    }
    #[allow(clippy::cast_precision_loss)]
    let total_quarter_notes = frame as f64 * tempo_bpm / (f64::from(sample_rate) * 60.0);
    let total_beats = total_quarter_notes * f64::from(time_signature.denominator) / 4.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bar = (total_beats / f64::from(time_signature.numerator.max(1))).floor() as u64 + 1;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let beat = (total_beats % f64::from(time_signature.numerator.max(1))).floor() as u64 + 1;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ticks = (total_beats.fract() * 960.0).floor() as u64;
    format!("{bar:03}.{beat:02}.{ticks:03}")
}

fn frames_to_seconds(frames: u64, sample_rate: u32) -> f64 {
    if sample_rate == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    {
        frames as f64 / f64::from(sample_rate)
    }
}

fn marker_insert_index(markers: &[Marker], frame: u64) -> usize {
    markers.partition_point(|marker| marker.frame <= frame)
}

fn arranger_insert_index(sections: &[ArrangerSection], frame: u64) -> usize {
    sections.partition_point(|section| section.start_frame <= frame)
}

fn arranger_color(index: u8) -> Color32 {
    ARRANGER_COLORS[usize::from(index) % ARRANGER_COLORS.len()]
}

fn color_swatch(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, Color32::from_rgb(48, 52, 60)),
        egui::StrokeKind::Inside,
    );
}

fn recording_output_path(
    project_path: Option<&Path>,
    project_name: &str,
    track_name: &str,
) -> std::io::Result<PathBuf> {
    let base = project_path
        .and_then(Path::parent)
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(std::env::current_dir, |path| Ok(path.to_path_buf()))?;
    let directory = base.join("Recordings");
    fs::create_dir_all(&directory)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(directory.join(format!(
        "{}_{}_{}.wav",
        sanitize_file_component(project_name),
        sanitize_file_component(track_name),
        timestamp
    )))
}

fn bounce_output_path(
    project_path: Option<&Path>,
    project_name: &str,
    track_name: &str,
) -> std::io::Result<PathBuf> {
    let base = project_path
        .and_then(Path::parent)
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(std::env::current_dir, |path| Ok(path.to_path_buf()))?;
    let directory = base.join("Exports").join("Bounces");
    fs::create_dir_all(&directory)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(directory.join(format!(
        "{}_{}_bounce_{}.wav",
        sanitize_file_component(project_name),
        sanitize_file_component(track_name),
        timestamp
    )))
}

fn sanitize_file_component(value: &str) -> String {
    let cleaned = value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if cleaned.trim_matches('_').is_empty() {
        "take".into()
    } else {
        cleaned
    }
}

fn truncate_stereo_recording(samples: &mut Vec<f32>, maximum_frames: usize) {
    samples.truncate(maximum_frames.saturating_mul(2));
}

fn discard_stereo_prefix(samples: &mut Vec<f32>, frames: usize) {
    let sample_count = frames.saturating_mul(2).min(samples.len());
    samples.drain(..sample_count);
}

fn routed_stereo_input(samples: &[f32], input: TrackInput) -> Cow<'_, [f32]> {
    match input {
        TrackInput::Audio => Cow::Borrowed(samples),
        TrackInput::AudioMonoLeft | TrackInput::AudioMonoRight => {
            let channel = usize::from(input == TrackInput::AudioMonoRight);
            let mut routed = Vec::with_capacity(samples.len());
            for frame in samples.chunks_exact(2) {
                routed.extend([frame[channel], frame[channel]]);
            }
            Cow::Owned(routed)
        }
        TrackInput::MidiOmni | TrackInput::MidiChannel(_) => Cow::Borrowed(&[]),
    }
}

const fn audio_input_file_suffix(input: TrackInput) -> &'static str {
    match input {
        TrackInput::Audio => "stereo_1-2",
        TrackInput::AudioMonoLeft => "mono_1",
        TrackInput::AudioMonoRight => "mono_2",
        TrackInput::MidiOmni | TrackInput::MidiChannel(_) => "midi",
    }
}

fn bars_to_frames(
    bars: u8,
    sample_rate: u32,
    tempo_bpm: f64,
    time_signature: TimeSignature,
) -> u64 {
    if bars == 0 || sample_rate == 0 || !tempo_bpm.is_finite() || tempo_bpm <= 0.0 {
        return 0;
    }
    let quarter_notes_per_bar =
        f64::from(time_signature.numerator) * 4.0 / f64::from(time_signature.denominator);
    let frames =
        f64::from(sample_rate) * 60.0 / tempo_bpm * quarter_notes_per_bar * f64::from(bars);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        frames.round() as u64
    }
}

fn audio_clip_section(
    clip: &Clip,
    start_frame: u64,
    end_frame: u64,
    project_sample_rate: u32,
) -> Option<Clip> {
    if start_frame < clip.start_frame || end_frame > clip.end_frame() || start_frame >= end_frame {
        return None;
    }
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
    let removed_project_frames = start_frame - clip.start_frame;
    Some(Clip {
        name: clip.name.clone(),
        start_frame,
        length_frames: end_frame - start_frame,
        gain: clip.gain,
        fade_in_frames: if start_frame == clip.start_frame {
            clip.fade_in_frames.min(end_frame - start_frame)
        } else {
            0
        },
        fade_out_frames: if end_frame == clip.end_frame() {
            clip.fade_out_frames.min(end_frame - start_frame)
        } else {
            0
        },
        fade_curve: clip.fade_curve,
        source: ClipSource::AudioFile {
            path: path.clone(),
            source_offset_frames: source_offset_frames.saturating_add(rescale_frames_round(
                removed_project_frames,
                project_sample_rate,
                *source_sample_rate,
            )),
            source_sample_rate: *source_sample_rate,
            channels: *channels,
            reversed: *reversed,
        },
    })
}

/// Places a recording on the audible comp lane. If it overlaps an existing
/// audio clip, that performance is retained as an alternate take and only the
/// recorded range is replaced.
fn comp_recording_into_track(
    track: &mut Track,
    recorded_clip: Clip,
    project_sample_rate: u32,
) -> usize {
    let recorded_end = recorded_clip.end_frame();
    let overlap = track.clips.iter().position(|clip| {
        matches!(clip.source, ClipSource::AudioFile { .. })
            && clip.start_frame < recorded_end
            && clip.end_frame() > recorded_clip.start_frame
    });
    let Some(overlap_index) = overlap else {
        let insert_index = track
            .clips
            .partition_point(|clip| clip.start_frame <= recorded_clip.start_frame);
        track.clips.insert(insert_index, recorded_clip);
        return insert_index;
    };

    let previous = track.clips.remove(overlap_index);
    if let Some(take) = AudioTake::from_clip(&previous) {
        track.take_lanes.push(take);
    }
    let mut insertion = overlap_index;
    if previous.start_frame < recorded_clip.start_frame
        && let Some(left) = audio_clip_section(
            &previous,
            previous.start_frame,
            recorded_clip.start_frame.min(previous.end_frame()),
            project_sample_rate,
        )
    {
        track.clips.insert(insertion, left);
        insertion += 1;
    }
    let selected_index = insertion;
    track.clips.insert(insertion, recorded_clip);
    insertion += 1;
    if previous.end_frame() > recorded_end
        && let Some(right) = audio_clip_section(
            &previous,
            recorded_end.max(previous.start_frame),
            previous.end_frame(),
            project_sample_rate,
        )
    {
        track.clips.insert(insertion, right);
    }
    selected_index
}

#[derive(Clone, Copy)]
struct LoopRecordingSource<'a> {
    base_name: &'a str,
    path: &'a str,
    source_sample_rate: u32,
    project_sample_rate: u32,
    channels: u16,
    record_start_frame: u64,
    recorded_length_frames: u64,
    loop_start_frame: u64,
    loop_end_frame: u64,
}

fn comp_loop_recording_into_track(track: &mut Track, source: LoopRecordingSource<'_>) -> usize {
    let takes = loop_recording_clips(source);
    if takes.is_empty() {
        return track.clips.len();
    }
    let active = takes[takes.len() - 1].clone();
    for take in takes[..takes.len() - 1]
        .iter()
        .filter_map(AudioTake::from_clip)
    {
        track.take_lanes.push(take);
    }
    comp_recording_into_track(track, active, source.project_sample_rate)
}

fn loop_recording_clips(source: LoopRecordingSource<'_>) -> Vec<Clip> {
    let loop_length = source
        .loop_end_frame
        .saturating_sub(source.loop_start_frame);
    if loop_length == 0 || source.recorded_length_frames == 0 {
        return Vec::new();
    }
    let mut clips = Vec::new();
    let mut offset = 0;
    while offset < source.recorded_length_frames {
        let length = loop_length.min(source.recorded_length_frames - offset);
        clips.push(Clip {
            name: format!("{} Pass {}", source.base_name, clips.len() + 1),
            start_frame: source.loop_start_frame,
            length_frames: length,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::AudioFile {
                path: source.path.to_owned(),
                source_offset_frames: source
                    .record_start_frame
                    .saturating_sub(source.loop_start_frame)
                    .saturating_add(offset),
                source_sample_rate: source.source_sample_rate,
                channels: source.channels,
                reversed: false,
            },
        });
        offset = offset.saturating_add(length);
    }
    clips
}

fn clip_from_take_section(
    take: &AudioTake,
    start_frame: u64,
    end_frame: u64,
    project_sample_rate: u32,
) -> Option<Clip> {
    audio_clip_section(&take.to_clip(), start_frame, end_frame, project_sample_rate)
}

fn with_extension_if_missing(mut path: PathBuf, extension: &str) -> PathBuf {
    if path.extension().is_none() {
        path.set_extension(extension);
    }
    path
}

const PIANO_MIN_MIDI_NOTE: u8 = 21;
const PIANO_MAX_MIDI_NOTE: u8 = 108;

fn musical_pitch_editor(
    ui: &mut egui::Ui,
    clip_id: (usize, usize),
    frequency_hz: &mut f32,
) -> bool {
    let mut midi_note =
        frequency_to_midi_note(*frequency_hz).clamp(PIANO_MIN_MIDI_NOTE, PIANO_MAX_MIDI_NOTE);
    let mut changed = false;

    ui.label("Pitch");
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                midi_note >= PIANO_MIN_MIDI_NOTE.saturating_add(12),
                egui::Button::new("Oct -"),
            )
            .on_hover_text("One octave down")
            .clicked()
        {
            midi_note = midi_note.saturating_sub(12);
            changed = true;
        }
        if ui
            .add_enabled(midi_note > PIANO_MIN_MIDI_NOTE, egui::Button::new("- Semi"))
            .on_hover_text("One semitone down")
            .clicked()
        {
            midi_note = midi_note.saturating_sub(1);
            changed = true;
        }
        if ui
            .add_enabled(midi_note < PIANO_MAX_MIDI_NOTE, egui::Button::new("+ Semi"))
            .on_hover_text("One semitone up")
            .clicked()
        {
            midi_note = midi_note.saturating_add(1);
            changed = true;
        }
        if ui
            .add_enabled(
                midi_note <= PIANO_MAX_MIDI_NOTE.saturating_sub(12),
                egui::Button::new("Oct +"),
            )
            .on_hover_text("One octave up")
            .clicked()
        {
            midi_note = midi_note.saturating_add(12);
            changed = true;
        }
    });

    egui::ComboBox::from_id_salt(("musical_pitch", clip_id))
        .selected_text(format!("Note {}", midi_note_name(midi_note)))
        .width(110.0)
        .show_ui(ui, |ui| {
            for candidate in PIANO_MIN_MIDI_NOTE..=PIANO_MAX_MIDI_NOTE {
                if ui
                    .selectable_value(&mut midi_note, candidate, midi_note_name(candidate))
                    .changed()
                {
                    changed = true;
                }
            }
        });

    if changed {
        *frequency_hz = midi_note_frequency(midi_note);
    }
    ui.small(format!(
        "{} · {:.2} Hz · A4 = 440 Hz",
        midi_note_name(midi_note),
        *frequency_hz
    ));
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_uses_the_selected_musical_grid() {
        assert_eq!(
            snap_frame(5_900, 48_000, 120.0, true, SnapGrid::Sixteenth),
            6_000
        );
        assert_eq!(
            snap_frame(7_000, 48_000, 120.0, true, SnapGrid::Eighth),
            12_000
        );
        assert_eq!(
            snap_frame(5_900, 48_000, 120.0, false, SnapGrid::Quarter),
            5_900
        );
    }

    #[test]
    fn time_format_is_stable() {
        assert_eq!(format_time(72_000, 48_000), "00:01.500");
    }

    #[test]
    fn musical_position_uses_four_four_bars_and_960_ticks() {
        let four_four = TimeSignature::default();
        assert_eq!(
            format_musical_position(0, 48_000, 120.0, four_four),
            "001.01.000"
        );
        assert_eq!(
            format_musical_position(24_000, 48_000, 120.0, four_four),
            "001.02.000"
        );
        assert_eq!(
            format_musical_position(96_000, 48_000, 120.0, four_four),
            "002.01.000"
        );
    }

    #[test]
    fn meters_follow_active_clip_gain_and_mute() {
        let mut track = Track::new("Metered");
        track.gain = 0.5;
        track.clips.push(Clip {
            name: "Note".into(),
            start_frame: 100,
            length_frames: 100,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::Midi {
                notes: vec![MidiNote {
                    start_frame: 0,
                    length_frames: 100,
                    midi_note: 60,
                    velocity: 127,
                }],
                amplitude: 1.0,
            },
        });

        let meter = estimate_track_meter(&track, 120);
        assert!((meter.left - 0.5).abs() < 0.001);
        assert!((meter.right - 0.5).abs() < 0.001);
        assert_eq!(estimate_track_meter(&track, 250), LevelMeter::SILENCE);

        track.muted = true;
        assert_eq!(estimate_track_meter(&track, 120), LevelMeter::SILENCE);
    }

    #[test]
    fn normalize_gain_uses_selected_audio_range_peak() {
        let samples = vec![0.1, -0.1, 0.2, -0.2, 0.5, -0.25, 0.05, -0.05];
        let gain = normalized_clip_gain(&samples, 1, 2, 48_000, 48_000).unwrap();
        assert!((gain - 1.96).abs() < 0.001);
        assert!(normalized_clip_gain(&[0.0, 0.0, 0.0, 0.0], 0, 2, 48_000, 48_000).is_none());
    }

    #[test]
    fn slip_edit_converts_project_frames_to_source_frames() {
        assert_eq!(
            slipped_source_offset(1_000, 48_000, 48_000, 44_100, Some(80_000)),
            45_100
        );
        assert_eq!(
            slipped_source_offset(1_000, -48_000, 48_000, 44_100, Some(80_000)),
            0
        );
        assert_eq!(
            slipped_source_offset(79_000, 48_000, 48_000, 44_100, Some(80_000)),
            79_999
        );
    }

    #[test]
    fn file_dialog_paths_receive_missing_extensions() {
        assert_eq!(
            with_extension_if_missing(PathBuf::from("song"), "dmo"),
            PathBuf::from("song.dmo")
        );
        assert_eq!(
            with_extension_if_missing(PathBuf::from("song.project"), "dmo"),
            PathBuf::from("song.project")
        );
    }

    #[test]
    fn audio_file_fingerprint_detects_size_changes() {
        let path = std::env::temp_dir().join(format!(
            "dmo-fingerprint-{}-{}.wav",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::write(&path, b"one").unwrap();
        let first = audio_file_fingerprint(&path).unwrap();
        fs::write(&path, b"one-two").unwrap();
        let second = audio_file_fingerprint(&path).unwrap();
        let _ = fs::remove_file(path);

        assert_ne!(first.len, second.len);
        assert_ne!(first, second);
    }

    #[test]
    fn channel_strip_presets_create_useful_insert_chains() {
        let vocal = ChannelStripPreset::CleanVocal.inserts();
        assert!(matches!(
            vocal[0].effect,
            InsertEffect::ThreeBandEq { high_db, .. } if high_db > 0.0
        ));
        assert!(matches!(
            vocal[1].effect,
            InsertEffect::Compressor { ratio, .. } if ratio >= 3.0
        ));

        let master = ChannelStripPreset::MasterGlue.inserts();
        assert_eq!(master.len(), 2);
        assert!(matches!(
            master[0].effect,
            InsertEffect::Compressor { ratio, .. } if ratio <= 2.0
        ));
        assert!(
            ChannelStripPreset::ALL
                .into_iter()
                .all(|preset| !preset.inserts().is_empty())
        );
    }

    #[test]
    fn recording_names_are_safe_and_punch_ranges_truncate_stereo_frames() {
        assert_eq!(sanitize_file_component("Lead / Vox: 1"), "Lead___Vox__1");
        assert_eq!(sanitize_file_component("///"), "take");
        let base = std::env::temp_dir().join(format!("dmo-bounce-test-{}", std::process::id()));
        let project_path = base.join("song.dmo");
        let bounce = bounce_output_path(Some(&project_path), "Song / 1", "Lead:Vox").unwrap();
        assert_eq!(
            bounce.parent().unwrap(),
            base.join("Exports").join("Bounces")
        );
        assert!(
            bounce
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Song___1_Lead_Vox_bounce_")
        );
        let mut samples = vec![0.0; 12];
        truncate_stereo_recording(&mut samples, 4);
        assert_eq!(samples.len(), 8);
        discard_stereo_prefix(&mut samples, 2);
        assert_eq!(samples.len(), 4);
        let four_four = TimeSignature::default();
        assert_eq!(bars_to_frames(1, 48_000, 120.0, four_four), 96_000);
        assert_eq!(bars_to_frames(4, 48_000, 120.0, four_four), 384_000);
        assert_eq!(bars_to_frames(0, 48_000, 120.0, four_four), 0);
    }

    #[test]
    fn audio_input_routes_stereo_and_mono_channels() {
        let samples = [0.1, 0.2, 0.3, 0.4];
        assert_eq!(
            routed_stereo_input(&samples, TrackInput::Audio).as_ref(),
            samples
        );
        assert_eq!(
            routed_stereo_input(&samples, TrackInput::AudioMonoLeft).as_ref(),
            [0.1, 0.1, 0.3, 0.3]
        );
        assert_eq!(
            routed_stereo_input(&samples, TrackInput::AudioMonoRight).as_ref(),
            [0.2, 0.2, 0.4, 0.4]
        );
    }

    #[test]
    fn overlapping_recording_creates_a_take_and_replaces_only_its_range() {
        let mut track = Track::new("Vocal");
        track.clips.push(Clip {
            name: "Old".into(),
            start_frame: 0,
            length_frames: 100,
            gain: 1.0,
            fade_in_frames: 10,
            fade_out_frames: 10,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::AudioFile {
                path: "old.wav".into(),
                source_offset_frames: 5,
                source_sample_rate: 48_000,
                channels: 2,
                reversed: false,
            },
        });
        let new_take = Clip {
            name: "New".into(),
            start_frame: 30,
            length_frames: 40,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::AudioFile {
                path: "new.wav".into(),
                source_offset_frames: 0,
                source_sample_rate: 48_000,
                channels: 2,
                reversed: false,
            },
        };

        let selected = comp_recording_into_track(&mut track, new_take, 48_000);

        assert_eq!(selected, 1);
        assert_eq!(track.clips.len(), 3);
        assert_eq!(track.take_lanes.len(), 1);
        assert_eq!(track.take_lanes[0].path, "old.wav");
        assert_eq!(track.clips[0].length_frames, 30);
        assert_eq!(track.clips[1].start_frame, 30);
        assert_eq!(track.clips[1].length_frames, 40);
        assert_eq!(track.clips[2].start_frame, 70);
        assert!(matches!(
            track.clips[2].source,
            ClipSource::AudioFile {
                source_offset_frames: 75,
                ..
            }
        ));

        let comp = clip_from_take_section(&track.take_lanes[0], 30, 70, 48_000).unwrap();
        assert_eq!(comp.start_frame, 30);
        assert_eq!(comp.length_frames, 40);
        assert!(matches!(
            comp.source,
            ClipSource::AudioFile {
                source_offset_frames: 35,
                ..
            }
        ));
    }

    #[test]
    fn loop_recording_splits_passes_into_active_clip_and_take_lanes() {
        let mut track = Track::new("Guitar");
        let selected = comp_loop_recording_into_track(
            &mut track,
            LoopRecordingSource {
                base_name: "Guitar Take",
                path: "loop.wav",
                source_sample_rate: 48_000,
                project_sample_rate: 48_000,
                channels: 2,
                record_start_frame: 100,
                recorded_length_frames: 250,
                loop_start_frame: 100,
                loop_end_frame: 200,
            },
        );

        assert_eq!(selected, 0);
        assert_eq!(track.clips.len(), 1);
        assert_eq!(track.take_lanes.len(), 2);
        assert_eq!(track.clips[0].name, "Guitar Take Pass 3");
        assert_eq!(track.clips[0].start_frame, 100);
        assert_eq!(track.clips[0].length_frames, 50);
        assert_eq!(track.take_lanes[0].name, "Guitar Take Pass 1");
        assert_eq!(track.take_lanes[0].source_offset_frames, 0);
        assert_eq!(track.take_lanes[1].source_offset_frames, 100);
    }

    #[test]
    fn first_available_clip_is_selected() {
        let mut project = Project::new("Selection", 48_000, 120.0).unwrap();
        project.tracks.push(Track::new("Empty"));
        let mut notes = Track::new("Notes");
        notes.clips.push(Clip {
            name: "C4".into(),
            start_frame: 0,
            length_frames: 1,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::Sine {
                frequency_hz: 261.63,
                amplitude: 0.5,
            },
        });
        project.tracks.push(notes);

        assert_eq!(first_clip_selection(&project), Some((1, 0)));
    }

    #[test]
    fn first_note_is_selected_for_a_midi_clip() {
        let project = demo_project(48_000);
        assert_eq!(
            first_note_selection(&project, Some((0, 0))),
            Some((0, 0, 0))
        );
        assert_eq!(first_note_selection(&project, None), None);
    }

    #[test]
    fn metronome_uses_tempo_spaced_stereo_clicks() {
        assert_eq!(metronome_preview_frames(48_000, 120.0, 4), 96_000);
        let mut samples = vec![0.0; 48_001 * 2];
        mix_metronome(&mut samples, 48_000, 120.0);

        assert!(samples[2].abs() > 0.001);
        assert!(samples[(24_000 + 1) * 2].abs() > 0.001);
        assert_eq!(samples[2].to_bits(), samples[3].to_bits());
    }

    #[test]
    fn demo_melody_is_one_clip_with_four_notes() {
        let project = demo_project(48_000);
        let melody = &project.tracks[0];

        assert_eq!(melody.clips.len(), 1);
        assert!(matches!(
            &melody.clips[0].source,
            ClipSource::Midi { notes, .. } if notes.len() == 4
        ));
    }

    #[test]
    fn midi_split_crops_notes_across_the_boundary() {
        let clip = Clip {
            name: "Part".into(),
            start_frame: 1_000,
            length_frames: 100,
            gain: 0.75,
            fade_in_frames: 10,
            fade_out_frames: 20,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::Midi {
                notes: vec![MidiNote {
                    start_frame: 20,
                    length_frames: 70,
                    midi_note: 60,
                    velocity: 100,
                }],
                amplitude: 0.8,
            },
        };

        let (left, right) = split_clip_at_frame(&clip, 1_060, 48_000).unwrap();

        let ClipSource::Midi {
            notes: left_notes, ..
        } = left.source
        else {
            panic!("expected MIDI");
        };
        let ClipSource::Midi {
            notes: right_notes, ..
        } = right.source
        else {
            panic!("expected MIDI");
        };
        assert_eq!(left.length_frames, 60);
        assert_eq!(left_notes[0].length_frames, 40);
        assert_eq!(right.start_frame, 1_060);
        assert_eq!(right_notes[0].start_frame, 0);
        assert_eq!(right_notes[0].length_frames, 30);
        assert_eq!(left_notes[0].velocity, 100);
        assert_eq!(right_notes[0].velocity, 100);
        assert_eq!(left.gain.to_bits(), 0.75_f32.to_bits());
        assert_eq!(left.fade_in_frames, 10);
        assert_eq!(left.fade_out_frames, 0);
        assert_eq!(right.gain.to_bits(), 0.75_f32.to_bits());
        assert_eq!(right.fade_in_frames, 0);
        assert_eq!(right.fade_out_frames, 20);
    }

    #[test]
    fn audio_split_advances_the_native_source_offset() {
        let clip = Clip {
            name: "Take".into(),
            start_frame: 0,
            length_frames: 96_000,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::AudioFile {
                path: "take.wav".into(),
                source_offset_frames: 100,
                source_sample_rate: 44_100,
                channels: 2,
                reversed: false,
            },
        };

        let (_, right) = split_clip_at_frame(&clip, 48_000, 48_000).unwrap();

        assert!(matches!(
            right.source,
            ClipSource::AudioFile {
                source_offset_frames: 44_200,
                ..
            }
        ));
    }

    #[test]
    fn left_trim_keeps_audio_position_across_sample_rates() {
        let mut project = Project::new("Trim", 48_000, 120.0).unwrap();
        let mut track = Track::new("Audio");
        track.clips.push(Clip {
            name: "Take".into(),
            start_frame: 1_000,
            length_frames: 48_000,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::AudioFile {
                path: "take.wav".into(),
                source_offset_frames: 100,
                source_sample_rate: 44_100,
                channels: 2,
                reversed: false,
            },
        });
        project.tracks.push(track);

        assert_eq!(
            clip_trim_command(&project, Some((0, 0)), 25_000, TrimSide::Left),
            Some(EditCommand::SetClipTiming {
                track_index: 0,
                clip_index: 0,
                start_frame: 25_000,
                length_frames: 24_000,
                source_offset_frames: Some(22_150),
            })
        );
    }

    #[test]
    fn trim_rejects_boundaries_and_right_trim_keeps_sine_source() {
        let project = demo_project(48_000);
        let clip = &project.tracks[0].clips[0];
        assert!(
            clip_trim_command(&project, Some((0, 0)), clip.start_frame, TrimSide::Right,).is_none()
        );
        assert!(
            clip_trim_command(&project, Some((0, 0)), clip.start_frame + 1, TrimSide::Left,)
                .is_none()
        );
        assert!(matches!(
            clip_trim_command(
                &project,
                Some((0, 0)),
                clip.start_frame + 1,
                TrimSide::Right,
            ),
            Some(EditCommand::SetClipTiming {
                length_frames: 1,
                source_offset_frames: None,
                ..
            })
        ));
    }

    #[test]
    fn live_midi_parser_handles_notes_pitch_and_pressure() {
        let now = Instant::now();
        let note = LiveMidiMessage::from_bytes_at(&[0x91, 64, 100], now).unwrap();
        let bend = LiveMidiMessage::from_bytes_at(&[0xE1, 0, 96], now).unwrap();
        let pressure = LiveMidiMessage::from_bytes_at(&[0xD1, 77], now).unwrap();
        let program = LiveMidiMessage::from_bytes_at(&[0xC1, 41], now).unwrap();

        assert_eq!(
            parse_live_midi(note),
            Some(ParsedLiveMidi::NoteOn {
                channel: 2,
                note: 64,
                velocity: 100,
            })
        );
        assert_eq!(
            parse_live_midi(bend),
            Some(ParsedLiveMidi::PitchBend {
                channel: 2,
                value: 4096,
            })
        );
        assert_eq!(
            parse_live_midi(pressure),
            Some(ParsedLiveMidi::ChannelPressure {
                channel: 2,
                value: 77,
            })
        );
        assert_eq!(
            parse_live_midi(program),
            Some(ParsedLiveMidi::ProgramChange {
                channel: 2,
                program: 42,
            })
        );
    }

    #[test]
    fn live_midi_recording_filters_channels_and_closes_held_notes() {
        let mut take = ActiveMidiTake {
            track_index: 0,
            input: TrackInput::MidiChannel(2),
            active_notes: HashMap::new(),
            notes: Vec::new(),
            controllers: Vec::new(),
            pitch_bend: Vec::new(),
            channel_pressure: Vec::new(),
            poly_pressure: Vec::new(),
            program_changes: Vec::new(),
        };
        record_midi_event(
            &mut take,
            ParsedLiveMidi::NoteOn {
                channel: 1,
                note: 60,
                velocity: 100,
            },
            100,
            100,
        );
        record_midi_event(
            &mut take,
            ParsedLiveMidi::NoteOn {
                channel: 2,
                note: 64,
                velocity: 110,
            },
            120,
            100,
        );
        record_midi_event(
            &mut take,
            ParsedLiveMidi::PitchBend {
                channel: 2,
                value: -2048,
            },
            140,
            100,
        );
        record_midi_event(
            &mut take,
            ParsedLiveMidi::ProgramChange {
                channel: 2,
                program: 42,
            },
            160,
            100,
        );
        finish_active_midi_notes(&mut take, 220, 100);

        assert_eq!(take.notes.len(), 1);
        assert_eq!(take.notes[0].midi_note, 64);
        assert_eq!(take.notes[0].start_frame, 120);
        assert_eq!(take.notes[0].length_frames, 100);
        assert_eq!(take.pitch_bend[0].value, -2048);
        assert_eq!(take.program_changes[0].program, 42);
    }

    #[test]
    fn retrospective_midi_clip_captures_recent_complete_notes() {
        let now = Instant::now();
        let mut events = VecDeque::new();
        events.push_back(RetrospectiveMidiEvent {
            received_at: now.checked_sub(Duration::from_millis(400)).unwrap(),
            event: ParsedLiveMidi::NoteOn {
                channel: 1,
                note: 64,
                velocity: 100,
            },
        });
        events.push_back(RetrospectiveMidiEvent {
            received_at: now.checked_sub(Duration::from_millis(100)).unwrap(),
            event: ParsedLiveMidi::NoteOff {
                channel: 1,
                note: 64,
            },
        });

        let clip = retrospective_midi_clip(&events, now, Duration::from_secs(1), 1_000).unwrap();

        assert_eq!(clip.length_frames, 300);
        assert!(matches!(
            clip.source,
            ClipSource::Midi { notes, .. }
                if notes.len() == 1
                    && notes[0].start_frame == 0
                    && notes[0].length_frames == 300
                    && notes[0].midi_note == 64
        ));
    }

    #[test]
    fn midi_output_events_follow_timeline_order() {
        let mut project = Project::new("MIDI out", 48_000, 120.0).unwrap();
        let mut track = Track::new("External");
        track.midi_channel = 3;
        track.clips.push(Clip {
            name: "Part".into(),
            start_frame: 100,
            length_frames: 200,
            gain: 1.0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            fade_curve: FadeCurve::Linear,
            source: ClipSource::Midi {
                notes: vec![MidiNote {
                    start_frame: 20,
                    length_frames: 40,
                    midi_note: 64,
                    velocity: 100,
                }],
                amplitude: 0.8,
            },
        });
        track.midi_cc.push(MidiControlPoint {
            frame: 130,
            controller: 74,
            value: 91,
        });
        track.midi_program_changes.push(MidiProgramChangePoint {
            frame: 125,
            program: 42,
        });
        project.tracks.push(track);

        assert_eq!(
            midi_output_events_between(&project, 100, 180),
            vec![
                MidiPlaybackEvent::NoteOn {
                    channel: 3,
                    note: 64,
                    velocity: 100,
                },
                MidiPlaybackEvent::ProgramChange {
                    channel: 3,
                    program: 42,
                },
                MidiPlaybackEvent::Control {
                    channel: 3,
                    controller: 74,
                    value: 91,
                },
                MidiPlaybackEvent::NoteOff {
                    channel: 3,
                    note: 64,
                },
            ]
        );
    }

    #[test]
    fn midi_output_bytes_encode_pitch_bend_center_and_limits() {
        assert_eq!(
            midi_playback_event_bytes(MidiPlaybackEvent::PitchBend {
                channel: 1,
                value: 0,
            }),
            [0xE0, 0, 64]
        );
        assert_eq!(
            midi_playback_event_bytes(MidiPlaybackEvent::PitchBend {
                channel: 16,
                value: 8191,
            }),
            [0xEF, 127, 127]
        );
        assert_eq!(
            midi_playback_event_bytes(MidiPlaybackEvent::NoteOn {
                channel: 2,
                note: 60,
                velocity: 127,
            }),
            [0x91, 60, 127]
        );
        assert_eq!(
            midi_playback_event_bytes(MidiPlaybackEvent::ProgramChange {
                channel: 3,
                program: 42,
            }),
            [0xC2, 41, 0]
        );
    }
}
