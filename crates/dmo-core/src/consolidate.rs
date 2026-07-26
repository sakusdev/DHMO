use std::{
    collections::HashMap,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use crate::{ClipSource, Project};

#[derive(Debug, Clone, PartialEq)]
pub struct ConsolidatedProject {
    pub project: Project,
    pub copied_files: usize,
}

#[derive(Debug)]
pub enum ConsolidateError {
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for ConsolidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "could not consolidate {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ConsolidateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
        }
    }
}

/// Copies every file-backed audio clip into `media_dir` and returns a project
/// whose audio paths point at the copied media.
///
/// Relative source paths are resolved against the parent directory of
/// `project_path` when possible. Duplicate source paths are copied once and
/// shared by every matching clip in the returned project.
///
/// # Errors
///
/// Returns [`ConsolidateError`] if the media directory cannot be created or a
/// referenced file cannot be copied.
pub fn consolidate_project_media(
    project: &Project,
    project_path: Option<&Path>,
    media_dir: &Path,
) -> Result<ConsolidatedProject, ConsolidateError> {
    create_dir_all(media_dir)?;
    let mut consolidated = project.clone();
    let project_base = project_path.and_then(Path::parent);
    let mut copied = HashMap::<String, String>::new();
    let mut copied_files = 0;

    for track in &mut consolidated.tracks {
        for clip in &mut track.clips {
            let ClipSource::AudioFile { path, .. } = &mut clip.source else {
                continue;
            };
            if let Some(copied_path) = copied.get(path).cloned() {
                *path = copied_path;
                continue;
            }

            let source_path = resolve_source_path(path, project_base);
            let destination = unique_media_path(media_dir, copied.len(), &source_path);
            copy_file(&source_path, &destination)?;
            let destination = destination.to_string_lossy().into_owned();
            copied.insert(path.clone(), destination.clone());
            *path = destination;
            copied_files += 1;
        }
    }

    Ok(ConsolidatedProject {
        project: consolidated,
        copied_files,
    })
}

fn resolve_source_path(path: &str, project_base: Option<&Path>) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else if let Some(base) = project_base {
        base.join(path)
    } else {
        path
    }
}

fn unique_media_path(media_dir: &Path, index: usize, source_path: &Path) -> PathBuf {
    let filename = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(safe_media_filename)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "audio.wav".into());
    media_dir.join(format!("{:03}_{filename}", index + 1))
}

fn safe_media_filename(filename: &str) -> String {
    filename
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn create_dir_all(path: &Path) -> Result<(), ConsolidateError> {
    fs::create_dir_all(path).map_err(|source| ConsolidateError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), ConsolidateError> {
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|source_error| ConsolidateError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, FadeCurve, Track};

    #[test]
    fn consolidation_copies_duplicate_audio_sources_once_and_relinks_clips() {
        let root = std::env::temp_dir().join(format!("dmo-consolidate-{}", std::process::id()));
        let source_dir = root.join("source");
        let output_media = root.join("Bundle").join("Media");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("Lead Vox.wav");
        fs::write(&source, b"not a real wav but enough to copy").unwrap();

        let mut project = Project::new("Bundle", 48_000, 120.0).unwrap();
        let mut track = Track::new("Vocal");
        for start_frame in [0, 100] {
            track.clips.push(Clip {
                name: "Take".into(),
                start_frame,
                length_frames: 10,
                gain: 1.0,
                fade_in_frames: 0,
                fade_out_frames: 0,
                fade_curve: FadeCurve::Linear,
                source: ClipSource::AudioFile {
                    path: source.to_string_lossy().into_owned(),
                    source_offset_frames: 0,
                    source_sample_rate: 48_000,
                    channels: 2,
                    reversed: false,
                },
            });
        }
        project.tracks.push(track);

        let consolidated = consolidate_project_media(&project, None, &output_media).unwrap();

        assert_eq!(consolidated.copied_files, 1);
        let ClipSource::AudioFile { path: first, .. } =
            &consolidated.project.tracks[0].clips[0].source
        else {
            panic!("expected audio file");
        };
        let ClipSource::AudioFile { path: second, .. } =
            &consolidated.project.tracks[0].clips[1].source
        else {
            panic!("expected audio file");
        };
        assert_eq!(first, second);
        assert!(Path::new(first).exists());
        assert!(
            Path::new(first)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("001_Lead_Vox.wav")
        );

        let _ = fs::remove_dir_all(root);
    }
}
