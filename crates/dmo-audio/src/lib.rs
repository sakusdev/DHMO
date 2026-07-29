//! Real-time audio-device integration for DMO.
//!
//! The playback callback reads an immutable, pre-rendered stereo buffer and
//! uses atomics for transport controls. It performs no allocation and takes no
//! locks on the audio thread.

use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use cpal::{
    FromSample, I24, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    SupportedStreamConfig, U24,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

/// The output device and format selected for a playback stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
}

/// A stable, selectable system audio-input endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Captured stereo input samples and the device configuration that produced them.
pub struct RecordedInput {
    pub samples: Vec<f32>,
    pub device_info: AudioDeviceInfo,
}

/// A live default-input recording stream with optional software monitoring.
pub struct Recording {
    input_stream: Stream,
    monitor_stream: Option<Stream>,
    state: Arc<RecordingState>,
    device_info: AudioDeviceInfo,
}

impl Recording {
    /// Starts recording the default input at the requested project sample rate.
    /// When `monitoring` is enabled, captured samples are also sent to the
    /// default output through a bounded queue.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] if a matching input/output configuration cannot
    /// be opened or started.
    pub fn start(sample_rate: u32, monitoring: bool) -> Result<Self, AudioError> {
        Self::start_on_device(sample_rate, monitoring, None)
    }

    /// Starts recording from a selected input-device ID, or from the current
    /// system default when `device_id` is `None`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] if the selected endpoint is unavailable or a
    /// matching input/output configuration cannot be opened or started.
    pub fn start_on_device(
        sample_rate: u32,
        monitoring: bool,
        device_id: Option<&str>,
    ) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let input_device = match device_id {
            Some(device_id) => host
                .input_devices()
                .map_err(AudioError::Backend)?
                .find(|device| {
                    device
                        .id()
                        .is_ok_and(|candidate| candidate.to_string() == device_id)
                })
                .ok_or_else(|| AudioError::InputDeviceUnavailable(device_id.to_owned()))?,
            None => host
                .default_input_device()
                .ok_or(AudioError::NoInputDevice)?,
        };
        let supported = choose_input_config(&input_device, sample_rate)?;
        let device_info = AudioDeviceInfo {
            name: input_device.to_string(),
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
            sample_format: supported.sample_format().to_string(),
        };
        let state = Arc::new(RecordingState::new(monitoring));
        let input_stream = build_input_stream(
            &input_device,
            supported.config(),
            supported.sample_format(),
            Arc::clone(&state),
        )?;
        let monitor_stream = if monitoring {
            let output_device = host
                .default_output_device()
                .ok_or(AudioError::NoOutputDevice)?;
            let output = choose_output_config(&output_device, sample_rate)?;
            Some(build_monitor_stream(
                &output_device,
                output.config(),
                output.sample_format(),
                Arc::clone(&state),
            )?)
        } else {
            None
        };
        input_stream.play().map_err(AudioError::Backend)?;
        if let Some(stream) = &monitor_stream {
            stream.play().map_err(AudioError::Backend)?;
        }
        Ok(Self {
            input_stream,
            monitor_stream,
            state,
            device_info,
        })
    }

    #[must_use]
    pub fn recorded_frames(&self) -> usize {
        self.state
            .samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
            / 2
    }

    #[must_use]
    pub fn has_stream_error(&self) -> bool {
        self.state.stream_error.load(Ordering::Acquire)
    }

    #[must_use]
    pub const fn device_info(&self) -> &AudioDeviceInfo {
        &self.device_info
    }

    /// Stops capture and returns every recorded stereo sample.
    #[must_use]
    pub fn finish(self) -> RecordedInput {
        self.state.active.store(false, Ordering::Release);
        drop(self.input_stream);
        drop(self.monitor_stream);
        let samples = std::mem::take(
            &mut *self
                .state
                .samples
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        RecordedInput {
            samples,
            device_info: self.device_info,
        }
    }
}

/// Enumerates currently available input endpoints with stable CPAL IDs.
///
/// # Errors
///
/// Returns [`AudioError`] when the platform audio host cannot enumerate or
/// identify its input endpoints.
pub fn available_input_devices() -> Result<Vec<AudioInputDevice>, AudioError> {
    let host = cpal::default_host();
    let default_id = host
        .default_input_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());
    let mut devices = host
        .input_devices()
        .map_err(AudioError::Backend)?
        .map(|device| {
            let id = device.id().map_err(AudioError::Backend)?.to_string();
            Ok(AudioInputDevice {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name: device.to_string(),
            })
        })
        .collect::<Result<Vec<_>, AudioError>>()?;
    devices.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(devices)
}

