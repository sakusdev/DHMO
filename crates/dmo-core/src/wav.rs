use std::{fmt, fs::File, io, io::Write, path::Path};

#[derive(Debug)]
pub enum WavError {
    Io(io::Error),
    TooManySamples(usize),
    OddSampleCount(usize),
}

impl fmt::Display for WavError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::TooManySamples(count) => write!(formatter, "too many WAV samples: {count}"),
            Self::OddSampleCount(count) => {
                write!(formatter, "stereo sample count must be even, got {count}")
            }
        }
    }
}

impl std::error::Error for WavError {}

impl From<io::Error> for WavError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Writes interleaved stereo floating-point samples as a PCM 16-bit WAV file.
///
/// # Errors
///
/// Returns [`WavError`] when the sample count is not stereo-aligned, the data
/// exceeds the WAV size limit, or the destination cannot be written.
pub fn write_stereo_i16_wav(
    path: impl AsRef<Path>,
    sample_rate: u32,
    samples: &[f32],
) -> Result<(), WavError> {
    if !samples.len().is_multiple_of(2) {
        return Err(WavError::OddSampleCount(samples.len()));
    }

    let data_size = u32::try_from(samples.len().saturating_mul(2))
        .map_err(|_| WavError::TooManySamples(samples.len()))?;
    let riff_size = 36_u32
        .checked_add(data_size)
        .ok_or(WavError::TooManySamples(samples.len()))?;
    let byte_rate = sample_rate.saturating_mul(4);

    let mut file = File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&4_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_size.to_le_bytes())?;

    for sample in samples {
        #[allow(clippy::cast_possible_truncation)]
        let pcm = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        file.write_all(&pcm.to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn writes_a_valid_wav_header() {
        let path = std::env::temp_dir().join(format!("dmo-wav-test-{}.wav", std::process::id()));
        write_stereo_i16_wav(&path, 48_000, &[0.0, 0.0, 1.0, -1.0]).unwrap();
        let bytes = fs::read(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(bytes.len(), 52);
    }
}
