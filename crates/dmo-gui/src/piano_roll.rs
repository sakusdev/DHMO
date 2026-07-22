//! Piano-roll painting and note-grid interaction.

use dmo_core::{Clip, ClipSource, Project, frequency_to_midi_note, midi_note_name};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

const KEY_WIDTH: f32 = 66.0;
const RULER_HEIGHT: f32 = 25.0;
const NOTE_HEIGHT: f32 = 16.0;
const MIN_PIANO_NOTE: u8 = 21;
const MAX_PIANO_NOTE: u8 = 108;
const MIN_VISIBLE_ROWS: u8 = 24;
const CC_LANE_HEIGHT: f32 = 86.0;

#[derive(Debug, Clone, Copy)]
pub struct PianoRollView {
    pub pixels_per_second: f32,
    pub selected_track: Option<usize>,
    pub selected_clip: Option<(usize, usize)>,
    pub selected_note: Option<(usize, usize, usize)>,
    pub playhead_frame: u64,
    pub scroll_to_selection: bool,
    pub snap_enabled: bool,
    pub grid_divisions_per_beat: u8,
    pub cc_controller: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct AddNoteRequest {
    pub track_index: usize,
    pub start_frame: u64,
    pub midi_note: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct DeleteNoteRequest {
    pub track: usize,
    pub clip: usize,
    pub note: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct ResizeNoteRequest {
    pub track_index: usize,
    pub clip_index: usize,
    pub note_index: usize,
    pub length_frames: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct MoveNoteRequest {
    pub track_index: usize,
    pub clip_index: usize,
    pub note_index: usize,
    pub start_frame: u64,
    pub midi_note: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct WriteCcRequest {
    pub track_index: usize,
    pub frame: u64,
    pub controller: u8,
    pub value: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PianoRollInteraction {
    pub selected_clip: Option<(usize, usize)>,
    pub selected_note: Option<(usize, usize, usize)>,
    pub seek_frame: Option<u64>,
    pub add_note: Option<AddNoteRequest>,
    pub delete_note: Option<DeleteNoteRequest>,
    pub resize_note: Option<ResizeNoteRequest>,
    pub move_note: Option<MoveNoteRequest>,
    pub write_cc: Option<WriteCcRequest>,
}

#[derive(Debug, Clone, Copy)]
struct NoteEvent {
    start_frame: u64,
    length_frames: u64,
    midi_note: u8,
    source_note_index: Option<usize>,
}

impl PianoRollView {
    #[allow(clippy::too_many_lines)]
    pub fn show(self, ui: &mut egui::Ui, project: &Project) -> PianoRollInteraction {
        let Some(track_index) = self
            .selected_track
            .filter(|index| *index < project.tracks.len())
        else {
            ui.centered_and_justified(|ui| ui.label("Select a track to edit notes."));
            return PianoRollInteraction::default();
        };
        let track = &project.tracks[track_index];
        let (lowest_note, highest_note) = visible_note_range(project, track_index);
        let duration_frames = project
            .duration_frames()
            .max(u64::from(project.sample_rate) * 8);
        #[allow(clippy::cast_precision_loss)]
        let duration_width =
            duration_frames as f32 / project.sample_rate as f32 * self.pixels_per_second + 220.0;
        let row_count = u16::from(highest_note) - u16::from(lowest_note) + 1;
        let desired_size = Vec2::new(
            (KEY_WIDTH + duration_width).max(ui.available_width()),
            RULER_HEIGHT + f32::from(row_count) * NOTE_HEIGHT + CC_LANE_HEIGHT,
        );

        let mut scroll_area = egui::ScrollArea::both().auto_shrink([false, false]);
        if self.scroll_to_selection
            && let Some((selected_track, selected_clip)) = self.selected_clip
            && selected_track == track_index
            && let Some(clip) = track.clips.get(selected_clip)
            && let Some(selected_note) = note_events(clip).first().map(|note| note.midi_note)
        {
            let focus_note = track_note_bounds(project, track_index)
                .filter(|(min, max)| {
                    f32::from(max.saturating_sub(*min).saturating_add(1)) * NOTE_HEIGHT
                        <= ui.available_height()
                })
                .map_or(selected_note, |(min, max)| {
                    u8::try_from(u16::midpoint(u16::from(min), u16::from(max)))
                        .unwrap_or(selected_note)
                });
            let row = f32::from(highest_note.saturating_sub(focus_note));
            let selected_center = RULER_HEIGHT + (row + 0.5) * NOTE_HEIGHT;
            scroll_area = scroll_area
                .vertical_scroll_offset((selected_center - ui.available_height() * 0.5).max(0.0));
        }

        let mut interaction = PianoRollInteraction::default();
        scroll_area.show(ui, |ui| {
            let (response, painter) = ui.allocate_painter(desired_size, Sense::click());
            let rect = response.rect;
            paint_note_rows(&painter, rect, lowest_note, highest_note);
            paint_time_grid(
                &painter,
                rect,
                project,
                self.pixels_per_second,
                self.grid_divisions_per_beat,
            );
            paint_notes(
                &painter,
                rect,
                project,
                track_index,
                lowest_note,
                highest_note,
                self.pixels_per_second,
                self.selected_clip,
                self.selected_note,
            );
            let cc_rect = Rect::from_min_max(
                Pos2::new(
                    rect.left(),
                    rect.top() + RULER_HEIGHT + f32::from(row_count) * NOTE_HEIGHT,
                ),
                rect.right_bottom(),
            );
            paint_cc_lane(
                &painter,
                cc_rect,
                project,
                track_index,
                self.cc_controller,
                self.pixels_per_second,
            );
            paint_playhead(
                &painter,
                rect,
                project.sample_rate,
                self.playhead_frame,
                self.pixels_per_second,
            );

            let mut note_clicked = false;
            for (clip_index, clip) in track.clips.iter().enumerate() {
                for (event_index, note) in note_events(clip).into_iter().enumerate() {
                    if !(lowest_note..=highest_note).contains(&note.midi_note) {
                        continue;
                    }
                    let note_bounds = note_rect(
                        rect,
                        project.sample_rate,
                        note.start_frame,
                        note.length_frames,
                        note.midi_note,
                        highest_note,
                        self.pixels_per_second,
                    );
                    let note_response = ui
                        .interact(
                            note_bounds,
                            ui.make_persistent_id((
                                "piano_note",
                                track_index,
                                clip_index,
                                event_index,
                            )),
                            Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab);
                    if note_response.double_clicked()
                        && let Some(note_index) = note.source_note_index
                    {
                        note_clicked = true;
                        interaction.delete_note = Some(DeleteNoteRequest {
                            track: track_index,
                            clip: clip_index,
                            note: note_index,
                        });
                    } else if note_response.clicked() {
                        note_clicked = true;
                        interaction.selected_clip = Some((track_index, clip_index));
                        if let Some(note_index) = note.source_note_index {
                            interaction.selected_note = Some((track_index, clip_index, note_index));
                        }
                        interaction.seek_frame = Some(note.start_frame);
                    }

                    if (note_response.dragged() || note_response.drag_stopped())
                        && let Some(note_index) = note.source_note_index
                        && let Some(pointer) = ui.input(|input| input.pointer.interact_pos())
                    {
                        note_clicked = true;
                        let midi_note = midi_note_at_y(rect, pointer.y, lowest_note, highest_note);
                        let start_frame = moved_note_start(
                            note.start_frame,
                            note_response.drag_delta().x,
                            project.sample_rate,
                            project.tempo_bpm,
                            self.pixels_per_second,
                            self.snap_enabled,
                            self.grid_divisions_per_beat,
                        );
                        let preview = note_rect(
                            rect,
                            project.sample_rate,
                            start_frame,
                            note.length_frames,
                            midi_note,
                            highest_note,
                            self.pixels_per_second,
                        );
                        painter.rect_stroke(
                            preview,
                            3.0,
                            Stroke::new(2.0, Color32::from_rgb(95, 231, 174)),
                            StrokeKind::Inside,
                        );
                        painter.text(
                            preview.left_top() - Vec2::new(0.0, 3.0),
                            Align2::LEFT_BOTTOM,
                            midi_note_name(midi_note),
                            FontId::monospace(10.0),
                            Color32::from_rgb(150, 255, 211),
                        );
                        if note_response.drag_stopped() {
                            interaction.move_note = Some(MoveNoteRequest {
                                track_index,
                                clip_index,
                                note_index,
                                start_frame,
                                midi_note,
                            });
                        }
                    }

                    if let Some(note_index) = note.source_note_index {
                        let handle_rect = Rect::from_center_size(
                            Pos2::new(note_bounds.right() - 1.5, note_bounds.center().y),
                            Vec2::new(8.0, note_bounds.height()),
                        );
                        let handle_response = ui
                            .interact(
                                handle_rect,
                                ui.make_persistent_id((
                                    "piano_note_end",
                                    track_index,
                                    clip_index,
                                    note_index,
                                )),
                                Sense::drag(),
                            )
                            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                        if handle_response.dragged() || handle_response.drag_stopped() {
                            note_clicked = true;
                            if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                                let end_frame = frame_at_x(
                                    rect,
                                    pointer.x,
                                    project.sample_rate,
                                    self.pixels_per_second,
                                );
                                let length_frames = resized_note_length(
                                    note.start_frame,
                                    end_frame,
                                    project.sample_rate,
                                    project.tempo_bpm,
                                    self.snap_enabled,
                                    self.grid_divisions_per_beat,
                                );
                                let preview = note_rect(
                                    rect,
                                    project.sample_rate,
                                    note.start_frame,
                                    length_frames,
                                    note.midi_note,
                                    highest_note,
                                    self.pixels_per_second,
                                );
                                painter.rect_stroke(
                                    preview,
                                    3.0,
                                    Stroke::new(2.0, Color32::from_rgb(255, 218, 92)),
                                    StrokeKind::Inside,
                                );
                                if handle_response.drag_stopped() {
                                    interaction.resize_note = Some(ResizeNoteRequest {
                                        track_index,
                                        clip_index,
                                        note_index,
                                        length_frames,
                                    });
                                }
                            }
                        }
                    }
                }
            }

            if !note_clicked
                && response.clicked()
                && let Some(pointer) = response.interact_pointer_pos()
            {
                if cc_rect.contains(pointer) && pointer.x >= rect.left() + KEY_WIDTH {
                    let frame =
                        frame_at_x(rect, pointer.x, project.sample_rate, self.pixels_per_second);
                    let normalized =
                        ((cc_rect.bottom() - pointer.y) / cc_rect.height()).clamp(0.0, 1.0);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let value = (normalized * 127.0).round() as u8;
                    interaction.write_cc = Some(WriteCcRequest {
                        track_index,
                        frame,
                        controller: self.cc_controller,
                        value,
                    });
                } else if let Some((start_frame, midi_note)) = grid_position(
                    rect,
                    pointer,
                    project.sample_rate,
                    self.pixels_per_second,
                    lowest_note,
                    highest_note,
                ) {
                    interaction.add_note = Some(AddNoteRequest {
                        track_index,
                        start_frame,
                        midi_note,
                    });
                }
            }
        });
        interaction
    }
}

fn paint_cc_lane(
    painter: &egui::Painter,
    rect: Rect,
    project: &Project,
    track_index: usize,
    controller: u8,
    pixels_per_second: f32,
) {
    painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 22, 29));
    painter.hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0, Color32::from_rgb(78, 86, 102)),
    );
    painter.rect_filled(
        Rect::from_min_max(
            rect.left_top(),
            Pos2::new(rect.left() + KEY_WIDTH, rect.bottom()),
        ),
        0.0,
        Color32::from_rgb(31, 36, 45),
    );
    painter.text(
        Pos2::new(rect.left() + 6.0, rect.top() + 6.0),
        Align2::LEFT_TOP,
        format!("CC{controller}"),
        FontId::monospace(11.0),
        Color32::from_rgb(164, 177, 198),
    );
    painter.text(
        Pos2::new(rect.left() + 6.0, rect.bottom() - 6.0),
        Align2::LEFT_BOTTOM,
        "0—127",
        FontId::monospace(9.0),
        Color32::from_gray(135),
    );
    let mut points = project.tracks[track_index]
        .midi_cc
        .iter()
        .filter(|point| point.controller == controller)
        .collect::<Vec<_>>();
    points.sort_by_key(|point| point.frame);
    let position = |point: &dmo_core::MidiControlPoint| {
        #[allow(clippy::cast_precision_loss)]
        let seconds = point.frame as f32 / project.sample_rate as f32;
        let x = rect.left() + KEY_WIDTH + seconds * pixels_per_second;
        let y = rect.bottom() - f32::from(point.value) / 127.0 * rect.height();
        Pos2::new(x, y)
    };
    let color = Color32::from_rgb(99, 210, 169);
    for pair in points.windows(2) {
        let left = position(pair[0]);
        let right = position(pair[1]);
        painter.line_segment([left, Pos2::new(right.x, left.y)], Stroke::new(1.4, color));
        painter.line_segment(
            [Pos2::new(right.x, left.y), right],
            Stroke::new(1.0, color.gamma_multiply(0.75)),
        );
    }
    for point in points {
        let position = position(point);
        painter.vline(
            position.x,
            position.y..=rect.bottom(),
            Stroke::new(1.0, color.gamma_multiply(0.55)),
        );
        painter.circle_filled(position, 3.5, color);
    }
}

fn visible_note_range(project: &Project, track_index: usize) -> (u8, u8) {
    let (min, max) = track_note_bounds(project, track_index).unwrap_or((60, 72));
    let mut lowest = min.saturating_sub(6).max(MIN_PIANO_NOTE);
    let mut highest = max.saturating_add(6).min(MAX_PIANO_NOTE);

    let rows = highest.saturating_sub(lowest).saturating_add(1);
    if rows < MIN_VISIBLE_ROWS {
        let missing = MIN_VISIBLE_ROWS - rows;
        lowest = lowest.saturating_sub(missing / 2).max(MIN_PIANO_NOTE);
        highest = lowest
            .saturating_add(MIN_VISIBLE_ROWS - 1)
            .min(MAX_PIANO_NOTE);
        lowest = highest
            .saturating_sub(MIN_VISIBLE_ROWS - 1)
            .max(MIN_PIANO_NOTE);
    }
    (lowest, highest)
}

fn track_note_bounds(project: &Project, track_index: usize) -> Option<(u8, u8)> {
    let notes = project.tracks[track_index]
        .clips
        .iter()
        .flat_map(note_events)
        .map(|note| note.midi_note)
        .collect::<Vec<_>>();
    Some((notes.iter().copied().min()?, notes.iter().copied().max()?))
}

fn note_events(clip: &Clip) -> Vec<NoteEvent> {
    match &clip.source {
        ClipSource::Midi { notes, .. } => notes
            .iter()
            .enumerate()
            .map(|(note_index, note)| NoteEvent {
                start_frame: clip.start_frame.saturating_add(note.start_frame),
                length_frames: note.length_frames,
                midi_note: note.midi_note,
                source_note_index: Some(note_index),
            })
            .collect(),
        ClipSource::Sine { frequency_hz, .. } => vec![NoteEvent {
            start_frame: clip.start_frame,
            length_frames: clip.length_frames,
            midi_note: frequency_to_midi_note(*frequency_hz),
            source_note_index: None,
        }],
        ClipSource::AudioFile { .. } => Vec::new(),
    }
}

fn paint_note_rows(painter: &egui::Painter, rect: Rect, lowest_note: u8, highest_note: u8) {
    painter.rect_filled(rect, 0.0, Color32::from_rgb(20, 23, 29));
    for midi_note in lowest_note..=highest_note {
        let row = note_row_rect(rect, midi_note, highest_note);
        let black = is_black_key(midi_note);
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left() + KEY_WIDTH, row.top()),
                row.right_bottom(),
            ),
            0.0,
            if black {
                Color32::from_rgb(25, 29, 37)
            } else {
                Color32::from_rgb(33, 37, 46)
            },
        );
        let key_rect = Rect::from_min_max(
            row.left_top(),
            Pos2::new(rect.left() + KEY_WIDTH, row.bottom()),
        );
        painter.rect_filled(
            key_rect,
            0.0,
            if black {
                Color32::from_rgb(39, 43, 53)
            } else {
                Color32::from_rgb(195, 201, 214)
            },
        );
        painter.text(
            key_rect.right_center() - Vec2::new(6.0, 0.0),
            Align2::RIGHT_CENTER,
            midi_note_name(midi_note),
            FontId::monospace(10.0),
            if black {
                Color32::WHITE
            } else {
                Color32::from_rgb(26, 29, 36)
            },
        );
        painter.hline(
            row.x_range(),
            row.bottom(),
            Stroke::new(0.7, Color32::from_rgb(53, 58, 69)),
        );
    }
    painter.vline(
        rect.left() + KEY_WIDTH,
        rect.top()..=rect.bottom(),
        Stroke::new(1.0, Color32::from_rgb(91, 98, 113)),
    );
}

