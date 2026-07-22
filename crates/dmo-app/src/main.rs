use std::{
    env,
    error::Error,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use dmo_audio::{Playback, default_output_device_info};
use dmo_core::{
    Clip, ClipSource, Instrument, MidiNote, Project, Track, load_project, save_project,
    try_render_stereo, write_stereo_i16_wav,
};

fn main() -> Result<(), Box<dyn Error>> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    run(&args)
}

fn run(args: &[OsString]) -> Result<(), Box<dyn Error>> {
    let Some(command) = args.first() else {
        print_help();
        return Ok(());
    };
    let command = command.to_string_lossy();
    let operands = &args[1..];

    match command.as_ref() {
        "-h" | "--help" | "help" => print_help(),
        "new" => command_new(operands)?,
        "info" => command_info(operands)?,
        "render" => command_render(operands)?,
        "play" => command_play(operands)?,
        _ if args.len() == 1 => {
            // Preserve the first MVP's `dmo output.wav` behavior.
            render_project(&demo_project(48_000)?, Path::new(command.as_ref()))?;
        }
        _ => return Err(invalid_arguments("unknown command or too many arguments")),
    }
    Ok(())
}

fn command_new(args: &[OsString]) -> Result<(), Box<dyn Error>> {
    let path = optional_single_path(args, "demo.dmo")?;
    let project = demo_project(48_000)?;
    save_project(&path, &project)?;
    println!("Created project '{}' at {}", project.name, path.display());
    Ok(())
}

fn command_info(args: &[OsString]) -> Result<(), Box<dyn Error>> {
    let path = required_single_path(args, "info requires a .dmo project path")?;
    let project = load_project(&path)?;
    print_project_info(&project, Some(&path));
    Ok(())
}

fn command_render(args: &[OsString]) -> Result<(), Box<dyn Error>> {
    match args {
        [] => render_project(&demo_project(48_000)?, Path::new("demo.wav"))?,
        [output] => render_project(&demo_project(48_000)?, Path::new(output))?,
        [input, output] => render_project(&load_project(input)?, Path::new(output))?,
        _ => {
            return Err(invalid_arguments(
                "render accepts at most input and output paths",
            ));
        }
    }
    Ok(())
}

fn command_play(args: &[OsString]) -> Result<(), Box<dyn Error>> {
    let project = match args {
        [] => {
            let device = default_output_device_info()?;
            demo_project(device.sample_rate)?
        }
        [input] => load_project(input)?,
        _ => return Err(invalid_arguments("play accepts at most one project path")),
    };
    play_project(&project)
}

fn render_project(project: &Project, output: &Path) -> Result<(), Box<dyn Error>> {
    let samples = try_render_stereo(project)?;
    write_stereo_i16_wav(output, project.sample_rate, &samples)?;
    #[allow(clippy::cast_precision_loss)]
    let duration_seconds = project.duration_frames() as f64 / f64::from(project.sample_rate);
    println!(
        "Rendered '{}' ({} tracks, {duration_seconds:.2} seconds) to {}",
        project.name,
        project.tracks.len(),
        output.display()
    );
    Ok(())
}

