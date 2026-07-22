//! Timeline coordinate conversion and painting.

use std::{collections::HashMap, path::Path};

use dmo_core::{
    AutomationPoint, ClipSource, Project, WaveformOverview, frequency_to_midi_note, midi_note_name,
    rescale_frames_round,
};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

pub const RULER_HEIGHT: f32 = 30.0;
pub const TRACK_HEIGHT: f32 = 76.0;

#[derive(Debug, Clone, Copy)]
pub struct TimelineView {
    pub pixels_per_second: f32,
    pub playhead_frame: u64,
    pub selected_track: Option<usize>,
    pub selected_clip: Option<(usize, usize)>,
    pub draw_mode: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TimelineInteraction {
    pub seek_frame: Option<u64>,
    pub selected_clip: Option<(usize, usize)>,
    pub move_clip: Option<(usize, usize, u64)>,
    pub draw_note: Option<(usize, u64)>,
}

impl TimelineView {
    #[allow(clippy::too_many_lines)]
    pub fn show(
        self,
        ui: &mut egui::Ui,
        project: &Project,
        waveforms: &HashMap<String, WaveformOverview>,
        waveform_errors: &HashMap<String, String>,
    ) -> TimelineInteraction {
        let duration_seconds = frames_to_seconds(
            project
                .duration_frames()
                .max(u64::from(project.sample_rate) * 8),
            project.sample_rate,
        );
        #[allow(clippy::cast_precision_loss)]
        let track_count = project.tracks.len().max(1) as f32;
        #[allow(clippy::cast_possible_truncation)]
        let duration_width = duration_seconds as f32 * self.pixels_per_second + 240.0;
        let desired_size = Vec2::new(
            duration_width.max(ui.available_width()),
            RULER_HEIGHT + track_count * TRACK_HEIGHT,
        );
        let (response, painter) = ui.allocate_painter(desired_size, Sense::click_and_drag());
        let rect = response.rect;
        painter.rect_filled(rect, 0.0, Color32::from_rgb(22, 25, 32));

        paint_rows(&painter, rect, project.tracks.len(), self.selected_track);
        paint_ruler(&painter, rect, project.tempo_bpm, self.pixels_per_second);
        paint_clips(
            &painter,
            rect,
            project,
            self.pixels_per_second,
            self.selected_clip,
            waveforms,
            waveform_errors,
        );
        paint_volume_automation(
            &painter,
            rect,
            project,
            self.pixels_per_second,
            self.selected_track,
        );
        paint_playhead(
            &painter,
            rect,
            project.sample_rate,
            self.playhead_frame,
            self.pixels_per_second,
        );

        let mut interaction = TimelineInteraction::default();
        let mut clip_interacted = false;
        if self.draw_mode && response.hovered() {
            ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::Crosshair);
        }
        for (track_index, track) in project.tracks.iter().enumerate() {
            for (clip_index, clip) in track.clips.iter().enumerate() {
                let clip_rect = clip_rect(
                    rect,
                    project.sample_rate,
                    track_index,
                    clip.start_frame,
                    clip.length_frames,
                    self.pixels_per_second,
                );
                let clip_response = ui.interact(
                    clip_rect,
                    ui.make_persistent_id(("timeline_clip", track_index, clip_index)),
                    if self.draw_mode {
                        Sense::click()
                    } else {
                        Sense::click_and_drag()
                    },
                );
                if clip_response.clicked() {
                    clip_interacted = true;
                    interaction.selected_clip = Some((track_index, clip_index));
                    interaction.seek_frame = clip_response.interact_pointer_pos().map(|pointer| {
                        position_to_frame(
                            rect,
                            pointer,
                            self.pixels_per_second,
                            project.sample_rate,
                        )
                    });
                }
                if !self.draw_mode && clip_response.drag_started() {
                    clip_interacted = true;
                    interaction.selected_clip = Some((track_index, clip_index));
                }
                if !self.draw_mode && clip_response.dragged() {
                    clip_interacted = true;
                    ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::Grabbing);
                }
                if !self.draw_mode
                    && clip_response.drag_stopped()
                    && let Some(delta) = clip_response.total_drag_delta()
                {
                    clip_interacted = true;
                    let delta_seconds = f64::from(delta.x / self.pixels_per_second);
                    #[allow(clippy::cast_precision_loss)]
                    let original_seconds = clip.start_frame as f64 / f64::from(project.sample_rate);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let start_frame = ((original_seconds + delta_seconds).max(0.0)
                        * f64::from(project.sample_rate))
                    .round() as u64;
                    interaction.move_clip = Some((track_index, clip_index, start_frame));
                    interaction.selected_clip = Some((track_index, clip_index));
                }
            }
        }