const MAX_MONITOR_SAMPLES: usize = 16_384;

struct RecordingState {
    samples: Mutex<Vec<f32>>,
    monitor: Mutex<VecDeque<f32>>,
    monitoring: bool,
    active: AtomicBool,
    stream_error: AtomicBool,
}

impl RecordingState {
    fn new(monitoring: bool) -> Self {
        Self {
            samples: Mutex::new(Vec::with_capacity(96_000)),
            monitor: Mutex::new(VecDeque::with_capacity(MAX_MONITOR_SAMPLES)),
            monitoring,
            active: AtomicBool::new(true),
            stream_error: AtomicBool::new(false),
        }
    }
}

/// Returns the format DMO would use on the default output device.
///
/// This is useful when creating a new project at the device's native sample
/// rate before opening a stream.
///
/// # Errors
///
/// Returns [`AudioError`] if no default output exists or its configuration
/// cannot be queried.
pub fn default_output_device_info() -> Result<AudioDeviceInfo, AudioError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?;
    let default = device
        .default_output_config()
        .map_err(AudioError::Backend)?;
    let selected = choose_output_config(&device, default.sample_rate())?;
    Ok(AudioDeviceInfo {
        name: device.to_string(),
        sample_rate: selected.sample_rate(),
        channels: selected.channels(),
        sample_format: selected.sample_format().to_string(),
    })
}

/// A live output stream. Keep this value alive for as long as audio should run.
pub struct Playback {
    _stream: Stream,
    handle: PlaybackHandle,
    device_info: AudioDeviceInfo,
}

impl Playback {
    /// Opens the default output device and starts playing an interleaved stereo
    /// buffer at `sample_rate`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] if the buffer is malformed, the default device is
    /// unavailable, the requested rate is unsupported, or the stream cannot be
    /// opened or started.
    pub fn start(samples: Vec<f32>, sample_rate: u32) -> Result<Self, AudioError> {
        Self::start_at(samples, sample_rate, 0)
    }

    /// Opens the default output device and starts at an absolute audio frame.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError`] under the same conditions as [`Self::start`], or
    /// when `start_frame` is past the supplied buffer.
    pub fn start_at(
        samples: Vec<f32>,
        sample_rate: u32,
        start_frame: usize,
    ) -> Result<Self, AudioError> {
        Self::start_at_with_state(samples, sample_rate, start_frame, true)
    }

    /// Opens and primes an output stream without advancing it. This lets a
    /// recorder prepare timeline audio before starting input and output close
    /// together, instead of capturing the time spent rendering the project.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::start_at`].
    pub fn start_at_paused(
        samples: Vec<f32>,
        sample_rate: u32,
        start_frame: usize,
    ) -> Result<Self, AudioError> {
        Self::start_at_with_state(samples, sample_rate, start_frame, false)
    }

    fn start_at_with_state(
        samples: Vec<f32>,
        sample_rate: u32,
        start_frame: usize,
        playing: bool,
    ) -> Result<Self, AudioError> {
        if !samples.len().is_multiple_of(2) {
            return Err(AudioError::OddStereoSampleCount(samples.len()));
        }

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;
        let supported = choose_output_config(&device, sample_rate)?;
        let device_info = AudioDeviceInfo {
            name: device.to_string(),
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
            sample_format: supported.sample_format().to_string(),
        };
        let sample_format = supported.sample_format();
        let config = supported.config();
        let state = Arc::new(PlaybackState::new(samples));
        let handle = PlaybackHandle {
            state: Arc::clone(&state),
        };
        let stream = build_output_stream(&device, config, sample_format, state)?;
        handle.seek(start_frame)?;
        if playing {
            handle.play();
        }
        stream.play().map_err(AudioError::Backend)?;

        Ok(Self {
            _stream: stream,
            handle,
            device_info,
        })
    }

    #[must_use]
    pub fn handle(&self) -> PlaybackHandle {
        self.handle.clone()
    }

