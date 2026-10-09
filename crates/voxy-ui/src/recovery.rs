//! Autosave files and crash recovery: where they live, which are worth
//! offering back, and how a crash is told from a window that is still open.
//!
//! - A **saved** project's autosave sits next to it: `MyProject.voxy.autosave`
//!   (the project path plus `.autosave`).
//! - An **untitled** project's autosave sits in the per-user recovery folder
//!   as `<session>.voxy.autosave`.
//! - Every file is the ordinary `.voxy` envelope, written atomically by
//!   `persist::save_with_ui`; recovery is just loading it.
//! - A clean exit, a save, or choosing "Don't Save" deletes the autosave, so
//!   **an autosave that is still there is a crash** (or a killed process).
//! - Each running session holds an exclusive OS lock on `<session>.lock` in the
//!   recovery folder. The operating system drops the lock when the process
//!   dies for any reason, so an untitled autosave whose lock can be taken is
//!   orphaned, while one whose lock is held belongs to a window that is open
//!   right now and is never offered. (Two windows on the *same saved
//!   project* are not supported yet and are not distinguished.)

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Appended to a project path (or a session id) to name its autosave.
pub const AUTOSAVE_SUFFIX: &str = ".autosave";
const UNTITLED_SUFFIX: &str = ".voxy.autosave";

/// The autosave that belongs to `project`: `<project>.autosave`.
pub fn autosave_path(project: &Path) -> PathBuf {
    let mut name = project.as_os_str().to_owned();
    name.push(AUTOSAVE_SUFFIX);
    PathBuf::from(name)
}

/// "3 minutes ago", for the recovery prompt.
pub fn describe_age(now: SystemTime, then: SystemTime) -> String {
    let secs = now.duration_since(then).map_or(0, |d| d.as_secs());
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..=59 => "less than a minute ago".to_owned(),
        60..=3599 => plural(secs / 60, "minute"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        _ => plural(secs / 86_400, "day"),
    }
}

/// An autosave worth offering back to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The autosave file.
    pub autosave: PathBuf,
    /// When it was written.
    pub modified: SystemTime,
    /// The project it belongs to; `None` for an untitled one.
    pub project: Option<PathBuf>,
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The autosave of `project` if there is one newer than the project file (or
/// the project file is gone). An older autosave is stale leftovers from
/// before a later save and is ignored (and not deleted: it is harmless).
pub fn offer_for_project(project: &Path) -> Option<Offer> {
    let autosave = autosave_path(project);
    let at = modified(&autosave)?;
    match modified(project) {
        Some(saved) if saved >= at => None,
        _ => Some(Offer {
            autosave,
            modified: at,
            project: Some(project.to_path_buf()),
        }),
    }
}

/// Removes an offer's file (and the dead session's lock beside it, if any).
pub fn discard(offer: &Offer) {
    let _ = fs::remove_file(&offer.autosave);
    if let Some(lock) = lock_path_of(&offer.autosave) {
        let _ = fs::remove_file(lock);
    }
}

/// `<dir>/<session>.lock` for `<dir>/<session>.voxy.autosave`.
fn lock_path_of(autosave: &Path) -> Option<PathBuf> {
    let name = autosave.file_name()?.to_str()?;
    let id = name.strip_suffix(UNTITLED_SUFFIX)?;
    Some(autosave.with_file_name(format!("{id}.lock")))
}

/// `true` if no process holds the lock file `lock` (it is missing, or can be
/// taken). Taking it for the check releases it again immediately.
fn lock_is_free(lock: &Path) -> bool {
    match File::options().read(true).write(true).open(lock) {
        Err(e) => e.kind() == io::ErrorKind::NotFound,
        Ok(file) => file.try_lock().is_ok(),
    }
}

/// The untitled autosaves in `dir` whose sessions are gone, newest first.
pub fn orphans(dir: &Path) -> Vec<Offer> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Offer> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(UNTITLED_SUFFIX))
        })
        .filter(|p| lock_path_of(p).is_none_or(|lock| lock_is_free(&lock)))
        .filter_map(|p| {
            Some(Offer {
                modified: modified(&p)?,
                autosave: p,
                project: None,
            })
        })
        .collect();
    found.sort_by_key(|o| std::cmp::Reverse(o.modified));
    found
}

/// One running voXY window's claim on the recovery folder. The lock is taken
/// the first time an untitled autosave is needed and held until
/// [`Session::finish`] or the process ends.
#[derive(Debug)]
pub struct Session {
    dir: PathBuf,
    id: String,
    lock: Option<File>,
}