        if !clip_interacted && let Some(pointer) = response.interact_pointer_pos() {
            if self.draw_mode && response.clicked() {
                if let Some(track_index) = position_to_track(rect, pointer, project.tracks.len()) {
                    interaction.draw_note = Some((
                        track_index,
                        position_to_frame(
                            rect,
                            pointer,
                            self.pixels_per_second,
                            project.sample_rate,
                        ),
                    ));
                }
            } else if !self.draw_mode && (response.clicked() || response.drag_started()) {
                interaction.seek_frame = Some(position_to_frame(
                    rect,
                    pointer,
                    self.pixels_per_second,
                    project.sample_rate,
                ));
            }
        }
        interaction
    }
}

fn paint_volume_automation(
    painter: &egui::Painter,
    rect: Rect,
    project: &Project,
    pixels_per_second: f32,
    selected_track: Option<usize>,
) {
    for (track_index, track) in project.tracks.iter().enumerate() {
        if track.volume_automation.is_empty() {
            continue;
        }
        let mut points: Vec<&AutomationPoint> = track.volume_automation.iter().collect();
        points.sort_by_key(|point| point.frame);
        #[allow(clippy::cast_precision_loss)]
        let row_top = rect.top() + RULER_HEIGHT + track_index as f32 * TRACK_HEIGHT;
        let top = row_top + 25.0;
        let bottom = row_top + TRACK_HEIGHT - 6.0;
        let color = if selected_track == Some(track_index) {
            Color32::from_rgb(255, 196, 92)
        } else {
            Color32::from_rgba_unmultiplied(238, 190, 96, 175)
        };
        let stroke = Stroke::new(
            if selected_track == Some(track_index) {
                1.8
            } else {
                1.2
            },
            color,
        );
        let point_position = |point: &AutomationPoint| {
            #[allow(clippy::cast_precision_loss)]
            let seconds = point.frame as f32 / project.sample_rate as f32;
            let x = rect.left() + seconds * pixels_per_second;
            let normalized = (point.value / 2.0).clamp(0.0, 1.0);
            Pos2::new(x, bottom - normalized * (bottom - top))
        };
        let first = point_position(points[0]);
        painter.line_segment([Pos2::new(rect.left(), first.y), first], stroke);
        for pair in points.windows(2) {
            painter.line_segment([point_position(pair[0]), point_position(pair[1])], stroke);
        }
        let last = point_position(points[points.len() - 1]);
        painter.line_segment([last, Pos2::new(rect.right(), last.y)], stroke);
        for point in points {
            painter.circle_filled(point_position(point), 3.0, color);
        }
    }
}

fn position_to_track(timeline_rect: Rect, pointer: Pos2, track_count: usize) -> Option<usize> {
    let relative_y = pointer.y - timeline_rect.top() - RULER_HEIGHT;
    if relative_y < 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let track_index = (relative_y / TRACK_HEIGHT).floor() as usize;
    (track_index < track_count).then_some(track_index)
}

fn paint_rows(
    painter: &egui::Painter,
    rect: Rect,
    track_count: usize,
    selected_track: Option<usize>,
) {
    for index in 0..track_count.max(1) {
        #[allow(clippy::cast_precision_loss)]
        let top = rect.top() + RULER_HEIGHT + index as f32 * TRACK_HEIGHT;
        let row = Rect::from_min_max(
            Pos2::new(rect.left(), top),
            Pos2::new(rect.right(), top + TRACK_HEIGHT),
        );
        let fill = if selected_track == Some(index) {
            Color32::from_rgb(35, 41, 52)
        } else if index.is_multiple_of(2) {
            Color32::from_rgb(29, 33, 42)
        } else {
            Color32::from_rgb(25, 29, 37)
        };
        painter.rect_filled(row, 0.0, fill);
        painter.hline(
            row.x_range(),
            row.bottom(),
            Stroke::new(1.0, Color32::from_rgb(47, 52, 63)),
        );
    }
}

