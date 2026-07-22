//! WAV-file inspection, decoding, resampling, and waveform summaries.
//!
//! All file access and sample conversion happens here, before a rendered
//! buffer is handed to the real-time audio callback.

use std::{fmt, path::Path};

use crate::ClipSource;

/// Default number of min/max pairs generated for an imported waveform.
pub const DEFAULT_WAVEFORM_PEAKS: usize = 1_024;

/// The encoding used by samples in a WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSampleFormat {
    Integer,
    Float,
}

/// Metadata available without decoding the WAV sample payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFileInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub bits_per_sample: u16,
    pub sample_format: AudioSampleFormat,
}

/// A WAV decoded to interleaved stereo floating-point samples.
///
/// `info.channels` retains the original file channel count. `samples` always
/// contains two values per frame, even for a mono or multichannel source.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudio {
    pub info: AudioFileInfo,
    pub samples: Vec<f32>,
}

impl DecodedAudio {
    #[must_use]
    pub fn frame_count(&self) -> u64 {
        u64::try_from(self.samples.len() / 2).unwrap_or(u64::MAX)
    }

    /// Linearly resamples the decoded stereo data to `target_sample_rate`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioFileError`] for a zero rate or an output too large for
    /// the current platform.
    pub fn resample_stereo(&self, target_sample_rate: u32) -> Result<Vec<f32>, AudioFileError> {
        let output_frames = convert_frame_count(
            self.frame_count(),
            self.info.sample_rate,
            target_sample_rate,
        )?;
        let output_capacity = stereo_sample_capacity(output_frames)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(output_capacity)
            .map_err(|_| AudioFileError::TooManyFrames(output_frames))?;

        for output_frame in 0..output_frames {
            let [left, right] = self
                .sample_at_rate(0, output_frame, target_sample_rate)
                .unwrap_or([0.0, 0.0]);
            output.push(left);
            output.push(right);
        }
        Ok(output)
    }

    /// Builds a bounded min/max representation suitable for drawing a
    /// waveform. At most `max_peaks` entries are returned.
    #[must_use]
    pub fn overview(&self, max_peaks: usize) -> WaveformOverview {
        let frame_count = self.samples.len() / 2;
        let peak_count = frame_count.min(max_peaks);
        let mut peaks = Vec::with_capacity(peak_count);

        for peak_index in 0..peak_count {
            let start = proportional_index(peak_index, frame_count, peak_count);
            let end = proportional_index(peak_index + 1, frame_count, peak_count);
            let first = self.stereo_frame_usize(start);
            let mut peak = WaveformPeak {
                left_min: first[0],
                left_max: first[0],
                right_min: first[1],
                right_max: first[1],
            };
            for frame in (start + 1)..end {
                let [left, right] = self.stereo_frame_usize(frame);
                peak.left_min = peak.left_min.min(left);
                peak.left_max = peak.left_max.max(left);
                peak.right_min = peak.right_min.min(right);
                peak.right_max = peak.right_max.max(right);
            }
            peaks.push(peak);
        }

        WaveformOverview {
            sample_rate: self.info.sample_rate,
            source_frames: self.frame_count(),
            peaks,
        }
    }

    /// Samples this audio at an output-rate frame using linear interpolation.
    /// `source_offset_frames` is expressed at the source file's native rate.
    #[must_use]
    pub fn sample_at_rate(
        &self,
        source_offset_frames: u64,
        output_frame: u64,
        output_sample_rate: u32,
    ) -> Option<[f32; 2]> {
        if output_sample_rate == 0 || self.info.sample_rate == 0 {
            return None;
        }

        let numerator = u128::from(output_frame).saturating_mul(u128::from(self.info.sample_rate));
        let denominator = u128::from(output_sample_rate);
        let relative_frame = numerator / denominator;
        let remainder = numerator % denominator;
        let source_frame = u128::from(source_offset_frames).checked_add(relative_frame)?;
        let source_frame = u64::try_from(source_frame).ok()?;
        let first = self.stereo_frame(source_frame)?;
        let second = self
            .stereo_frame(source_frame.saturating_add(1))
            .unwrap_or(first);
        #[allow(clippy::cast_precision_loss)]
        let fraction = remainder as f32 / output_sample_rate as f32;

        Some([
            first[0] + (second[0] - first[0]) * fraction,
            first[1] + (second[1] - first[1]) * fraction,
        ])
    }

    fn stereo_frame(&self, frame: u64) -> Option<[f32; 2]> {
        let frame = usize::try_from(frame).ok()?;
        let index = frame.checked_mul(2)?;
        Some([*self.samples.get(index)?, *self.samples.get(index + 1)?])
    }