fn paint_time_grid(
    painter: &egui::Painter,
    rect: Rect,
    project: &Project,
    pixels_per_second: f32,
    grid_divisions_per_beat: u8,
) {
    let beat_seconds = 60.0 / project.tempo_bpm;
    let divisions_per_beat = grid_divisions_per_beat.max(1);
    let divisions = u64::from(divisions_per_beat);
    let grid_seconds = beat_seconds / f64::from(divisions_per_beat);
    let visible_seconds = f64::from((rect.width() - KEY_WIDTH) / pixels_per_second);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let grid_count = (visible_seconds / grid_seconds).ceil() as u64;
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(rect.left() + KEY_WIDTH, rect.top()),
            Pos2::new(rect.right(), rect.top() + RULER_HEIGHT),
        ),
        0.0,
        Color32::from_rgb(18, 20, 26),
    );

    for grid in 0..=grid_count {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let x = rect.left() + KEY_WIDTH + (grid as f64 * grid_seconds) as f32 * pixels_per_second;
        let is_beat = grid.is_multiple_of(divisions);
        let is_bar = grid.is_multiple_of(divisions * 4);
        painter.vline(
            x,
            rect.top()..=rect.bottom(),
            Stroke::new(
                if is_bar {
                    1.2
                } else if is_beat {
                    0.8
                } else {
                    0.45
                },
                if is_bar {
                    Color32::from_rgb(114, 122, 141)
                } else if is_beat {
                    Color32::from_rgb(72, 78, 92)
                } else {
                    Color32::from_rgb(48, 53, 64)
                },
            ),
        );
        if is_bar {
            painter.text(
                Pos2::new(x + 4.0, rect.top() + 5.0),
                Align2::LEFT_TOP,
                format!("{}", grid / (divisions * 4) + 1),
                FontId::monospace(10.0),
                Color32::LIGHT_GRAY,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_notes(
    painter: &egui::Painter,
    rect: Rect,
    project: &Project,
    track_index: usize,
    lowest_note: u8,
    highest_note: u8,
    pixels_per_second: f32,
    selected_clip: Option<(usize, usize)>,
    selected_note: Option<(usize, usize, usize)>,
) {
    for (clip_index, clip) in project.tracks[track_index].clips.iter().enumerate() {
        let selected = selected_clip == Some((track_index, clip_index));
        for note in note_events(clip) {
            if !(lowest_note..=highest_note).contains(&note.midi_note) {
                continue;
            }
            let note_rect = note_rect(
                rect,
                project.sample_rate,
                note.start_frame,
                note.length_frames,
                note.midi_note,
                highest_note,
                pixels_per_second,
            );
            let note_selected = note.source_note_index.is_some_and(|note_index| {
                selected_note == Some((track_index, clip_index, note_index))
            });
            let fill = if note_selected {
                Color32::from_rgb(241, 174, 73)
            } else if selected {
                Color32::from_rgb(104, 137, 241)
            } else {
                Color32::from_rgb(65, 103, 211)
            };
            painter.rect_filled(note_rect, 3.0, fill);
            painter.rect_stroke(
                note_rect,
                3.0,
                Stroke::new(
                    if note_selected || selected { 1.8 } else { 0.8 },
                    Color32::WHITE,
                ),
                StrokeKind::Inside,
            );
            if note.source_note_index.is_some() && note_rect.width() >= 8.0 {
                painter.vline(
                    note_rect.right() - 2.5,
                    note_rect.top() + 2.0..=note_rect.bottom() - 2.0,
                    Stroke::new(1.2, Color32::from_white_alpha(210)),
                );
            }
            if note_rect.width() > 34.0 {
                painter.text(
                    note_rect.left_center() + Vec2::new(5.0, 0.0),
                    Align2::LEFT_CENTER,
                    midi_note_name(note.midi_note),
                    FontId::monospace(10.0),
                    Color32::WHITE,
                );
            }
        }
    }
}

fn paint_playhead(
    painter: &egui::Painter,
    rect: Rect,
    sample_rate: u32,
    playhead_frame: u64,
    pixels_per_second: f32,
) {
    #[allow(clippy::cast_precision_loss)]
    let x =
        rect.left() + KEY_WIDTH + playhead_frame as f32 / sample_rate as f32 * pixels_per_second;
    painter.vline(
        x,
        rect.top()..=rect.bottom(),
        Stroke::new(1.5, Color32::from_rgb(255, 94, 104)),
    );
}

fn note_row_rect(rect: Rect, midi_note: u8, highest_note: u8) -> Rect {
    let row = f32::from(highest_note - midi_note);
    let top = rect.top() + RULER_HEIGHT + row * NOTE_HEIGHT;
    Rect::from_min_max(
        Pos2::new(rect.left(), top),
        Pos2::new(rect.right(), top + NOTE_HEIGHT),
    )
}

fn note_rect(
    rect: Rect,
    sample_rate: u32,
    start_frame: u64,
    length_frames: u64,
    midi_note: u8,
    highest_note: u8,
    pixels_per_second: f32,
) -> Rect {
    let row = note_row_rect(rect, midi_note, highest_note);
    #[allow(clippy::cast_precision_loss)]
    let x = rect.left() + KEY_WIDTH + start_frame as f32 / sample_rate as f32 * pixels_per_second;
    #[allow(clippy::cast_precision_loss)]
    let width = (length_frames as f32 / sample_rate as f32 * pixels_per_second).max(5.0);
    Rect::from_min_size(
        Pos2::new(x + 1.5, row.top() + 2.0),
        Vec2::new(width - 3.0, NOTE_HEIGHT - 4.0),
    )
}

fn frame_at_x(rect: Rect, x: f32, sample_rate: u32, pixels_per_second: f32) -> u64 {
    let seconds = ((x - rect.left() - KEY_WIDTH) / pixels_per_second).max(0.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    {
        (seconds * sample_rate as f32).round() as u64
    }
}

fn midi_note_at_y(rect: Rect, y: f32, lowest_note: u8, highest_note: u8) -> u8 {
    let row = ((y - rect.top() - RULER_HEIGHT) / NOTE_HEIGHT)
        .floor()
        .max(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let row = row as u8;
    highest_note.saturating_sub(row).max(lowest_note)
}

#[allow(clippy::too_many_arguments)]
fn moved_note_start(
    original_start: u64,
    drag_delta_x: f32,
    sample_rate: u32,
    tempo_bpm: f64,
    pixels_per_second: f32,
    snap_enabled: bool,
    grid_divisions_per_beat: u8,
) -> u64 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    let delta_frames = (drag_delta_x / pixels_per_second * sample_rate as f32).round() as i64;
    let raw_start = original_start.saturating_add_signed(delta_frames);
    if !snap_enabled {
        return raw_start;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let grid_frames =
        (f64::from(sample_rate) * 60.0 / tempo_bpm / f64::from(grid_divisions_per_beat.max(1)))
            .round() as u64;
    let grid_frames = grid_frames.max(1);
    raw_start
        .saturating_add(grid_frames / 2)
        .checked_div(grid_frames)
        .unwrap_or(0)
        .saturating_mul(grid_frames)
}

fn resized_note_length(
    start_frame: u64,
    end_frame: u64,
    sample_rate: u32,
    tempo_bpm: f64,
    snap_enabled: bool,
    grid_divisions_per_beat: u8,
) -> u64 {
    let raw_length = end_frame.saturating_sub(start_frame).max(1);
    if !snap_enabled {
        return raw_length;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let grid_frames =
        (f64::from(sample_rate) * 60.0 / tempo_bpm / f64::from(grid_divisions_per_beat.max(1)))
            .round() as u64;
    let grid_frames = grid_frames.max(1);
    let units = raw_length.saturating_add(grid_frames / 2) / grid_frames;
    units.max(1).saturating_mul(grid_frames)
}

fn grid_position(
    rect: Rect,
    pointer: Pos2,
    sample_rate: u32,
    pixels_per_second: f32,
    lowest_note: u8,
    highest_note: u8,
) -> Option<(u64, u8)> {
    if pointer.x < rect.left() + KEY_WIDTH || pointer.y < rect.top() + RULER_HEIGHT {
        return None;
    }
    let seconds = ((pointer.x - rect.left() - KEY_WIDTH) / pixels_per_second).max(0.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    let frame = (seconds * sample_rate as f32).round() as u64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let row = ((pointer.y - rect.top() - RULER_HEIGHT) / NOTE_HEIGHT).floor() as u8;
    let midi_note = highest_note.saturating_sub(row);
    (midi_note >= lowest_note).then_some((frame, midi_note))
}

fn is_black_key(midi_note: u8) -> bool {
    matches!(midi_note % 12, 1 | 3 | 6 | 8 | 10)
}

#[cfg(test)]
mod tests {
    use dmo_core::{Clip, Track, midi_note_frequency};

    use super::*;

    #[test]
    fn visible_range_contains_track_notes_and_has_useful_height() {
        let mut project = Project::new("Piano", 48_000, 120.0).unwrap();
        let mut track = Track::new("Notes");
        for note in [48, 72] {
            track.clips.push(Clip {
                name: midi_note_name(note),
                start_frame: 0,
                length_frames: 1,
                gain: 1.0,
                fade_in_frames: 0,
                fade_out_frames: 0,
                source: ClipSource::Sine {
                    frequency_hz: midi_note_frequency(note),
                    amplitude: 0.8,
                },
            });
        }
        project.tracks.push(track);

        let (low, high) = visible_note_range(&project, 0);
        assert!(low <= 48);
        assert!(high >= 72);
        assert!(high - low + 1 >= MIN_VISIBLE_ROWS);
    }

    #[test]
    fn grid_position_maps_pixels_to_time_and_pitch() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1_000.0, 600.0));
        let pointer = Pos2::new(KEY_WIDTH + 100.0, RULER_HEIGHT + NOTE_HEIGHT * 2.5);

        assert_eq!(
            grid_position(rect, pointer, 48_000, 100.0, 48, 72),
            Some((48_000, 70))
        );
    }

    #[test]
    fn black_key_pattern_matches_pitch_classes() {
        assert!(!is_black_key(60));
        assert!(is_black_key(61));
        assert!(!is_black_key(64));
    }

    #[test]
    fn note_resize_snaps_to_the_active_grid() {
        assert_eq!(resized_note_length(0, 7_000, 48_000, 120.0, true, 4), 6_000);
        assert_eq!(
            resized_note_length(0, 10_000, 48_000, 120.0, true, 4),
            12_000
        );
        assert_eq!(
            resized_note_length(0, 7_000, 48_000, 120.0, false, 4),
            7_000
        );
    }

    #[test]
    fn vertical_position_maps_to_pitch_rows() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1_000.0, 600.0));
        assert_eq!(midi_note_at_y(rect, RULER_HEIGHT + 1.0, 48, 72), 72);
        assert_eq!(
            midi_note_at_y(rect, RULER_HEIGHT + NOTE_HEIGHT * 4.5, 48, 72),
            68
        );
        assert_eq!(midi_note_at_y(rect, 10_000.0, 48, 72), 48);
    }

    #[test]
    fn horizontal_note_drag_snaps_to_the_grid() {
        assert_eq!(
            moved_note_start(24_000, 13.0, 48_000, 120.0, 120.0, true, 4),
            30_000
        );
        assert_eq!(
            moved_note_start(24_000, -100.0, 48_000, 120.0, 120.0, false, 4),
            0
        );
    }
}