    #[must_use]
    pub const fn device_info(&self) -> &AudioDeviceInfo {
        &self.device_info
    }
}

/// A cheap, thread-safe handle for controlling an active [`Playback`].
#[derive(Clone)]
pub struct PlaybackHandle {
    state: Arc<PlaybackState>,
}

impl PlaybackHandle {
    pub fn play(&self) {
        if self.position_frames() < self.duration_frames() {
            self.state.playing.store(true, Ordering::Release);
        }
    }

    pub fn pause(&self) {
        self.state.playing.store(false, Ordering::Release);
    }

    pub fn stop(&self) {
        self.pause();
        self.state.position.store(0, Ordering::Release);
    }

    /// Moves the playback cursor to an absolute frame.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::SeekOutOfRange`] if `frame` is past the end.
    pub fn seek(&self, frame: usize) -> Result<(), AudioError> {
        if frame > self.duration_frames() {
            return Err(AudioError::SeekOutOfRange {
                frame,
                duration: self.duration_frames(),
            });
        }
        self.state.position.store(frame, Ordering::Release);
        Ok(())
    }

    /// Enables a half-open loop range, `start..end`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidLoopRange`] for an empty, reversed, or
    /// out-of-bounds range.
    pub fn set_loop(&self, start: usize, end: usize) -> Result<(), AudioError> {
        if start >= end || end > self.duration_frames() {
            return Err(AudioError::InvalidLoopRange {
                start,
                end,
                duration: self.duration_frames(),
            });
        }
        self.state.loop_start.store(start, Ordering::Release);
        self.state.loop_end.store(end, Ordering::Release);
        self.state.looping.store(true, Ordering::Release);
        if self.position_frames() >= end {
            self.state.position.store(start, Ordering::Release);
        }
        Ok(())
    }

    pub fn clear_loop(&self) {
        self.state.looping.store(false, Ordering::Release);
    }

    /// Returns whether the backend has reported a stream failure.
    #[must_use]
    pub fn has_stream_error(&self) -> bool {
        self.state.stream_error.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.state.playing.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn position_frames(&self) -> usize {
        self.state.position.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn duration_frames(&self) -> usize {
        self.state.samples.len() / 2
    }
}

struct PlaybackState {
    samples: Box<[f32]>,
    position: AtomicUsize,
    playing: AtomicBool,
    looping: AtomicBool,
    loop_start: AtomicUsize,
    loop_end: AtomicUsize,
    stream_error: AtomicBool,
}

impl PlaybackState {
    fn new(samples: Vec<f32>) -> Self {
        let frame_count = samples.len() / 2;
        Self {
            samples: samples.into_boxed_slice(),
            position: AtomicUsize::new(0),
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            loop_start: AtomicUsize::new(0),
            loop_end: AtomicUsize::new(frame_count),
            stream_error: AtomicBool::new(false),
        }
    }

    fn next_frame(&self) -> [f32; 2] {
        if !self.playing.load(Ordering::Acquire) {
            return [0.0; 2];
        }

        let duration = self.samples.len() / 2;
        let looping = self.looping.load(Ordering::Acquire);
        let loop_start = self.loop_start.load(Ordering::Acquire);
        let loop_end = self.loop_end.load(Ordering::Acquire).min(duration);
        let position = self
            .position
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                let mut next = current.saturating_add(1);
                if looping && current < loop_end && next >= loop_end {
                    next = loop_start;
                } else if next >= duration {
                    next = duration;
                }
                Some(next)
            })
            .unwrap_or(duration);

        if position >= duration {
            self.playing.store(false, Ordering::Release);
            return [0.0; 2];
        }
        if !looping && position.saturating_add(1) >= duration {
            self.playing.store(false, Ordering::Release);
        }
        [self.samples[position * 2], self.samples[position * 2 + 1]]
    }
}

fn choose_output_config(
    device: &cpal::Device,
    sample_rate: u32,
) -> Result<SupportedStreamConfig, AudioError> {
    device
        .supported_output_configs()
        .map_err(AudioError::Backend)?
        .filter(|range| range.contains_rate(sample_rate) && is_pcm(range.sample_format()))
        .min_by_key(|range| {
            let channel_distance = range.channels().abs_diff(2);
            (format_priority(range.sample_format()), channel_distance)
        })
        .map(|range| range.with_sample_rate(sample_rate))
        .ok_or(AudioError::UnsupportedSampleRate(sample_rate))
}