    fn stereo_frame_usize(&self, frame: usize) -> [f32; 2] {
        let index = frame * 2;
        [self.samples[index], self.samples[index + 1]]
    }
}

/// The extrema for one horizontal segment of a stereo waveform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveformPeak {
    pub left_min: f32,
    pub left_max: f32,
    pub right_min: f32,
    pub right_max: f32,
}

/// A compact waveform representation derived from decoded source frames.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformOverview {
    pub sample_rate: u32,
    pub source_frames: u64,
    pub peaks: Vec<WaveformPeak>,
}

/// Data needed to add a decoded WAV to a project and paint it immediately.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedAudioFile {
    pub source: ClipSource,
    pub info: AudioFileInfo,
    pub overview: WaveformOverview,
}

impl ImportedAudioFile {
    /// Returns the source duration expressed in project timeline frames.
    ///
    /// # Errors
    ///
    /// Returns [`AudioFileError`] if either sample rate is zero or the result
    /// exceeds a `u64` frame count.
    pub fn project_length_frames(&self, project_sample_rate: u32) -> Result<u64, AudioFileError> {
        convert_frame_count(self.info.frames, self.info.sample_rate, project_sample_rate)
    }
}

/// An error produced while reading or converting an audio file.
#[derive(Debug)]
pub enum AudioFileError {
    Wav(hound::Error),
    InvalidFormat(String),
    InvalidSample { index: u64 },
    NonUnicodePath,
    InvalidSampleRate(u32),
    TooManyFrames(u64),
}

impl fmt::Display for AudioFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wav(error) => write!(formatter, "WAV decoding failed: {error}"),
            Self::InvalidFormat(message) => write!(formatter, "invalid WAV format: {message}"),
            Self::InvalidSample { index } => {
                write!(formatter, "WAV sample {index} is not finite")
            }
            Self::NonUnicodePath => write!(formatter, "audio path is not valid Unicode"),
            Self::InvalidSampleRate(rate) => write!(formatter, "invalid sample rate: {rate}"),
            Self::TooManyFrames(frames) => {
                write!(formatter, "audio contains too many frames: {frames}")
            }
        }
    }
}

impl std::error::Error for AudioFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wav(error) => Some(error),
            Self::InvalidFormat(_)
            | Self::InvalidSample { .. }
            | Self::NonUnicodePath
            | Self::InvalidSampleRate(_)
            | Self::TooManyFrames(_) => None,
        }
    }
}

impl From<hound::Error> for AudioFileError {
    fn from(error: hound::Error) -> Self {
        Self::Wav(error)
    }
}

/// Reads WAV metadata without decoding the sample payload.
///
/// # Errors
///
/// Returns [`AudioFileError`] if the file cannot be opened or its encoding is
/// unsupported or malformed.
pub fn probe_wav(path: impl AsRef<Path>) -> Result<AudioFileInfo, AudioFileError> {
    let reader = hound::WavReader::open(path)?;
    audio_file_info(reader.spec(), u64::from(reader.duration()))
}

/// Decodes integer PCM or 32-bit floating-point WAV data to interleaved f32
/// stereo samples. Mono files are duplicated; files with more than two
/// channels use their first two channels.
///
/// # Errors
///
/// Returns [`AudioFileError`] for file I/O, malformed data, unsupported sample
/// encodings, non-finite float samples, or impractically large data.
pub fn decode_wav(path: impl AsRef<Path>) -> Result<DecodedAudio, AudioFileError> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let info = audio_file_info(spec, u64::from(reader.duration()))?;
    let estimated_samples = stereo_sample_capacity(info.frames)?;
    let channels = usize::from(info.channels);
    let samples = match info.sample_format {
        AudioSampleFormat::Integer => {
            let scale = integer_scale(info.bits_per_sample)?;
            decode_to_stereo(
                reader.samples::<i32>(),
                channels,
                estimated_samples,
                |sample, _| {
                    #[allow(clippy::cast_precision_loss)]
                    let normalized = sample as f32 / scale;
                    Ok(normalized.clamp(-1.0, 1.0))
                },
            )?
        }
        AudioSampleFormat::Float => decode_to_stereo(
            reader.samples::<f32>(),
            channels,
            estimated_samples,
            |sample, index| {
                if sample.is_finite() {
                    Ok(sample.clamp(-1.0, 1.0))
                } else {
                    Err(AudioFileError::InvalidSample { index })
                }
            },
        )?,
    };

    Ok(DecodedAudio { info, samples })
}

/// Decodes a WAV and immediately reduces it to a waveform overview.
///
/// # Errors
///
/// Returns the same errors as [`decode_wav`].
pub fn decode_wav_overview(
    path: impl AsRef<Path>,
    max_peaks: usize,
) -> Result<WaveformOverview, AudioFileError> {
    Ok(decode_wav(path)?.overview(max_peaks))
}

