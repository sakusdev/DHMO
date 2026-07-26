//! Minimal PCM `SoundFont` 2 (SF2) reader and sampler.
//!
//! DMO deliberately keeps the `SoundFont` file external and reads the standard
//! `pdta` tables plus `smpl` PCM data. This supports the common melodic and
//! drum `SoundFonts` without importing a heavyweight runtime dependency.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::struct_field_names
)]

use std::{fmt, fs, path::Path};

use crate::SoundFontPreset;

const GENERATOR_INSTRUMENT: u16 = 41;
const GENERATOR_KEY_RANGE: u16 = 43;
const GENERATOR_COARSE_TUNE: u16 = 51;
const GENERATOR_FINE_TUNE: u16 = 52;
const GENERATOR_SAMPLE_ID: u16 = 53;
const GENERATOR_SAMPLE_MODES: u16 = 54;
const GENERATOR_OVERRIDING_ROOT_KEY: u16 = 58;

#[derive(Debug)]
pub enum SoundFontError {
    Io(std::io::Error),
    Invalid(String),
}

impl fmt::Display for SoundFontError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "SoundFont I/O failed: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid SoundFont: {message}"),
        }
    }
}

impl std::error::Error for SoundFontError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<std::io::Error> for SoundFontError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundFontInfo {
    pub presets: Vec<SoundFontPreset>,
}

/// Reads the selectable presets from an SF2 file without decoding its samples.
///
/// # Errors
///
/// Returns [`SoundFontError`] when the file cannot be read or does not contain
/// the required SF2 tables.
pub fn inspect_soundfont(path: impl AsRef<Path>) -> Result<SoundFontInfo, SoundFontError> {
    let path = path.as_ref();
    let data = fs::read(path)?;
    let parsed = ParsedSoundFont::parse(&data, false)?;
    let path = path.to_string_lossy().into_owned();
    Ok(SoundFontInfo {
        presets: parsed
            .presets
            .iter()
            .map(|preset| SoundFontPreset::new(&path, preset.bank, preset.program, &preset.name))
            .collect(),
    })
}

