//! Platform file and confirmation dialogs.

#[cfg(not(target_os = "android"))]
pub use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

#[cfg(target_os = "android")]
mod android {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::SystemTime,
    };

    /// App-storage replacement for a native file dialog on Android.
    #[derive(Default)]
    pub struct FileDialog {
        extensions: Vec<String>,
        file_name: Option<String>,
        title: Option<String>,
    }

    impl FileDialog {
        #[must_use]
        pub fn new() -> Self {
            Self::default()
        }

        #[must_use]
        pub fn add_filter(mut self, _name: &str, extensions: &[&str]) -> Self {
            self.extensions = extensions
                .iter()
                .map(|extension| extension.to_ascii_lowercase())
                .collect();
            self
        }

        #[must_use]
        pub fn set_file_name(mut self, file_name: impl Into<String>) -> Self {
            self.file_name = Some(file_name.into());
            self
        }

        #[must_use]
        pub fn set_title(mut self, title: impl Into<String>) -> Self {
            self.title = Some(title.into());
            self
        }

        #[must_use]
        pub fn pick_file(self) -> Option<PathBuf> {
            newest_matching_file(&self.open_directory(), &self.extensions)
        }

        #[must_use]
        pub fn save_file(self) -> Option<PathBuf> {
            let directory = self.save_directory();
            fs::create_dir_all(&directory).ok()?;
            let fallback = self.extensions.first().map_or_else(
                || "untitled".to_owned(),
                |extension| format!("untitled.{extension}"),
            );
            let requested = self.file_name.as_deref().unwrap_or(&fallback);
            let safe_name = Path::new(requested)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(&fallback);
            Some(directory.join(safe_name))
        }

        #[must_use]
        pub fn pick_folder(self) -> Option<PathBuf> {
            let title = self
                .title
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let directory = if title.contains("stem") {
                PathBuf::from("Exports").join("Stems")
            } else if title.contains("consolidate") {
                PathBuf::from("Projects").join("Consolidated")
            } else {
                PathBuf::from("Exports")
            };
            fs::create_dir_all(&directory).ok()?;
            Some(directory)
        }

        fn open_directory(&self) -> PathBuf {
            if self.extensions.iter().any(|extension| extension == "dmo") {
                PathBuf::from("Projects")
            } else {
                PathBuf::from("Imports")
            }
        }

        fn save_directory(&self) -> PathBuf {
            if self.extensions.iter().any(|extension| extension == "dmo") {
                PathBuf::from("Projects")
            } else {
                PathBuf::from("Exports")
            }
        }
    }

    fn newest_matching_file(directory: &Path, extensions: &[String]) -> Option<PathBuf> {
        let mut files = fs::read_dir(directory)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let extension = path.extension()?.to_str()?.to_ascii_lowercase();
                (extensions.is_empty() || extensions.contains(&extension)).then(|| {
                    let modified = entry
                        .metadata()
                        .and_then(|metadata| metadata.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    (modified, path)
                })
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| right.0.cmp(&left.0));
        files.into_iter().next().map(|(_, path)| path)
    }

    #[derive(Clone, Copy)]
    pub enum MessageLevel {
        Warning,
    }

    #[derive(Clone, Copy)]
    pub enum MessageButtons {
        YesNo,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum MessageDialogResult {
        Yes,
        No,
    }

    #[derive(Default)]
    pub struct MessageDialog;

    impl MessageDialog {
        #[must_use]
        pub fn new() -> Self {
            Self
        }

        #[must_use]
        pub const fn set_level(self, _level: MessageLevel) -> Self {
            self
        }

        #[must_use]
        pub fn set_title(self, _title: impl Into<String>) -> Self {
            self
        }

        #[must_use]
        pub fn set_description(self, _description: impl Into<String>) -> Self {
            self
        }

        #[must_use]
        pub const fn set_buttons(self, _buttons: MessageButtons) -> Self {
            self
        }

        #[must_use]
        pub const fn show(self) -> MessageDialogResult {
            // Do not discard unsaved mobile work without an explicit in-app
            // confirmation surface. Saving clears the dirty flag and lets the
            // requested New/Open action proceed on the next tap.
            MessageDialogResult::No
        }
    }
}

#[cfg(target_os = "android")]
pub use android::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