fn paint_ruler(painter: &egui::Painter, rect: Rect, tempo_bpm: f64, pixels_per_second: f32) {
    let beat_seconds = 60.0 / tempo_bpm;
    let visible_seconds = f64::from(rect.width() / pixels_per_second);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let beat_count = (visible_seconds / beat_seconds).ceil() as u64;
    painter.rect_filled(
        Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + RULER_HEIGHT)),
        0.0,
        Color32::from_rgb(18, 20, 26),
    );

    for beat in 0..=beat_count {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let x = rect.left() + (beat as f64 * beat_seconds) as f32 * pixels_per_second;
        let is_bar = beat.is_multiple_of(4);
        let color = if is_bar {
            Color32::from_rgb(105, 113, 130)
        } else {
            Color32::from_rgb(57, 63, 76)
        };
        painter.vline(
            x,
            rect.top()..=rect.bottom(),
            Stroke::new(if is_bar { 1.2 } else { 0.6 }, color),
        );
        if is_bar {
            painter.text(
                Pos2::new(x + 5.0, rect.top() + 7.0),
                Align2::LEFT_TOP,
                format!("{}", beat / 4 + 1),
                FontId::monospace(12.0),
                Color32::from_rgb(190, 196, 210),
            );
        }
    }
}

#[allow(clippy::too_many_lines)]
fn paint_clips(
    painter: &egui::Painter,
    rect: Rect,
    project: &Project,
    pixels_per_second: f32,
    selected_clip: Option<(usize, usize)>,
    waveforms: &HashMap<String, WaveformOverview>,
    waveform_errors: &HashMap<String, String>,
) {
    for (track_index, track) in project.tracks.iter().enumerate() {
        for (clip_index, clip) in track.clips.iter().enumerate() {
            let clip_rect = clip_rect(
                rect,
                project.sample_rate,
                track_index,
                clip.start_frame,
                clip.length_frames,
                pixels_per_second,
            );
            let selected = selected_clip == Some((track_index, clip_index));
            let base_color = track_color(track_index);
            let color = if selected {
                base_color.gamma_multiply(1.12)
            } else {
                base_color.gamma_multiply(0.92)
            };
            painter.rect_filled(clip_rect, 2.0, color);
            painter.rect_filled(
                Rect::from_min_max(
                    clip_rect.left_top(),
                    Pos2::new(clip_rect.right(), clip_rect.top() + 20.0),
                ),
                2.0,
                base_color.gamma_multiply(if selected { 0.92 } else { 0.72 }),
            );
            painter.rect_stroke(
                clip_rect,
                2.0,
                Stroke::new(
                    if selected { 1.6 } else { 0.8 },
                    if selected {
                        Color32::WHITE
                    } else {
                        color.gamma_multiply(1.35)
                    },
                ),
                StrokeKind::Inside,
            );
            if let ClipSource::Midi { notes, .. } = &clip.source {
                paint_midi_preview(painter, clip_rect, notes, clip.length_frames);
            }
            if let ClipSource::AudioFile {
                path,
                source_offset_frames,
                source_sample_rate,
                ..
            } = &clip.source
            {
                if let Some(overview) = waveforms.get(path) {
                    paint_waveform(
                        painter,
                        clip_rect,
                        overview,
                        *source_offset_frames,
                        rescale_frames_round(
                            clip.length_frames,
                            project.sample_rate,
                            *source_sample_rate,
                        ),
                    );
                } else if waveform_errors.contains_key(path) {
                    painter.rect_filled(
                        clip_rect.shrink(2.0),
                        4.0,
                        Color32::from_rgba_unmultiplied(125, 28, 36, 125),
                    );
                }
            }
            paint_clip_envelope(
                painter,
                clip_rect,
                clip.length_frames,
                clip.fade_in_frames,
                clip.fade_out_frames,
                selected,
            );
            painter.text(
                clip_rect.left_top() + Vec2::new(5.0, 4.0),
                Align2::LEFT_TOP,
                &clip.name,
                FontId::proportional(11.0),
                Color32::WHITE,
            );
            let alternate_count = track
                .take_lanes
                .iter()
                .filter(|take| {
                    take.start_frame < clip.end_frame() && take.end_frame() > clip.start_frame
                })
                .count();
            if alternate_count > 0 && clip_rect.width() > 82.0 {
                let badge = Rect::from_min_size(
                    Pos2::new(clip_rect.right() - 49.0, clip_rect.top() + 3.0),
                    Vec2::new(45.0, 15.0),
                );
                painter.rect_filled(badge, 3.0, Color32::from_rgb(37, 51, 61));
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    format!("{} takes", alternate_count + 1),
                    FontId::monospace(9.0),
                    Color32::from_rgb(143, 220, 190),
                );
            }
            if clip_rect.width() > 90.0 {
                let detail = match &clip.source {
                    ClipSource::Midi { notes, .. } => format!("{} notes", notes.len()),
                    ClipSource::Sine { frequency_hz, .. } => {
                        midi_note_name(frequency_to_midi_note(*frequency_hz))
                    }
                    ClipSource::AudioFile { path, .. } => waveform_errors.get(path).map_or_else(
                        || {
                            Path::new(path)
                                .file_name()
                                .map_or_else(|| "WAV audio".into(), |name| name.to_string_lossy())
                                .into_owned()
                        },
                        |_| "Missing / unreadable WAV".into(),
                    ),
                };
                painter.text(
                    clip_rect.left_bottom() + Vec2::new(5.0, -5.0),
                    Align2::LEFT_BOTTOM,
                    detail,
                    FontId::monospace(11.0),
                    Color32::from_white_alpha(190),
                );
            }
        }
    }
}