pub(crate) fn load_soundfont(path: &str) -> Result<ParsedSoundFont, SoundFontError> {
    ParsedSoundFont::parse(&fs::read(path)?, true)
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedSoundFont {
    presets: Vec<Preset>,
    preset_bags: Vec<Bag>,
    preset_generators: Vec<Generator>,
    instruments: Vec<Instrument>,
    instrument_bags: Vec<Bag>,
    instrument_generators: Vec<Generator>,
    samples: Vec<Sample>,
    pcm: Vec<i16>,
}

impl ParsedSoundFont {
    fn parse(data: &[u8], include_samples: bool) -> Result<Self, SoundFontError> {
        if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"sfbk" {
            return Err(SoundFontError::Invalid("expected RIFF sfbk header".into()));
        }
        let mut chunks = Chunks::default();
        collect_chunks(data, 12, data.len(), &mut chunks)?;
        let phdr = chunks
            .phdr
            .ok_or_else(|| SoundFontError::Invalid("missing phdr table".into()))?;
        let pbag = chunks
            .pbag
            .ok_or_else(|| SoundFontError::Invalid("missing pbag table".into()))?;
        let pgen = chunks
            .pgen
            .ok_or_else(|| SoundFontError::Invalid("missing pgen table".into()))?;
        let inst = chunks
            .inst
            .ok_or_else(|| SoundFontError::Invalid("missing inst table".into()))?;
        let ibag = chunks
            .ibag
            .ok_or_else(|| SoundFontError::Invalid("missing ibag table".into()))?;
        let igen = chunks
            .igen
            .ok_or_else(|| SoundFontError::Invalid("missing igen table".into()))?;
        let shdr = chunks
            .shdr
            .ok_or_else(|| SoundFontError::Invalid("missing shdr table".into()))?;
        let presets = parse_presets(phdr)?;
        let preset_bags = parse_bags(pbag)?;
        let preset_generators = parse_generators(pgen)?;
        let instruments = parse_instruments(inst)?;
        let instrument_bags = parse_bags(ibag)?;
        let instrument_generators = parse_generators(igen)?;
        let samples = parse_samples(shdr)?;
        if presets.is_empty() || instruments.is_empty() || samples.is_empty() {
            return Err(SoundFontError::Invalid(
                "SoundFont has no playable preset data".into(),
            ));
        }
        let pcm = if include_samples {
            let smpl = chunks
                .smpl
                .ok_or_else(|| SoundFontError::Invalid("missing smpl data".into()))?;
            smpl.chunks_exact(2)
                .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self {
            presets,
            preset_bags,
            preset_generators,
            instruments,
            instrument_bags,
            instrument_generators,
            samples,
            pcm,
        })
    }

    pub(crate) fn sample_for_note(
        &self,
        preset: &SoundFontPreset,
        note: u8,
    ) -> Option<SampleVoice> {
        let preset_index = self.presets.iter().position(|candidate| {
            candidate.bank == preset.bank && candidate.program == preset.program
        })?;
        let next_preset_bag = self
            .presets
            .get(preset_index + 1)
            .map_or(self.preset_bags.len(), |item| usize::from(item.bag));
        let preset_bag = usize::from(self.presets[preset_index].bag);
        for bag_index in preset_bag..next_preset_bag {
            let (instrument_index, preset_range) = self.preset_zone(bag_index)?;
            if !preset_range.contains(&note) {
                continue;
            }
            let next_instrument_bag = self
                .instruments
                .get(instrument_index + 1)
                .map_or(self.instrument_bags.len(), |item| usize::from(item.bag));
            let instrument_bag = usize::from(self.instruments[instrument_index].bag);
            for zone_index in instrument_bag..next_instrument_bag {
                let zone = self.instrument_zone(zone_index)?;
                if !zone.key_range.contains(&note) {
                    continue;
                }
                let sample = self.samples.get(zone.sample_id)?;
                return Some(SampleVoice {
                    sample: sample.clone(),
                    coarse_tune: zone.coarse_tune,
                    fine_tune: zone.fine_tune,
                    root_key: zone.root_key,
                    looping: zone.looping,
                });
            }
        }
        None
    }

    fn preset_zone(&self, bag_index: usize) -> Option<(usize, std::ops::RangeInclusive<u8>)> {
        let bag = self.preset_bags.get(bag_index)?;
        let next = self.preset_bags.get(bag_index + 1)?;
        let mut instrument = None;
        let mut key_range = 0..=127;
        for generator in
            &self.preset_generators[usize::from(bag.generator)..usize::from(next.generator)]
        {
            match generator.operator {
                GENERATOR_INSTRUMENT => instrument = Some(usize::from(generator.amount)),
                GENERATOR_KEY_RANGE => key_range = range(generator.amount),
                _ => {}
            }
        }
        instrument.map(|instrument| (instrument, key_range))
    }

    fn instrument_zone(&self, bag_index: usize) -> Option<Zone> {
        let bag = self.instrument_bags.get(bag_index)?;
        let next = self.instrument_bags.get(bag_index + 1)?;
        let mut zone = Zone::default();
        for generator in
            &self.instrument_generators[usize::from(bag.generator)..usize::from(next.generator)]
        {
            match generator.operator {
                GENERATOR_SAMPLE_ID => zone.sample_id = usize::from(generator.amount),
                GENERATOR_KEY_RANGE => zone.key_range = range(generator.amount),
                GENERATOR_COARSE_TUNE => zone.coarse_tune = generator.amount as i16,
                GENERATOR_FINE_TUNE => zone.fine_tune = generator.amount as i16,
                GENERATOR_SAMPLE_MODES => zone.looping = generator.amount & 1 != 0,
                GENERATOR_OVERRIDING_ROOT_KEY => zone.root_key = Some(generator.amount as u8),
                _ => {}
            }
        }
        (zone.sample_id != usize::MAX).then_some(zone)
    }

    pub(crate) fn pcm_at(&self, index: usize) -> Option<i16> {
        self.pcm.get(index).copied()
    }
}

#[derive(Default)]
struct Chunks<'a> {
    phdr: Option<&'a [u8]>,
    pbag: Option<&'a [u8]>,
    pgen: Option<&'a [u8]>,
    inst: Option<&'a [u8]>,
    ibag: Option<&'a [u8]>,
    igen: Option<&'a [u8]>,
    shdr: Option<&'a [u8]>,
    smpl: Option<&'a [u8]>,
}

fn collect_chunks<'a>(
    data: &'a [u8],
    mut offset: usize,
    end: usize,
    chunks: &mut Chunks<'a>,
) -> Result<(), SoundFontError> {
    while offset.checked_add(8).is_some_and(|next| next <= end) {
        let id = &data[offset..offset + 4];
        let length = usize::try_from(read_u32(data, offset + 4)?)
            .map_err(|_| SoundFontError::Invalid("chunk too large".into()))?;
        let start = offset + 8;
        let chunk_end = start
            .checked_add(length)
            .filter(|next| *next <= end)
            .ok_or_else(|| SoundFontError::Invalid("truncated chunk".into()))?;
        if id == b"LIST" {
            if length < 4 {
                return Err(SoundFontError::Invalid("short LIST chunk".into()));
            }
            collect_chunks(data, start + 4, chunk_end, chunks)?;
        } else {
            let target = match id {
                b"phdr" => &mut chunks.phdr,
                b"pbag" => &mut chunks.pbag,
                b"pgen" => &mut chunks.pgen,
                b"inst" => &mut chunks.inst,
                b"ibag" => &mut chunks.ibag,
                b"igen" => &mut chunks.igen,
                b"shdr" => &mut chunks.shdr,
                b"smpl" => &mut chunks.smpl,
                _ => {
                    offset = chunk_end + length % 2;
                    continue;
                }
            };
            *target = Some(&data[start..chunk_end]);
        }
        offset = chunk_end + length % 2;
    }
    Ok(())
}

