# DMO

DMO is an experimental, open-source digital audio workstation written in Rust.
The project starts with a small, testable audio core and will grow into a
cross-platform desktop DAW.

## What works today

- A project model with tempo, master output, stereo tracks, gain, pan, mute, and timeline clips
- Sample-accurate playback of multi-note MIDI parts through the built-in tone generator
- Per-track Sine, Triangle, Saw, and Square instruments with MIDI channels 1–16
- Track mute/solo, renaming, duplication, and independent mixer controls
- A master output channel and linearly interpolated track-volume automation
- Default-input audio recording with track arm, software monitoring, punch ranges,
  configurable pre-roll, non-destructive take lanes, and section comping
- WAV import for integer PCM and 32-bit float audio, including mono/stereo conversion
- Automatic linear resampling when a WAV and project use different sample rates
- Stereo waveform overviews drawn directly inside imported timeline clips
- Non-destructive left/right clip trimming with undo and redo
- Per-clip gain and non-destructive Fade In/Out with timeline envelope overlays
- Play, pause, stop, seek, and sample-accurate loop transport behavior
- Real-time playback through the default system audio device
- Offline stereo 16-bit WAV rendering
- Versioned, human-readable `.dmo` project save/load (v12), with v1–v11 compatibility
- A desktop GUI with timeline, mixer controls, clip inspector, zoom, and playhead
- A resizable piano roll with note lanes, bar/beat grid, note selection, and click-to-add entry
- Clip copy/paste, duplication, playhead splitting, and two-dimensional MIDI note movement
- Per-note MIDI velocity editing, editable tempo, playback metronome, and loop markers
- Standard MIDI File import/export with multitrack, channel, tempo, note, and CC data
- MIDI quantize with strength/end options, transpose, velocity trim, legato, deterministic
  humanize, and overlapping-note cleanup
- Click-editable MIDI CC lanes for volume, expression, modulation, sustain, pan, and common
  synth controls; CC7 and CC11 also drive the built-in instruments during playback/render
- Live MIDI input discovery/connection and timestamped recording with per-track Omni or
  channel filtering, pre-roll, punch ranges, held-note completion, CC, pitch wheel, channel
  pressure, and polyphonic aftertouch capture
- Pitch bend and aftertouch preservation in Standard MIDI Files, editable events in the
  inspector, and expressive playback through the built-in instruments
- A Studio One-inspired Arrange toolbar with Select/Draw tools, configurable snap grid,
  and a dedicated bottom transport with bars/beats, time, tempo, and signature displays
- Undo/redo for track, mixer, and clip edits
- A CLI for creating, inspecting, rendering, and playing projects

This is still an early DAW core. Plug-in hosting and scheduled external MIDI-device
output are not implemented yet.

## Launch the desktop DAW

```sh
cargo run -p dmo-gui
cargo run -p dmo-gui -- my-song.dmo
```

The GUI supports native Open/Save/Import dialogs, WAV and MIDI import/export,
real-time playback, stereo waveforms, timeline seeking, draggable clips with
sixteenth-note snapping, playhead-based non-destructive trimming, track
gain/pan/mute controls, grouped multi-note MIDI parts, and click-to-add
piano-roll editing inside the selected part. Its MIDI toolbar covers quantize,
transpose, velocity, legato, humanize, and overlap cleanup, while the CC lane
can draw controller events directly. The piano roll can be toggled from the
View menu. Keyboard shortcuts:

- `Space`: play/pause
- `Ctrl/Cmd+S`: save
- `Ctrl/Cmd+Z`: undo
- `Ctrl/Cmd+Y`: redo
- `Ctrl/Cmd+C`: copy the selected clip
- `Ctrl/Cmd+V`: paste a clip at the playhead
- `Ctrl/Cmd+D`: duplicate the selected clip
- `Ctrl/Cmd+B`: split the selected clip at the playhead
- `Delete`: remove the selected clip
- `1`: Select tool
- `2`: Draw tool

## Command line

```sh
cargo run -p dmo-app -- new my-song.dmo
cargo run -p dmo-app -- info my-song.dmo
cargo run -p dmo-app -- play my-song.dmo
cargo run -p dmo-app -- render my-song.dmo my-song.wav
```

Run `cargo run -p dmo-app -- --help` for the complete command summary. Run the
checks with:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Architecture

- `dmo-core`: project/timeline types and deterministic offline audio rendering
- `dmo-audio`: lock-free callback adapter for native audio devices via CPAL
- `dmo-gui`: the eframe/egui desktop application
- `dmo-app`: headless project and rendering CLI

The audio core does not depend on a GUI or operating-system audio API. This
keeps timeline and mixing behavior independently testable and leaves room for a
real-time engine without coupling it to the interface.

## Roadmap

1. Fade curve types, crossfades, waveform cache invalidation, and media relinking
2. Input routing, loop-record take cycling, retrospective recording, and take-lane overview
3. Mixer routing UI, meters, channel-strip presets, and more built-in effects
4. Track, plug-in, tempo, and clip automation with editable curves
5. Low-latency MIDI input monitoring, external MIDI output, groove templates, program
   change, editable poly-aftertouch lanes, and MPE
6. Time stretch, transient/warp editing, transpose, reverse, normalize, and slip editing
7. Marker, arranger, tempo, signature, chord, and video tracks
8. Freeze, bounce, stem export, render-in-place, and project consolidation
9. Stable track/clip IDs, grouped edit transactions, folders, and track templates
10. VST3/CLAP plug-in hosting, delay compensation, scanning, and sandbox recovery

## Platform notes

DMO uses the platform's native audio backend through CPAL: WASAPI on Windows,
CoreAudio on macOS, and ALSA by default on Linux. Building on Linux requires
the ALSA development package (for example, `libasound2-dev` on Debian/Ubuntu).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). DMO is licensed under the MIT License.
