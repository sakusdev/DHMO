# DMO

DMO is an experimental, open-source digital audio workstation written in Rust.
The project starts with a small, testable audio core and will grow into a
cross-platform desktop DAW.

## What works today

- A project model with tempo, master output, stereo tracks, gain, pan, mute, and timeline clips
- Sample-accurate playback of multi-note MIDI parts through built-in oscillators or PCM SoundFonts
- Per-track Sine, Triangle, Saw, and Square instruments with MIDI channels 1–16
- SF2 SoundFont loading: extract available presets, select a bank/program per track, and render PCM sample zones internally
- Track mute/solo, renaming, duplication, and independent mixer controls
- Track and master level meters in the mixer panel
- A master output channel and linearly interpolated track-volume automation
- Audio recording with track arm, software monitoring, punch ranges,
  configurable pre-roll, loop-record take cycling, non-destructive take lanes,
  and section comping
- Selectable system audio-input devices with per-track Stereo 1–2, Mono Input 1,
  and Mono Input 2 recording routes
- WAV import for integer PCM and 32-bit float audio, including mono/stereo conversion
- Automatic linear resampling when a WAV and project use different sample rates
- Stereo waveform overviews drawn directly inside imported timeline clips, with file-change
  cache invalidation
- Media relinking for imported WAV clips while preserving timeline timing
- Non-destructive left/right clip trimming plus imported-audio slip and reverse editing with undo and redo
- Non-destructive imported-audio clip gain normalization from the selected source range peak
- Per-clip gain and non-destructive Fade In/Out with linear, equal-power, slow,
  and fast curve shapes plus timeline envelope overlays
- Automatic same-track crossfades for overlapping clips, with timeline overlap overlays
- Play, pause, stop, seek, and sample-accurate loop/cycle transport behavior
- Real-time playback through the default system audio device
- Offline stereo mixdown export as 16-bit PCM, 24-bit PCM, or 32-bit float WAV, plus per-track stem export
- Project consolidation that copies referenced WAV media into a portable project folder
- Versioned, human-readable `.dmo` project save/load (v21), with v1–v20 compatibility
- A desktop GUI with timeline, mixer routing, bus/send/insert controls, clip inspector,
  zoom, and playhead
- Channel-strip insert presets for vocal, bass, drums, master glue, and lo-fi color
- Track bounce/render-in-place and freeze-to-audio from the Arrange track list
- A resizable piano roll with note lanes, bar/beat grid, note selection, and click-to-add entry
- Clip copy/paste, duplication, playhead splitting, and two-dimensional MIDI note movement
- Per-note MIDI velocity editing, editable tempo, playback metronome, persistent cycle ranges,
  and loop markers
- Timeline markers with ruler labels, add/delete/edit controls, and previous/next navigation
- Arranger sections with colored ruler blocks and editable start/length/name controls
- Standard MIDI File import/export with multitrack, channel, tempo, note, and CC data
- MIDI quantize with strength/end options, swing feel, groove templates, scale conform,
  fixed note lengths, transpose, velocity trim, legato, deterministic humanize,
  and overlapping-note cleanup
- Click-editable MIDI CC lanes for volume, expression, modulation, sustain, pan, and common
  synth controls; CC7 and CC11 also drive the built-in instruments during playback/render
- Live MIDI input discovery/connection and timestamped recording with per-track Omni or
  channel filtering, pre-roll, punch ranges, held-note completion, CC, pitch wheel, channel
  pressure, and polyphonic aftertouch capture
- Retrospective MIDI capture for turning the last 30 seconds of live input into a clip
- External MIDI output discovery/connection for MIDI-clip playback, including note,
  controller, program-change, pitch-wheel, and aftertouch messages
- Pitch bend and aftertouch preservation in Standard MIDI Files, editable events in the
  inspector, and expressive playback through the built-in instruments
- MIDI program-change preservation in project files and Standard MIDI Files, with
  Inspector write/update/delete controls
- A Studio One-inspired Arrange toolbar with Select/Draw tools, configurable snap grid,
  and a dedicated bottom transport with bars/beats, time, tempo, and signature displays
- Undo/redo for track, mixer, and clip edits
- A CLI for creating, inspecting, rendering, and playing projects

This is still an early DAW core. Plug-in hosting and sample-accurate external
MIDI-device scheduling are not implemented yet.

## License

DMO is available under the [MIT License](LICENSE).

## Launch the desktop DAW

```sh
cargo run -p dmo-gui
cargo run -p dmo-gui -- my-song.dmo
```

The GUI supports native Open/Save/Import dialogs, WAV, stem, and MIDI import/export plus project consolidation,
real-time playback, stereo waveforms, timeline seeking, draggable clips with
sixteenth-note snapping, saved L/R cycle ranges, playhead-based non-destructive trimming, track
gain/pan/mute controls, track bounce/freeze to rendered audio tracks, imported-audio relink,
normalize, slip-edit, and reverse controls, grouped multi-note MIDI parts, and click-to-add
piano-roll editing inside the selected part. Its MIDI toolbar covers straight/swing/groove
quantize, scale conform, fixed note lengths, transpose, velocity, legato, humanize,
and overlap cleanup, while the CC lane can draw controller events directly. The
piano roll can be toggled from the View menu. The Inspector exposes track output routing, pre/post sends, channel
inserts, master inserts, and bus channel controls. Keyboard shortcuts:

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
cargo run -p dmo-app -- stems my-song.dmo Exports/stems
cargo run -p dmo-app -- consolidate my-song.dmo MySongBundle
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

1. Expandable take-lane overview and additional multichannel input routing
2. More built-in effects
3. Track, plug-in, tempo, and clip automation with editable curves
4. Low-latency MIDI input monitoring, sample-accurate external MIDI scheduling,
   editable poly-aftertouch lanes, and MPE
5. Time stretch, transient/warp editing, and transpose
6. Tempo, signature, chord, and video tracks
7. Deeper render-in-place options and archival/export polish
8. Stable track/clip IDs, grouped edit transactions, folders, and track templates
9. VST3/CLAP plug-in hosting, delay compensation, scanning, and sandbox recovery

## Platform notes

DMO uses the platform's native audio backend through CPAL: WASAPI on Windows,
CoreAudio on macOS, and ALSA by default on Linux. Building on Linux requires
the ALSA development package (for example, `libasound2-dev` on Debian/Ubuntu).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). DMO is licensed under the MIT License.