fn parse_presets(data: &[u8]) -> Result<Vec<Preset>, SoundFontError> {
    let mut presets = parse_records(data, 38, |record| {
        Ok(Preset {
            name: name(record),
            program: read_u16(record, 20)?,
            bank: read_u16(record, 22)?,
            bag: read_u16(record, 24)?,
        })
    })?;
    presets.pop();
    Ok(presets)
}
fn parse_instruments(data: &[u8]) -> Result<Vec<Instrument>, SoundFontError> {
    let mut instruments = parse_records(data, 22, |record| {
        Ok(Instrument {
            bag: read_u16(record, 20)?,
        })
    })?;
    instruments.pop();
    Ok(instruments)
}
fn parse_bags(data: &[u8]) -> Result<Vec<Bag>, SoundFontError> {
    parse_records(data, 4, |record| {
        Ok(Bag {
            generator: read_u16(record, 0)?,
        })
    })
}
fn parse_generators(data: &[u8]) -> Result<Vec<Generator>, SoundFontError> {
    parse_records(data, 4, |record| {
        Ok(Generator {
            operator: read_u16(record, 0)?,
            amount: read_u16(record, 2)?,
        })
    })
}
fn parse_samples(data: &[u8]) -> Result<Vec<Sample>, SoundFontError> {
    let mut samples = parse_records(data, 46, |record| {
        Ok(Sample {
            start: read_u32(record, 20)?,
            end: read_u32(record, 24)?,
            loop_start: read_u32(record, 28)?,
            loop_end: read_u32(record, 32)?,
            sample_rate: read_u32(record, 36)?,
            original_pitch: record[40],
            pitch_correction: record[41] as i8,
        })
    })?;
    samples.pop();
    Ok(samples)
}
fn parse_records<T>(
    data: &[u8],
    size: usize,
    parse: impl Fn(&[u8]) -> Result<T, SoundFontError>,
) -> Result<Vec<T>, SoundFontError> {
    if !data.len().is_multiple_of(size) {
        return Err(SoundFontError::Invalid("malformed table length".into()));
    }
    data.chunks_exact(size).map(parse).collect()
}
fn name(record: &[u8]) -> String {
    let end = record[..20]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(20);
    String::from_utf8_lossy(&record[..end]).trim().to_owned()
}
fn read_u16(data: &[u8], offset: usize) -> Result<u16, SoundFontError> {
    data.get(offset..offset + 2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .ok_or_else(|| SoundFontError::Invalid("truncated table record".into()))
}
fn read_u32(data: &[u8], offset: usize) -> Result<u32, SoundFontError> {
    data.get(offset..offset + 4)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .ok_or_else(|| SoundFontError::Invalid("truncated table record".into()))
}
fn range(amount: u16) -> std::ops::RangeInclusive<u8> {
    (amount as u8)..=((amount >> 8) as u8)
}

#[derive(Debug, Clone)]
struct Preset {
    name: String,
    program: u16,
    bank: u16,
    bag: u16,
}
#[derive(Debug, Clone)]
struct Instrument {
    bag: u16,
}
#[derive(Debug, Clone)]
struct Bag {
    generator: u16,
}
#[derive(Debug, Clone)]
struct Generator {
    operator: u16,
    amount: u16,
}
#[derive(Debug, Clone)]
pub(crate) struct Sample {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) loop_start: u32,
    pub(crate) loop_end: u32,
    pub(crate) sample_rate: u32,
    pub(crate) original_pitch: u8,
    pub(crate) pitch_correction: i8,
}
#[derive(Debug, Clone)]
pub(crate) struct SampleVoice {
    pub(crate) sample: Sample,
    pub(crate) coarse_tune: i16,
    pub(crate) fine_tune: i16,
    pub(crate) root_key: Option<u8>,
    pub(crate) looping: bool,
}
#[derive(Debug, Clone)]
struct Zone {
    sample_id: usize,
    key_range: std::ops::RangeInclusive<u8>,
    coarse_tune: i16,
    fine_tune: i16,
    root_key: Option<u8>,
    looping: bool,
}
impl Default for Zone {
    fn default() -> Self {
        Self {
            sample_id: usize::MAX,
            key_range: 0..=127,
            coarse_tune: 0,
            fine_tune: 0,
            root_key: None,
            looping: false,
        }
    }
}
