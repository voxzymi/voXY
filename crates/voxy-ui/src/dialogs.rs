//! File pickers behind a trait, so the editor logic runs without a window
//! and tests can answer "cancel" or choose files.

use std::fmt::Debug;
use std::path::PathBuf;

use voxy_core::persist;

/// File extensions voXY imports as images.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];
/// File extensions voXY imports as LUTs (for the LUT effect).
pub const LUT_EXTENSIONS: &[&str] = &["cube"];
/// File extensions voXY imports as video.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "ts", "mts", "mxf",
];

/// Every question the editor asks through a file dialog. `None` or an empty
/// list means the user cancelled, and the caller must change nothing.
pub trait Dialogs: Debug + Send {
    /// Any number of image and video files to import.
    fn pick_media(&self) -> Vec<PathBuf>;
    fn pick_image(&self) -> Option<PathBuf>;
    fn pick_video(&self) -> Option<PathBuf>;
    /// One image or video file to relink to.
    fn pick_relink(&self) -> Option<PathBuf>;
    /// One LUT file, for the LUT effect's "Import LUT…".
    fn pick_lut(&self) -> Option<PathBuf> {
        None
    }
    fn open_project(&self) -> Option<PathBuf>;
    fn save_project(&self, file_name: &str) -> Option<PathBuf>;
}

/// The operating system's dialogs.
#[derive(Debug, Default)]
pub struct NativeDialogs;

fn project_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("voXY project", &[persist::PROJECT_EXTENSION])
}

fn media_extensions() -> Vec<&'static str> {
    IMAGE_EXTENSIONS
        .iter()
        .chain(VIDEO_EXTENSIONS)
        .chain(LUT_EXTENSIONS)
        .copied()
        .collect()
}

impl Dialogs for NativeDialogs {
    fn pick_media(&self) -> Vec<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Import")
            .add_filter("Images, video and LUTs", &media_extensions())
            .add_filter("Images", IMAGE_EXTENSIONS)
            .add_filter("Video", VIDEO_EXTENSIONS)
            .add_filter("LUTs", LUT_EXTENSIONS)
            .pick_files()
            .unwrap_or_default()
    }

    fn pick_image(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Import Image")
            .add_filter("Images", IMAGE_EXTENSIONS)
            .pick_file()
    }

    fn pick_video(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Import Video")
            .add_filter("Video", VIDEO_EXTENSIONS)
            .pick_file()
    }

    fn pick_relink(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Relink")
            .add_filter("Images, video and LUTs", &media_extensions())
            .pick_file()
    }

    fn pick_lut(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Import LUT")
            .add_filter("LUTs", LUT_EXTENSIONS)
            .pick_file()
    }

    fn open_project(&self) -> Option<PathBuf> {
        project_dialog().pick_file()
    }

    fn save_project(&self, file_name: &str) -> Option<PathBuf> {
        project_dialog().set_file_name(file_name).save_file()
    }
}

/// Answers every dialog with "cancel". Used where no window exists.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct NoDialogs;

#[cfg(test)]
impl Dialogs for NoDialogs {
    fn pick_media(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn pick_image(&self) -> Option<PathBuf> {
        None
    }
    fn pick_video(&self) -> Option<PathBuf> {
        None
    }
    fn pick_relink(&self) -> Option<PathBuf> {
        None
    }
    fn open_project(&self) -> Option<PathBuf> {
        None
    }
    fn save_project(&self, _file_name: &str) -> Option<PathBuf> {
        None
    }
}