fn play_project(project: &Project) -> Result<(), Box<dyn Error>> {
    let samples = try_render_stereo(project)?;
    let playback = Playback::start(samples, project.sample_rate)?;
    let handle = playback.handle();
    let device = playback.device_info();
    println!(
        "Playing '{}' through {} ({} Hz, {} channels, {})",
        project.name, device.name, device.sample_rate, device.channels, device.sample_format
    );

    while handle.is_playing() {
        if handle.has_stream_error() {
            return Err(io::Error::other("the audio output stream failed").into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    if handle.has_stream_error() {
        return Err(io::Error::other("the audio output stream failed").into());
    }

    // Allow the device buffer containing the final frames to drain before the
    // stream is dropped.
    thread::sleep(Duration::from_millis(100));
    println!("Playback finished");
    Ok(())
}

fn print_project_info(project: &Project, path: Option<&Path>) {
    #[allow(clippy::cast_precision_loss)]
    let duration_seconds = project.duration_frames() as f64 / f64::from(project.sample_rate);
    if let Some(path) = path {
        println!("Project: {}", path.display());
    }
    println!("Name: {}", project.name);
    println!("Sample rate: {} Hz", project.sample_rate);
    println!("Tempo: {:.2} BPM", project.tempo_bpm);
    println!("Master gain: {:.2}", project.master_gain);
    println!("Tracks: {}", project.tracks.len());
    println!("Duration: {duration_seconds:.2} seconds");
    for (index, track) in project.tracks.iter().enumerate() {
        println!(
            "  {}. {} — {} clips, {} automation points, gain {:.2}, pan {:.2}{}",
            index + 1,
            track.name,
            track.clips.len(),
            track.volume_automation.len(),
            track.gain,
            track.pan,
            if track.muted { ", muted" } else { "" }
        );
    }
}

fn optional_single_path(args: &[OsString], default: &str) -> Result<PathBuf, Box<dyn Error>> {
    match args {
        [] => Ok(PathBuf::from(default)),
        [path] => Ok(PathBuf::from(path)),
        _ => Err(invalid_arguments("expected at most one path")),
    }
}

fn required_single_path(args: &[OsString], message: &str) -> Result<PathBuf, Box<dyn Error>> {
    match args {
        [path] => Ok(PathBuf::from(path)),
        _ => Err(invalid_arguments(message)),
    }
}

fn invalid_arguments(message: &str) -> Box<dyn Error> {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{message}; run `dmo --help` for usage"),
    )
    .into()
}

fn print_help() {
    println!(
        "DMO — an open-source digital audio workstation\n\
         \nUSAGE:\n\
         \x20 dmo new [project.dmo]\n\
         \x20 dmo info <project.dmo>\n\
         \x20 dmo render [output.wav]\n\
         \x20 dmo render <project.dmo> <output.wav>\n\
         \x20 dmo play [project.dmo]\n\
         \nWith no project argument, DMO uses its built-in two-track demo."
    );
}

fn demo_project(sample_rate: u32) -> Result<Project, dmo_core::ProjectError> {
    let mut project = Project::new("First DMO Song", sample_rate, 120.0)?;
    let two_seconds = project.seconds_to_frames(2.0);

    let mut melody = Track::new("Melody");
    melody.gain = 0.35;
    melody.instrument = Instrument::Triangle;
    melody.midi_channel = 1;
    melody.clips.push(Clip {
        name: "Melody Part".into(),
        start_frame: 0,
        length_frames: two_seconds,
        gain: 1.0,
        fade_in_frames: 0,
        fade_out_frames: 0,
        source: ClipSource::Midi {
            notes: [60, 64, 67, 72]
                .into_iter()
                .enumerate()
                .map(|(index, midi_note)| MidiNote {
                    start_frame: u64::try_from(index).unwrap_or(0) * two_seconds / 4,
                    length_frames: two_seconds / 4,
                    midi_note,
                    velocity: 108,
                })
                .collect(),
            amplitude: 0.8,
        },
    });

    let mut bass = Track::new("Bass");
    bass.gain = 0.2;
    bass.pan = -0.15;
    bass.instrument = Instrument::Square;
    bass.midi_channel = 2;
    bass.clips.push(Clip {
        name: "C3".into(),
        start_frame: 0,
        length_frames: two_seconds,
        gain: 1.0,
        fade_in_frames: 0,
        fade_out_frames: 0,
        source: ClipSource::Midi {
            notes: vec![MidiNote {
                start_frame: 0,
                length_frames: two_seconds,
                midi_note: 48,
                velocity: 116,
            }],
            amplitude: 0.8,
        },
    });

    project.tracks.extend([melody, bass]);
    Ok(project)
}