fn paint_clip_envelope(
    painter: &egui::Painter,
    clip_rect: Rect,
    length_frames: u64,
    fade_in_frames: u64,
    fade_out_frames: u64,
    selected: bool,
) {
    if length_frames == 0 || clip_rect.width() < 16.0 {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let fade_in_fraction = fade_in_frames.min(length_frames) as f32 / length_frames as f32;
    #[allow(clippy::cast_precision_loss)]
    let fade_out_fraction = fade_out_frames.min(length_frames) as f32 / length_frames as f32;
    let left = clip_rect.left() + 2.0;
    let right = clip_rect.right() - 2.0;
    let top = clip_rect.top() + 23.0;
    let bottom = clip_rect.bottom() - 3.0;
    let fade_in_x = left + fade_in_fraction * (right - left);
    let fade_out_x = right - fade_out_fraction * (right - left);
    let color = if selected {
        Color32::from_rgba_unmultiplied(255, 222, 145, 215)
    } else {
        Color32::from_rgba_unmultiplied(245, 248, 255, 125)
    };
    let stroke = Stroke::new(if selected { 1.5 } else { 1.0 }, color);
    if fade_in_frames.saturating_add(fade_out_frames) <= length_frames {
        painter.line_segment([Pos2::new(left, bottom), Pos2::new(fade_in_x, top)], stroke);
        painter.line_segment(
            [Pos2::new(fade_in_x, top), Pos2::new(fade_out_x, top)],
            stroke,
        );
        painter.line_segment(
            [Pos2::new(fade_out_x, top), Pos2::new(right, bottom)],
            stroke,
        );
        if selected {
            painter.circle_filled(Pos2::new(fade_in_x, top), 2.5, color);
            painter.circle_filled(Pos2::new(fade_out_x, top), 2.5, color);
        }
    } else {
        let fade_sum = fade_in_frames.saturating_add(fade_out_frames);
        #[allow(clippy::cast_precision_loss)]
        let peak_fraction = fade_in_frames as f32 / fade_sum as f32;
        #[allow(clippy::cast_precision_loss)]
        let peak_gain = length_frames as f32 / fade_sum as f32;
        let peak = Pos2::new(
            left + peak_fraction * (right - left),
            bottom - peak_gain.clamp(0.0, 1.0) * (bottom - top),
        );
        painter.line_segment([Pos2::new(left, bottom), peak], stroke);
        painter.line_segment([peak, Pos2::new(right, bottom)], stroke);
        if selected {
            painter.circle_filled(peak, 2.5, color);
        }
    }
}

fn paint_midi_preview(
    painter: &egui::Painter,
    clip_rect: Rect,
    notes: &[dmo_core::MidiNote],
    clip_length_frames: u64,
) {
    if notes.is_empty() || clip_length_frames == 0 || clip_rect.width() < 12.0 {
        return;
    }
    let content = Rect::from_min_max(
        clip_rect.left_top() + Vec2::new(4.0, 24.0),
        clip_rect.right_bottom() - Vec2::new(4.0, 5.0),
    );
    if content.width() <= 2.0 || content.height() <= 2.0 {
        return;
    }
    let lowest = notes.iter().map(|note| note.midi_note).min().unwrap_or(60);
    let highest = notes
        .iter()
        .map(|note| note.midi_note)
        .max()
        .unwrap_or(lowest);
    let pitch_span = f32::from(highest.saturating_sub(lowest).max(1));

    for note in notes {
        #[allow(clippy::cast_precision_loss)]
        let x_fraction = note.start_frame as f32 / clip_length_frames as f32;
        #[allow(clippy::cast_precision_loss)]
        let width_fraction = note.length_frames as f32 / clip_length_frames as f32;
        let left = content.left() + x_fraction * content.width();
        let width = (width_fraction * content.width()).max(2.0);
        let pitch_fraction = f32::from(note.midi_note.saturating_sub(lowest)) / pitch_span;
        let center_y = content.bottom() - pitch_fraction * content.height();
        let note_rect = Rect::from_min_size(
            Pos2::new(left, center_y - 1.5),
            Vec2::new(width.min(content.right() - left), 3.0),
        );
        painter.rect_filled(note_rect, 0.8, Color32::from_white_alpha(205));
    }
}

fn paint_waveform(
    painter: &egui::Painter,
    clip_rect: Rect,
    overview: &WaveformOverview,
    source_offset_frames: u64,
    source_length_frames: u64,
) {
    if clip_rect.width() < 12.0
        || overview.peaks.is_empty()
        || overview.source_frames == 0
        || source_length_frames == 0
    {
        return;
    }

    let wave_rect = Rect::from_min_max(
        Pos2::new(clip_rect.left() + 3.0, clip_rect.top() + 23.0),
        Pos2::new(clip_rect.right() - 3.0, clip_rect.bottom() - 4.0),
    );
    if wave_rect.height() <= 4.0 {
        return;
    }

    let source_end = source_offset_frames.saturating_add(source_length_frames);
    let peak_count = overview.peaks.len() as u64;
    let first_peak = source_offset_frames
        .saturating_mul(peak_count)
        .checked_div(overview.source_frames)
        .unwrap_or(0)
        .min(peak_count.saturating_sub(1));
    let last_peak = source_end
        .saturating_mul(peak_count)
        .saturating_add(overview.source_frames.saturating_sub(1))
        .checked_div(overview.source_frames)
        .unwrap_or(peak_count)
        .min(peak_count);
    let clipped = painter.with_clip_rect(wave_rect);
    let lane_height = wave_rect.height() * 0.5;
    let color = Color32::from_rgba_unmultiplied(235, 242, 255, 175);

    for peak_index in first_peak..last_peak.max(first_peak.saturating_add(1)) {
        let Some(peak) = usize::try_from(peak_index)
            .ok()
            .and_then(|index| overview.peaks.get(index))
        else {
            break;
        };
        let peak_frame = peak_index
            .saturating_mul(overview.source_frames)
            .checked_div(peak_count)
            .unwrap_or(0);
        let relative = peak_frame.saturating_sub(source_offset_frames);
        #[allow(clippy::cast_precision_loss)]
        let x =
            wave_rect.left() + (relative as f32 / source_length_frames as f32) * wave_rect.width();

        let upper_center = wave_rect.top() + lane_height * 0.5;
        let lower_center = wave_rect.top() + lane_height * 1.5;
        let amplitude_height = lane_height * 0.45;
        clipped.line_segment(
            [
                Pos2::new(x, upper_center - peak.left_max * amplitude_height),
                Pos2::new(x, upper_center - peak.left_min * amplitude_height),
            ],
            Stroke::new(1.0, color),
        );
        clipped.line_segment(
            [
                Pos2::new(x, lower_center - peak.right_max * amplitude_height),
                Pos2::new(x, lower_center - peak.right_min * amplitude_height),
            ],
            Stroke::new(1.0, color),
        );
    }
}

fn paint_playhead(
    painter: &egui::Painter,
    rect: Rect,
    sample_rate: u32,
    playhead_frame: u64,
    pixels_per_second: f32,
) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let x =
        rect.left() + (playhead_frame as f64 / f64::from(sample_rate)) as f32 * pixels_per_second;
    let color = Color32::from_rgb(255, 94, 104);
    painter.vline(x, rect.top()..=rect.bottom(), Stroke::new(2.0, color));
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(x - 6.0, rect.top()),
            Pos2::new(x + 6.0, rect.top()),
            Pos2::new(x, rect.top() + 8.0),
        ],
        color,
        Stroke::NONE,
    ));
}

