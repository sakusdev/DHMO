use std::fmt;

/// The current playback mode of a [`Transport`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TransportState {
    /// Playback is stopped. A stop command also returns the playhead to the
    /// timeline origin, while seeking can reposition it afterward.
    #[default]
    Stopped,
    /// The playhead advances when [`Transport::advance`] is called.
    Playing,
    /// Playback is suspended while retaining the current playhead position.
    Paused,
}

/// A half-open frame range used for looped playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopRange {
    start_frame: u64,
    end_frame: u64,
}

impl LoopRange {
    /// Creates a loop range covering `start_frame..end_frame`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::InvalidLoopRange`] when the end does not come
    /// after the start.
    pub const fn new(start_frame: u64, end_frame: u64) -> Result<Self, TransportError> {
        if start_frame >= end_frame {
            return Err(TransportError::InvalidLoopRange {
                start_frame,
                end_frame,
            });
        }

        Ok(Self {
            start_frame,
            end_frame,
        })
    }

    /// Returns the first frame included in the loop.
    #[must_use]
    pub const fn start_frame(self) -> u64 {
        self.start_frame
    }

    /// Returns the first frame after the loop.
    #[must_use]
    pub const fn end_frame(self) -> u64 {
        self.end_frame
    }

    /// Returns the loop length in frames.
    #[must_use]
    pub const fn length_frames(self) -> u64 {
        self.end_frame - self.start_frame
    }
}

/// Errors produced while configuring a [`Transport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// A loop must contain at least one frame.
    InvalidLoopRange { start_frame: u64, end_frame: u64 },
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLoopRange {
                start_frame,
                end_frame,
            } => write!(
                formatter,
                "invalid loop range: end frame {end_frame} must be after start frame {start_frame}"
            ),
        }
    }
}

impl std::error::Error for TransportError {}

/// Allocation-free playback state for use by an audio callback.
///
/// The transport measures positions in audio frames rather than interleaved
/// samples. It owns no audio data and performs no I/O or synchronization.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Transport {
    state: TransportState,
    position_frames: u64,
    loop_range: Option<LoopRange>,
}

impl Transport {
    /// Creates a stopped transport at frame zero with looping disabled.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: TransportState::Stopped,
            position_frames: 0,
            loop_range: None,
        }
    }

    /// Returns the current playback mode.
    #[must_use]
    pub const fn state(&self) -> TransportState {
        self.state
    }

    /// Returns the current playhead position in audio frames.
    #[must_use]
    pub const fn position_frames(&self) -> u64 {
        self.position_frames
    }

    /// Returns the active loop range, if looping is enabled.
    #[must_use]
    pub const fn loop_range(&self) -> Option<LoopRange> {
        self.loop_range
    }

    /// Returns whether the playhead advances when frames are processed.
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        matches!(self.state, TransportState::Playing)
    }

    /// Starts or resumes playback from the current playhead position.
    pub const fn play(&mut self) {
        self.state = TransportState::Playing;
    }

    /// Pauses active playback without moving the playhead.
    ///
    /// Calling this while stopped leaves the transport stopped.
    pub const fn pause(&mut self) {
        if matches!(self.state, TransportState::Playing) {
            self.state = TransportState::Paused;
        }
    }

    /// Stops playback and returns the playhead to frame zero.
    pub const fn stop(&mut self) {
        self.state = TransportState::Stopped;
        self.position_frames = 0;
    }

    /// Moves the playhead to an exact timeline frame without changing state.
    pub const fn seek(&mut self, frame: u64) {
        self.position_frames = frame;
    }

    /// Enables looping over the half-open range `start_frame..end_frame`.
    ///
    /// The playhead is not moved until playback advances. If it is already at
    /// or beyond the loop end, the next advance folds it into the loop.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::InvalidLoopRange`] when the range is empty or
    /// reversed. An existing valid loop range is retained on error.
    pub fn set_loop_range(
        &mut self,
        start_frame: u64,
        end_frame: u64,
    ) -> Result<(), TransportError> {
        self.loop_range = Some(LoopRange::new(start_frame, end_frame)?);
        Ok(())
    }

    /// Disables looping without changing playback state or position.
    pub const fn clear_loop_range(&mut self) {
        self.loop_range = None;
    }

    /// Advances the playhead by a number of audio frames and returns its new
    /// position.
    ///
    /// Stopped and paused transports do not advance. With looping disabled,
    /// arithmetic saturates at [`u64::MAX`]. With looping enabled, crossing the
    /// exclusive loop end wraps sample-accurately to its start, including when
    /// a single callback spans the loop more than once.
    #[must_use]
    pub fn advance(&mut self, frame_count: u64) -> u64 {
        if !self.is_playing() || frame_count == 0 {
            return self.position_frames;
        }

        self.position_frames = match self.loop_range {
            Some(loop_range) => advance_looped(self.position_frames, frame_count, loop_range),
            None => self.position_frames.saturating_add(frame_count),
        };
        self.position_frames
    }
}