/// Probes and decodes a WAV into project-source metadata and a default-sized
/// waveform overview.
///
/// # Errors
///
/// Returns the same errors as [`decode_wav`], plus an error when the path
/// cannot be represented by [`ClipSource::AudioFile`].
pub fn import_wav(path: impl AsRef<Path>) -> Result<ImportedAudioFile, AudioFileError> {
    import_wav_with_overview(path, DEFAULT_WAVEFORM_PEAKS)
}

/// Imports a WAV with an explicit maximum number of waveform peaks.
///
/// # Errors
///
/// Returns the same errors as [`import_wav`].
pub fn import_wav_with_overview(
    path: impl AsRef<Path>,
    max_peaks: usize,
) -> Result<ImportedAudioFile, AudioFileError> {
    let path = path.as_ref();
    let stored_path = path
        .to_str()
        .ok_or(AudioFileError::NonUnicodePath)?
        .to_owned();
    let decoded = decode_wav(path)?;
    let info = decoded.info;
    let overview = decoded.overview(max_peaks);
    Ok(ImportedAudioFile {
        source: ClipSource::AudioFile {
            path: stored_path,
            source_offset_frames: 0,
            source_sample_rate: info.sample_rate,
            channels: info.channels,
        },
        info,
        overview,
    })
}

/// Converts a frame count between sample-rate domains using nearest-integer
/// rounding and overflow-checked integer arithmetic.
///
/// # Errors
///
/// Returns [`AudioFileError`] for a zero rate or a result beyond `u64`.
pub fn convert_frame_count(
    frames: u64,
    source_sample_rate: u32,
    target_sample_rate: u32,
) -> Result<u64, AudioFileError> {
    if source_sample_rate == 0 {
        return Err(AudioFileError::InvalidSampleRate(source_sample_rate));
    }
    if target_sample_rate == 0 {
        return Err(AudioFileError::InvalidSampleRate(target_sample_rate));
    }

    let numerator = u128::from(frames)
        .saturating_mul(u128::from(target_sample_rate))
        .saturating_add(u128::from(source_sample_rate / 2));
    let converted = numerator / u128::from(source_sample_rate);
    u64::try_from(converted).map_err(|_| AudioFileError::TooManyFrames(u64::MAX))
}

fn audio_file_info(spec: hound::WavSpec, frames: u64) -> Result<AudioFileInfo, AudioFileError> {
    if spec.sample_rate == 0 {
        return Err(AudioFileError::InvalidSampleRate(spec.sample_rate));
    }
    if spec.channels == 0 {
        return Err(AudioFileError::InvalidFormat(
            "channel count must be greater than zero".to_owned(),
        ));
    }

    let sample_format = match spec.sample_format {
        hound::SampleFormat::Int if (1..=32).contains(&spec.bits_per_sample) => {
            AudioSampleFormat::Integer
        }
        hound::SampleFormat::Float if spec.bits_per_sample == 32 => AudioSampleFormat::Float,
        hound::SampleFormat::Int => {
            return Err(AudioFileError::InvalidFormat(format!(
                "unsupported PCM bit depth: {}",
                spec.bits_per_sample
            )));
        }
        hound::SampleFormat::Float => {
            return Err(AudioFileError::InvalidFormat(format!(
                "unsupported float bit depth: {}",
                spec.bits_per_sample
            )));
        }
    };

    Ok(AudioFileInfo {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        frames,
        bits_per_sample: spec.bits_per_sample,
        sample_format,
    })
}

fn integer_scale(bits_per_sample: u16) -> Result<f32, AudioFileError> {
    let exponent = u32::from(bits_per_sample)
        .checked_sub(1)
        .ok_or_else(|| AudioFileError::InvalidFormat("PCM bit depth must be positive".into()))?;
    let scale = 1_u64
        .checked_shl(exponent)
        .ok_or_else(|| AudioFileError::InvalidFormat("PCM bit depth exceeds 32".into()))?;
    #[allow(clippy::cast_precision_loss)]
    Ok(scale as f32)
}