impl Session {
    pub fn new(dir: PathBuf) -> Self {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Session {
            dir,
            id: format!("{:x}-{nanos:x}", std::process::id()),
            lock: None,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where this session's untitled project autosaves, creating the folder
    /// and taking the session lock on first use.
    pub fn untitled_path(&mut self) -> io::Result<PathBuf> {
        if self.lock.is_none() {
            fs::create_dir_all(&self.dir)?;
            let file = File::options()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(self.dir.join(format!("{}.lock", self.id)))?;
            file.try_lock().map_err(|e| match e {
                fs::TryLockError::Error(e) => e,
                fs::TryLockError::WouldBlock => {
                    io::Error::new(io::ErrorKind::WouldBlock, "session lock is held")
                }
            })?;
            self.lock = Some(file);
        }
        Ok(self.dir.join(format!("{}{UNTITLED_SUFFIX}", self.id)))
    }

    /// Deletes this session's untitled autosave, if any (a clean exit, a save
    /// under a name, or discarding the project).
    pub fn forget_untitled(&self) {
        let _ = fs::remove_file(self.dir.join(format!("{}{UNTITLED_SUFFIX}", self.id)));
    }

    /// Ends the session: no autosave of it survives and the lock is released
    /// and removed.
    pub fn finish(&mut self) {
        self.forget_untitled();
        if self.lock.take().is_some() {
            let _ = fs::remove_file(self.dir.join(format!("{}.lock", self.id)));
        }
    }
}

impl Drop for Session {
    /// A session that is dropped without [`Session::finish`] (a crash never
    /// gets here) leaves its autosave behind on purpose.
    fn drop(&mut self) {
        // Closing the file releases the lock.
        self.lock.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, age_secs: u64) {
        fs::write(path, "{}").unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn autosave_sits_next_to_the_project() {
        assert_eq!(
            autosave_path(Path::new("/a/b/My Project.voxy")),
            PathBuf::from("/a/b/My Project.voxy.autosave")
        );
    }

    #[test]
    fn only_an_autosave_newer_than_the_project_is_offered() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("P.voxy");
        // No autosave at all.
        touch(&project, 100);
        assert_eq!(offer_for_project(&project), None);
        // Older than the saved project: leftovers, ignored.
        touch(&autosave_path(&project), 500);
        assert_eq!(offer_for_project(&project), None);
        // Newer: offered.
        touch(&autosave_path(&project), 10);
        let offer = offer_for_project(&project).unwrap();
        assert_eq!(offer.autosave, autosave_path(&project));
        assert_eq!(offer.project.as_deref(), Some(project.as_path()));
        // The project file is gone but its autosave remains: offered.
        fs::remove_file(&project).unwrap();
        assert!(offer_for_project(&project).is_some());
        discard(&offer);
        assert_eq!(offer_for_project(&project), None);
    }

    #[test]
    fn a_session_holds_its_lock_and_is_not_an_orphan_while_alive() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::new(dir.path().join("recovery"));
        let path = session.untitled_path().unwrap();
        touch(&path, 1);
        // The lock is held, so another window sees nothing to recover.
        assert_eq!(orphans(session.dir()).len(), 0);
        // The lock does not move: asking again is cheap and stable.
        assert_eq!(session.untitled_path().unwrap(), path);
        // Clean exit removes the autosave and the lock.
        session.finish();
        assert!(!path.exists());
        assert_eq!(orphans(session.dir()).len(), 0);
        assert_eq!(fs::read_dir(session.dir()).unwrap().count(), 0);
    }

    #[test]
    fn a_crashed_session_leaves_an_orphan_that_can_be_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let rec = dir.path().join("recovery");
        let mut crashed = Session::new(rec.clone());
        let path = crashed.untitled_path().unwrap();
        touch(&path, 5);
        // The process dies: the OS releases the lock, the files stay. (A
        // dropped session without `finish` models that.)
        drop(crashed);
        let found = orphans(&rec);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].autosave, path);
        assert_eq!(found[0].project, None);
        discard(&found[0]);
        assert_eq!(orphans(&rec).len(), 0);
        assert_eq!(fs::read_dir(&rec).unwrap().count(), 0, "lock removed too");
    }

    #[test]
    fn an_autosave_with_no_lock_at_all_is_an_orphan_and_newest_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("1-a.voxy.autosave"), 300);
        touch(&dir.path().join("2-b.voxy.autosave"), 20);
        touch(&dir.path().join("notes.txt"), 1);
        touch(&dir.path().join("3-c.voxy"), 1);
        let found = orphans(dir.path());
        let names: Vec<_> = found
            .iter()
            .map(|o| o.autosave.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["2-b.voxy.autosave", "1-a.voxy.autosave"]);
    }

    #[test]
    fn two_sessions_never_share_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Session::new(dir.path().to_path_buf());
        let mut b = Session::new(dir.path().to_path_buf());
        assert_ne!(a.untitled_path().unwrap(), b.untitled_path().unwrap());
        touch(&a.untitled_path().unwrap(), 1);
        // `b` is alive too, and neither shows up as an orphan of the other.
        assert_eq!(orphans(dir.path()).len(), 0);
        a.finish();
        b.finish();
    }

    #[test]
    fn ages_are_described_in_plain_words() {
        let now = SystemTime::now();
        let ago = |s: u64| describe_age(now, now - Duration::from_secs(s));
        assert_eq!(ago(5), "less than a minute ago");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(150), "2 minutes ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(7 * 3600), "7 hours ago");
        assert_eq!(ago(86_400 * 3), "3 days ago");
        // A clock that went backwards is not an error.
        assert_eq!(
            describe_age(now - Duration::from_secs(10), now),
            "less than a minute ago"
        );
    }

    #[test]
    fn a_missing_folder_has_no_orphans() {
        assert_eq!(orphans(Path::new("/definitely/not/here")).len(), 0);
    }
}