fn clip_rect(
    timeline_rect: Rect,
    sample_rate: u32,
    track_index: usize,
    start_frame: u64,
    length_frames: u64,
    pixels_per_second: f32,
) -> Rect {
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let start_x = timeline_rect.left()
        + (start_frame as f64 / f64::from(sample_rate)) as f32 * pixels_per_second;
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let width =
        ((length_frames as f64 / f64::from(sample_rate)) as f32 * pixels_per_second).max(8.0);
    #[allow(clippy::cast_precision_loss)]
    let top = timeline_rect.top() + RULER_HEIGHT + track_index as f32 * TRACK_HEIGHT + 3.0;
    Rect::from_min_size(
        Pos2::new(start_x + 1.0, top),
        Vec2::new((width - 2.0).max(4.0), TRACK_HEIGHT - 6.0),
    )
}

fn position_to_frame(
    timeline_rect: Rect,
    pointer: Pos2,
    pixels_per_second: f32,
    sample_rate: u32,
) -> u64 {
    let seconds = f64::from(((pointer.x - timeline_rect.left()) / pixels_per_second).max(0.0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frame = (seconds * f64::from(sample_rate)).round() as u64;
    frame
}

fn frames_to_seconds(frames: u64, sample_rate: u32) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let seconds = frames as f64 / f64::from(sample_rate);
    seconds
}

pub fn track_color(track_index: usize) -> Color32 {
    let palette = [
        Color32::from_rgb(73, 111, 214),
        Color32::from_rgb(126, 88, 204),
        Color32::from_rgb(35, 155, 139),
        Color32::from_rgb(207, 119, 52),
        Color32::from_rgb(196, 74, 123),
    ];
    palette[track_index % palette.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_and_pixel_conversion_uses_project_sample_rate() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1_000.0, 400.0));
        assert_eq!(
            position_to_frame(rect, Pos2::new(250.0, 0.0), 100.0, 48_000),
            120_000
        );
    }

    #[test]
    fn pointer_y_maps_to_arrangement_tracks_below_the_ruler() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1_000.0, 400.0));
        assert_eq!(
            position_to_track(rect, Pos2::new(100.0, RULER_HEIGHT + 4.0), 2),
            Some(0)
        );
        assert_eq!(
            position_to_track(rect, Pos2::new(100.0, RULER_HEIGHT + TRACK_HEIGHT + 4.0), 2,),
            Some(1)
        );
        assert_eq!(position_to_track(rect, Pos2::new(100.0, 4.0), 2), None);
    }
}
