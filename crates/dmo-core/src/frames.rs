/// Converts a frame count between sample-rate domains using nearest-integer
/// rounding and saturating arithmetic.
///
/// A zero source or target rate produces zero. Projects and imported audio
/// normally reject zero rates before reaching this helper.
#[must_use]
pub fn rescale_frames_round(frames: u64, from_rate: u32, to_rate: u32) -> u64 {
    if frames == 0 || from_rate == 0 || to_rate == 0 {
        return 0;
    }

    let numerator = u128::from(frames)
        .saturating_mul(u128::from(to_rate))
        .saturating_add(u128::from(from_rate) / 2);
    let scaled = numerator / u128::from(from_rate);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescales_between_common_audio_rates() {
        assert_eq!(rescale_frames_round(48_000, 48_000, 44_100), 44_100);
        assert_eq!(rescale_frames_round(1_000, 48_000, 44_100), 919);
        assert_eq!(rescale_frames_round(1, 2, 1), 1);
    }

    #[test]
    fn rescaling_saturates_and_handles_zero_rates() {
        assert_eq!(rescale_frames_round(u64::MAX, 1, u32::MAX), u64::MAX);
        assert_eq!(rescale_frames_round(100, 0, 48_000), 0);
        assert_eq!(rescale_frames_round(100, 48_000, 0), 0);
    }
}