fn choose_input_config(
    device: &cpal::Device,
    sample_rate: u32,
) -> Result<SupportedStreamConfig, AudioError> {
    device
        .supported_input_configs()
        .map_err(AudioError::Backend)?
        .filter(|range| range.contains_rate(sample_rate) && is_pcm(range.sample_format()))
        .min_by_key(|range| {
            let channel_distance = range.channels().abs_diff(2);
            (format_priority(range.sample_format()), channel_distance)
        })
        .map(|range| range.with_sample_rate(sample_rate))
        .ok_or(AudioError::UnsupportedInputSampleRate(sample_rate))
}

const fn is_pcm(format: SampleFormat) -> bool {
    !matches!(
        format,
        SampleFormat::DsdU8 | SampleFormat::DsdU16 | SampleFormat::DsdU32
    )
}

const fn format_priority(format: SampleFormat) -> u8 {
    match format {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        SampleFormat::U16 => 2,
        SampleFormat::F64 => 3,
        SampleFormat::I24 | SampleFormat::I32 => 4,
        SampleFormat::U24 | SampleFormat::U32 => 5,
        SampleFormat::I8 | SampleFormat::I64 => 6,
        SampleFormat::U8 | SampleFormat::U64 => 7,
        _ => u8::MAX,
    }
}

fn build_output_stream(
    device: &cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    state: Arc<PlaybackState>,
) -> Result<Stream, AudioError> {
    let stream = match format {
        SampleFormat::I8 => build_typed_stream::<i8>(device, config, state),
        SampleFormat::I16 => build_typed_stream::<i16>(device, config, state),
        SampleFormat::I24 => build_typed_stream::<I24>(device, config, state),
        SampleFormat::I32 => build_typed_stream::<i32>(device, config, state),
        SampleFormat::I64 => build_typed_stream::<i64>(device, config, state),
        SampleFormat::U8 => build_typed_stream::<u8>(device, config, state),
        SampleFormat::U16 => build_typed_stream::<u16>(device, config, state),
        SampleFormat::U24 => build_typed_stream::<U24>(device, config, state),
        SampleFormat::U32 => build_typed_stream::<u32>(device, config, state),
        SampleFormat::U64 => build_typed_stream::<u64>(device, config, state),
        SampleFormat::F32 => build_typed_stream::<f32>(device, config, state),
        SampleFormat::F64 => build_typed_stream::<f64>(device, config, state),
        _ => return Err(AudioError::UnsupportedSampleFormat(format.to_string())),
    }
    .map_err(AudioError::Backend)?;
    Ok(stream)
}

fn build_input_stream(
    device: &cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    state: Arc<RecordingState>,
) -> Result<Stream, AudioError> {
    let stream = match format {
        SampleFormat::I8 => build_typed_input_stream::<i8>(device, config, state),
        SampleFormat::I16 => build_typed_input_stream::<i16>(device, config, state),
        SampleFormat::I24 => build_typed_input_stream::<I24>(device, config, state),
        SampleFormat::I32 => build_typed_input_stream::<i32>(device, config, state),
        SampleFormat::I64 => build_typed_input_stream::<i64>(device, config, state),
        SampleFormat::U8 => build_typed_input_stream::<u8>(device, config, state),
        SampleFormat::U16 => build_typed_input_stream::<u16>(device, config, state),
        SampleFormat::U24 => build_typed_input_stream::<U24>(device, config, state),
        SampleFormat::U32 => build_typed_input_stream::<u32>(device, config, state),
        SampleFormat::U64 => build_typed_input_stream::<u64>(device, config, state),
        SampleFormat::F32 => build_typed_input_stream::<f32>(device, config, state),
        SampleFormat::F64 => build_typed_input_stream::<f64>(device, config, state),
        _ => return Err(AudioError::UnsupportedSampleFormat(format.to_string())),
    }
    .map_err(AudioError::Backend)?;
    Ok(stream)
}

fn build_typed_input_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    state: Arc<RecordingState>,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let error_state = Arc::clone(&state);
    device.build_input_stream(
        config,
        move |input: &[T], _| capture_input(input, channels, &state),
        move |_| error_state.stream_error.store(true, Ordering::Release),
        None,
    )
}