fn advance_looped(position: u64, frame_count: u64, loop_range: LoopRange) -> u64 {
    let start = loop_range.start_frame();
    let end = loop_range.end_frame();
    let length = loop_range.length_frames();

    if position < end {
        let frames_to_end = end - position;
        if frame_count < frames_to_end {
            return position + frame_count;
        }

        return start + (frame_count - frames_to_end) % length;
    }

    let offset = (position - start) % length;
    start + add_modulo(offset, frame_count % length, length)
}

fn add_modulo(left: u64, right: u64, modulus: u64) -> u64 {
    let distance_to_wrap = modulus - left;
    if right >= distance_to_wrap {
        right - distance_to_wrap
    } else {
        left + right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_starts_stopped_at_the_origin() {
        let mut transport = Transport::new();

        assert_eq!(transport.state(), TransportState::Stopped);
        assert_eq!(transport.position_frames(), 0);
        assert_eq!(transport.loop_range(), None);
        assert!(!transport.is_playing());
        assert_eq!(transport.advance(512), 0);
        transport.pause();
        assert_eq!(transport.state(), TransportState::Stopped);
    }

    #[test]
    fn play_pause_resume_and_stop_have_stable_positions() {
        let mut transport = Transport::new();

        transport.play();
        assert_eq!(transport.advance(256), 256);
        transport.pause();
        assert_eq!(transport.advance(512), 256);

        transport.play();
        assert_eq!(transport.advance(128), 384);
        transport.stop();
        assert_eq!(transport.state(), TransportState::Stopped);
        assert_eq!(transport.position_frames(), 0);
    }

    #[test]
    fn seeking_does_not_change_playback_state() {
        let mut transport = Transport::new();

        transport.seek(42_000);
        assert_eq!(transport.position_frames(), 42_000);
        assert_eq!(transport.state(), TransportState::Stopped);

        transport.play();
        transport.seek(123);
        assert!(transport.is_playing());
        assert_eq!(transport.advance(1), 124);
    }

    #[test]
    fn loop_range_must_contain_at_least_one_frame() {
        let mut transport = Transport::new();
        transport.set_loop_range(10, 20).unwrap();

        assert_eq!(
            transport.set_loop_range(30, 30),
            Err(TransportError::InvalidLoopRange {
                start_frame: 30,
                end_frame: 30,
            })
        );
        assert_eq!(transport.loop_range(), LoopRange::new(10, 20).ok());

        transport.clear_loop_range();
        assert_eq!(transport.loop_range(), None);
    }

    #[test]
    fn playback_wraps_at_the_exclusive_loop_end() {
        let mut transport = Transport::new();
        transport.set_loop_range(100, 200).unwrap();
        transport.seek(190);
        transport.play();

        assert_eq!(transport.advance(9), 199);
        assert_eq!(transport.advance(1), 100);
        assert_eq!(transport.advance(250), 150);
    }

    #[test]
    fn playback_before_a_loop_reaches_it_without_skipping_frames() {
        let mut transport = Transport::new();
        transport.set_loop_range(100, 200).unwrap();
        transport.seek(50);
        transport.play();

        assert_eq!(transport.advance(75), 125);
        assert_eq!(transport.advance(75), 100);
    }

    #[test]
    fn playback_beyond_a_loop_is_folded_into_the_range() {
        let mut transport = Transport::new();
        transport.set_loop_range(100, 200).unwrap();
        transport.seek(350);
        transport.play();

        assert_eq!(transport.advance(25), 175);
    }

    #[test]
    fn advancing_without_a_loop_saturates() {
        let mut transport = Transport::new();
        transport.seek(u64::MAX - 3);
        transport.play();

        assert_eq!(transport.advance(10), u64::MAX);
    }

    #[test]
    fn very_large_loop_ranges_do_not_overflow() {
        let mut transport = Transport::new();
        transport.set_loop_range(1, u64::MAX).unwrap();
        transport.seek(u64::MAX - 2);
        transport.play();

        assert_eq!(transport.advance(u64::MAX - 2), u64::MAX - 3);
    }
}
