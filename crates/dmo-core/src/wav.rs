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

/// Writes interleaved stereo samples as 24-bit PCM WAV.
///
/// # Errors
///
/// Returns [`WavError`] when the sample count is not stereo-aligned, the data
/// exceeds the WAV size limit, or the destination cannot be written.
pub fn write_stereo_i24_wav(
    path: impl AsRef<Path>,
    sample_rate: u32,
    samples: &[f32],
) -> Result<(), WavError> {
    if !samples.len().is_multiple_of(2) {
        return Err(WavError::OddSampleCount(samples.len()));
    }
    let data_size = u32::try_from(samples.len().saturating_mul(3))
        .map_err(|_| WavError::TooManySamples(samples.len()))?;
    let mut file = File::create(path)?;
    write_wav_header(&mut file, sample_rate, data_size, 1, 24, 6)?;
    for sample in samples {
        #[allow(clippy::cast_possible_truncation)]
        let pcm = (sample.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
        let bytes = pcm.to_le_bytes();
        file.write_all(&bytes[..3])?;
    }
    Ok(())
}

/// Writes interleaved stereo samples as 32-bit IEEE-float WAV.
///
/// # Errors
///
/// Returns [`WavError`] when the sample count is not stereo-aligned, the data
/// exceeds the WAV size limit, or the destination cannot be written.
pub fn write_stereo_f32_wav(
    path: impl AsRef<Path>,
    sample_rate: u32,
    samples: &[f32],
) -> Result<(), WavError> {
    if !samples.len().is_multiple_of(2) {
        return Err(WavError::OddSampleCount(samples.len()));
    }
    let data_size = u32::try_from(samples.len().saturating_mul(4))
        .map_err(|_| WavError::TooManySamples(samples.len()))?;
    let mut file = File::create(path)?;
    write_wav_header(&mut file, sample_rate, data_size, 3, 32, 8)?;
    for sample in samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

fn write_wav_header(
    file: &mut File,
    sample_rate: u32,
    data_size: u32,
    format: u16,
    bits_per_sample: u16,
    block_align: u16,
) -> Result<(), WavError> {
    let riff_size = 36_u32
        .checked_add(data_size)
        .ok_or(WavError::TooManySamples(
            usize::try_from(data_size).unwrap_or(usize::MAX),
        ))?;
    let byte_rate = sample_rate.saturating_mul(u32::from(block_align));
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&format.to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&bits_per_sample.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_size.to_le_bytes())?;
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

    #[test]
    fn writes_24_bit_and_float_wav_headers() {
        let base = std::env::temp_dir().join(format!("dmo-wav-format-test-{}", std::process::id()));
        let pcm_path = base.with_extension("pcm.wav");
        let float_path = base.with_extension("float.wav");
        write_stereo_i24_wav(&pcm_path, 48_000, &[0.0, 0.0]).unwrap();
        write_stereo_f32_wav(&float_path, 48_000, &[0.0, 0.0]).unwrap();
        let pcm = fs::read(&pcm_path).unwrap();
        let float = fs::read(&float_path).unwrap();
        let _ = fs::remove_file(pcm_path);
        let _ = fs::remove_file(float_path);
        assert_eq!(&pcm[20..22], &1_u16.to_le_bytes());
        assert_eq!(&pcm[34..36], &24_u16.to_le_bytes());
        assert_eq!(&float[20..22], &3_u16.to_le_bytes());
        assert_eq!(&float[34..36], &32_u16.to_le_bytes());
    }
}