fn capture_input<T>(input: &[T], channels: usize, state: &RecordingState)
where
    T: Sample,
    f32: FromSample<T>,
{
    if !state.active.load(Ordering::Acquire) {
        return;
    }
    let channels = channels.max(1);
    let mut samples = state
        .samples
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut monitor = state.monitoring.then(|| {
        state
            .monitor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    });
    for frame in input.chunks(channels) {
        let left = f32::from_sample(frame[0]);
        let right = frame
            .get(1)
            .map_or(left, |sample| f32::from_sample(*sample));
        samples.extend([left, right]);
        if let Some(queue) = &mut monitor {
            while queue.len() > MAX_MONITOR_SAMPLES.saturating_sub(2) {
                queue.pop_front();
            }
            queue.extend([left, right]);
        }
    }
}

fn build_monitor_stream(
    device: &cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    state: Arc<RecordingState>,
) -> Result<Stream, AudioError> {
    let stream = match format {
        SampleFormat::I8 => build_typed_monitor_stream::<i8>(device, config, state),
        SampleFormat::I16 => build_typed_monitor_stream::<i16>(device, config, state),
        SampleFormat::I24 => build_typed_monitor_stream::<I24>(device, config, state),
        SampleFormat::I32 => build_typed_monitor_stream::<i32>(device, config, state),
        SampleFormat::I64 => build_typed_monitor_stream::<i64>(device, config, state),
        SampleFormat::U8 => build_typed_monitor_stream::<u8>(device, config, state),
        SampleFormat::U16 => build_typed_monitor_stream::<u16>(device, config, state),
        SampleFormat::U24 => build_typed_monitor_stream::<U24>(device, config, state),
        SampleFormat::U32 => build_typed_monitor_stream::<u32>(device, config, state),
        SampleFormat::U64 => build_typed_monitor_stream::<u64>(device, config, state),
        SampleFormat::F32 => build_typed_monitor_stream::<f32>(device, config, state),
        SampleFormat::F64 => build_typed_monitor_stream::<f64>(device, config, state),
        _ => return Err(AudioError::UnsupportedSampleFormat(format.to_string())),
    }
    .map_err(AudioError::Backend)?;
    Ok(stream)
}

fn build_typed_monitor_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    state: Arc<RecordingState>,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let error_state = Arc::clone(&state);
    device.build_output_stream(
        config,
        move |output: &mut [T], _| write_monitor_output(output, channels, &state),
        move |_| error_state.stream_error.store(true, Ordering::Release),
        None,
    )
}

fn write_monitor_output<T>(output: &mut [T], channels: usize, state: &RecordingState)
where
    T: Sample + FromSample<f32>,
{
    let mut queue = state
        .monitor
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for frame in output.chunks_mut(channels.max(1)) {
        let left = queue.pop_front().unwrap_or(0.0);
        let right = queue.pop_front().unwrap_or(left);
        if frame.len() == 1 {
            frame[0] = T::from_sample((left + right) * 0.5);
            continue;
        }
        frame[0] = T::from_sample(left);
        frame[1] = T::from_sample(right);
        for sample in &mut frame[2..] {
            *sample = T::EQUILIBRIUM;
        }
    }
}

fn build_typed_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    state: Arc<PlaybackState>,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let error_state = Arc::clone(&state);
    device.build_output_stream(
        config,
        move |output: &mut [T], _| write_output(output, channels, &state),
        move |_| error_state.stream_error.store(true, Ordering::Release),
        None,
    )
}

fn write_output<T>(output: &mut [T], channels: usize, state: &PlaybackState)
where
    T: Sample + FromSample<f32>,
{
    for frame in output.chunks_mut(channels.max(1)) {
        let [left, right] = state.next_frame();
        if frame.len() == 1 {
            frame[0] = T::from_sample((left + right) * 0.5);
            continue;
        }
        frame[0] = T::from_sample(left);
        frame[1] = T::from_sample(right);
        for sample in &mut frame[2..] {
            *sample = T::EQUILIBRIUM;
        }
    }
}