fn decode_to_stereo<S, I, F>(
    samples: I,
    channels: usize,
    estimated_samples: usize,
    mut convert: F,
) -> Result<Vec<f32>, AudioFileError>
where
    I: Iterator<Item = Result<S, hound::Error>>,
    F: FnMut(S, u64) -> Result<f32, AudioFileError>,
{
    let mut stereo = Vec::new();
    stereo
        .try_reserve_exact(estimated_samples)
        .map_err(|_| AudioFileError::TooManyFrames(u64::MAX))?;
    let mut left = 0.0;
    let mut sample_count = 0_u64;

    for (index, sample) in samples.enumerate() {
        let sample = sample?;
        let index_u64 = u64::try_from(index).unwrap_or(u64::MAX);
        let sample = convert(sample, index_u64)?;
        let channel = index % channels;
        if channels == 1 {
            stereo.push(sample);
            stereo.push(sample);
        } else if channel == 0 {
            left = sample;
        } else if channel == 1 {
            stereo.push(left);
            stereo.push(sample);
        }
        sample_count = sample_count
            .checked_add(1)
            .ok_or(AudioFileError::TooManyFrames(u64::MAX))?;
    }

    let channels_u64 = u64::try_from(channels).unwrap_or(u64::MAX);
    if !sample_count.is_multiple_of(channels_u64) {
        return Err(AudioFileError::InvalidFormat(
            "sample data ends in a partial frame".to_owned(),
        ));
    }
    Ok(stereo)
}

fn stereo_sample_capacity(frames: u64) -> Result<usize, AudioFileError> {
    let frames = usize::try_from(frames).map_err(|_| AudioFileError::TooManyFrames(frames))?;
    frames
        .checked_mul(2)
        .ok_or(AudioFileError::TooManyFrames(u64::MAX))
}

fn proportional_index(index: usize, frames: usize, peak_count: usize) -> usize {
    if peak_count == 0 {
        return 0;
    }
    index.saturating_mul(frames) / peak_count
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_FILE_ID: AtomicU64 = AtomicU64::new(0);

    fn test_path(label: &str) -> PathBuf {
        let id = NEXT_FILE_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("dmo-{label}-{}-{id}.wav", std::process::id()))
    }

    #[test]
    fn probes_and_decodes_mono_pcm_as_stereo() {
        let path = test_path("mono-pcm");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(i16::MIN).unwrap();
        writer.write_sample(0_i16).unwrap();
        writer.write_sample(i16::MAX).unwrap();
        writer.finalize().unwrap();

        let info = probe_wav(&path).unwrap();
        assert_eq!(info.sample_rate, 24_000);
        assert_eq!(info.channels, 1);
        assert_eq!(info.frames, 3);
        let decoded = decode_wav(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(decoded.samples.len(), 6);
        assert!((decoded.samples[0] + 1.0).abs() < f32::EPSILON);
        assert!((decoded.samples[0] - decoded.samples[1]).abs() < f32::EPSILON);
        assert!((decoded.samples[4] - 0.999_969_5).abs() < 0.000_01);
        assert!((decoded.samples[4] - decoded.samples[5]).abs() < f32::EPSILON);
    }

    #[test]
    fn decodes_float_stereo_without_changing_channels() {
        let path = test_path("float-stereo");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in [0.25_f32, -0.5, 1.25, -1.25] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let decoded = decode_wav(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(decoded.samples, vec![0.25, -0.5, 1.0, -1.0]);
    }

    #[test]
    fn resamples_with_linear_interpolation() {
        let decoded = DecodedAudio {
            info: AudioFileInfo {
                sample_rate: 2,
                channels: 1,
                frames: 2,
                bits_per_sample: 16,
                sample_format: AudioSampleFormat::Integer,
            },
            samples: vec![0.0, 0.0, 1.0, 1.0],
        };

        assert_eq!(
            decoded.resample_stereo(4).unwrap(),
            vec![0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0]
        );
    }

    #[test]
    fn overview_preserves_stereo_extrema() {
        let decoded = DecodedAudio {
            info: AudioFileInfo {
                sample_rate: 48_000,
                channels: 2,
                frames: 4,
                bits_per_sample: 32,
                sample_format: AudioSampleFormat::Float,
            },
            samples: vec![-1.0, 0.5, 0.25, -0.75, 0.8, 1.0, -0.4, -0.2],
        };

        let overview = decoded.overview(2);
        assert_eq!(overview.source_frames, 4);
        assert_eq!(
            overview.peaks[0],
            WaveformPeak {
                left_min: -1.0,
                left_max: 0.25,
                right_min: -0.75,
                right_max: 0.5,
            }
        );
        assert!((overview.peaks[1].left_min + 0.4).abs() < f32::EPSILON);
        assert!((overview.peaks[1].right_max - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn frame_rate_conversion_rounds_to_nearest() {
        assert_eq!(convert_frame_count(44_100, 44_100, 48_000).unwrap(), 48_000);
        assert_eq!(convert_frame_count(1, 2, 3).unwrap(), 2);
        assert!(convert_frame_count(1, 0, 48_000).is_err());
    }

    #[test]
    fn corrupt_wav_returns_an_error() {
        let path = test_path("corrupt");
        fs::write(&path, b"not a wave file").unwrap();
        let result = decode_wav(&path);
        let _ = fs::remove_file(path);
        assert!(result.is_err());
    }
}