/// Errors raised while configuring or controlling audio playback.
#[derive(Debug)]
pub enum AudioError {
    NoOutputDevice,
    NoInputDevice,
    InputDeviceUnavailable(String),
    OddStereoSampleCount(usize),
    UnsupportedSampleRate(u32),
    UnsupportedInputSampleRate(u32),
    UnsupportedSampleFormat(String),
    SeekOutOfRange {
        frame: usize,
        duration: usize,
    },
    InvalidLoopRange {
        start: usize,
        end: usize,
        duration: usize,
    },
    Backend(cpal::Error),
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOutputDevice => formatter.write_str("no default audio output device"),
            Self::NoInputDevice => formatter.write_str("no default audio input device"),
            Self::InputDeviceUnavailable(id) => {
                write!(formatter, "the selected audio input is unavailable: {id}")
            }
            Self::OddStereoSampleCount(count) => {
                write!(formatter, "stereo sample count must be even, got {count}")
            }
            Self::UnsupportedSampleRate(rate) => {
                write!(
                    formatter,
                    "the default output device does not support {rate} Hz"
                )
            }
            Self::UnsupportedInputSampleRate(rate) => {
                write!(
                    formatter,
                    "the default input device does not support {rate} Hz"
                )
            }
            Self::UnsupportedSampleFormat(format) => {
                write!(formatter, "unsupported output sample format: {format}")
            }
            Self::SeekOutOfRange { frame, duration } => {
                write!(
                    formatter,
                    "cannot seek to frame {frame}; duration is {duration}"
                )
            }
            Self::InvalidLoopRange {
                start,
                end,
                duration,
            } => write!(
                formatter,
                "invalid loop range {start}..{end}; duration is {duration}"
            ),
            Self::Backend(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AudioError {}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn playing_state(samples: Vec<f32>) -> PlaybackState {
        let state = PlaybackState::new(samples);
        state.playing.store(true, Ordering::Release);
        state
    }

    #[test]
    fn consumes_stereo_frames_and_stops_at_the_end() {
        let state = playing_state(vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(state.next_frame(), [0.1, 0.2]);
        assert_eq!(state.next_frame(), [0.3, 0.4]);
        assert!(!state.playing.load(Ordering::Acquire));
        assert_eq!(state.next_frame(), [0.0, 0.0]);
    }

    #[test]
    fn primed_playback_does_not_advance_until_started() {
        let state = PlaybackState::new(vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(state.next_frame(), [0.0, 0.0]);
        assert_eq!(state.position.load(Ordering::Acquire), 0);
        state.playing.store(true, Ordering::Release);
        assert_eq!(state.next_frame(), [0.1, 0.2]);
        assert_eq!(state.position.load(Ordering::Acquire), 1);
    }

    #[test]
    fn loop_wraps_without_leaving_the_audio_callback() {
        let state = playing_state(vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        state.loop_start.store(1, Ordering::Release);
        state.loop_end.store(3, Ordering::Release);
        state.looping.store(true, Ordering::Release);
        assert_eq!(state.next_frame(), [0.1, 0.2]);
        assert_eq!(state.next_frame(), [0.3, 0.4]);
        assert_eq!(state.next_frame(), [0.5, 0.6]);
        assert_eq!(state.next_frame(), [0.3, 0.4]);
    }

    #[test]
    fn maps_stereo_to_multichannel_output() {
        let state = playing_state(vec![0.25, -0.5]);
        let mut output = [1.0_f32; 4];
        write_output(&mut output, 4, &state);
        assert_eq!(output, [0.25, -0.5, 0.0, 0.0]);
    }

    #[test]
    fn captures_mono_as_stereo_and_feeds_the_monitor_queue() {
        let state = RecordingState::new(true);
        capture_input(&[0.25_f32, -0.5], 1, &state);

        assert_eq!(*state.samples.lock().unwrap(), vec![0.25, 0.25, -0.5, -0.5]);
        let mut output = [0.0_f32; 4];
        write_monitor_output(&mut output, 2, &state);
        assert_eq!(output, [0.25, 0.25, -0.5, -0.5]);
    }

    #[test]
    fn captures_the_first_two_channels_from_multichannel_input() {
        let state = RecordingState::new(false);
        capture_input(&[0.1_f32, 0.2, 0.9, 0.3, 0.4, 0.8], 3, &state);

        assert_eq!(*state.samples.lock().unwrap(), vec![0.1, 0.2, 0.3, 0.4]);
        assert!(state.monitor.lock().unwrap().is_empty());
    }
}
