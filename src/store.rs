//! Where CLI-Walker keeps its files, and the append-only session log.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::SessionRecord;

/// The data directory, `~/.cli-walker` unless demo mode points it somewhere disposable.
#[derive(Debug, Clone)]
pub struct DataDir(PathBuf);

impl DataDir {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// `~/.cli-walker` (`%USERPROFILE%\.cli-walker` on Windows), shared with the Kotlin version.
    pub fn default_location() -> Result<Self> {
        let home = directories::BaseDirs::new()
            .context("cannot find your home directory")?
            .home_dir()
            .to_path_buf();
        Ok(Self(home.join(".cli-walker")))
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    pub fn ensure(&self) -> io::Result<()> {
        create_private_dir(&self.0)
    }

    pub fn sessions(&self) -> SessionStore {
        SessionStore {
            path: self.file("sessions.jsonl"),
            dir: self.clone(),
        }
    }
}

/// Creates a directory readable only by its owner, like the Kotlin version's `mkdir(dir, 0700)`.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// Writes `contents` to a file only its owner can read. The file is created with mode 0600
/// up front, so secrets never exist on disk under the default umask, even briefly.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_replacing(path, contents, true)
}

/// Replaces a file's contents. The new contents go to a temporary file beside it that then
/// takes its place, so a write that fails or is cut short leaves the old file intact rather
/// than empty or half-written.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_replacing(path, contents, false)
}

fn write_replacing(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(".{}.tmp", std::process::id()));
    let temp = path.with_file_name(temp_name);

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let written = options.open(&temp).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()
    });
    match written.and_then(|()| fs::rename(&temp, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// `sessions.jsonl`: one JSON record per line, only ever appended to.
#[derive(Debug, Clone)]
pub struct SessionStore {
    path: PathBuf,
    dir: DataDir,
}

impl SessionStore {
    /// Every readable record, oldest first. Malformed lines are skipped, not fatal.
    pub fn load(&self) -> Vec<SessionRecord> {
        let Ok(file) = File::open(&self.path) else {
            return Vec::new();
        };
        BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line.trim()).ok())
            .collect()
    }

    pub fn append(&self, record: &SessionRecord) -> Result<()> {
        self.dir.ensure()?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("cannot open {}", self.path.display()))?;
        let mut line = serde_json::to_string(record)?;
        line.push('\n');
        file.write_all(line.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;

    fn record(start: i64) -> SessionRecord {
        SessionRecord {
            started_at: Timestamp::from_second(start).unwrap(),
            ended_at: Timestamp::from_second(start + 600).unwrap(),
            duration_seconds: 600,
            distance_km: 0.5,
            steps: 770,
            calories: 42,
            avg_speed_kmh: 3.0,
            max_speed_kmh: 3.5,
        }
    }

    #[test]
    fn appends_and_reloads_records() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path().join("nested"));
        let store = dir.sessions();
        assert_eq!(store.load(), []);

        store.append(&record(1_700_000_000)).unwrap();
        store.append(&record(1_700_100_000)).unwrap();
        assert_eq!(
            store.load(),
            vec![record(1_700_000_000), record(1_700_100_000)]
        );
    }

    #[test]
    fn skips_malformed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        let store = dir.sessions();
        store.append(&record(1_700_000_000)).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.file("sessions.jsonl"))
            .unwrap();
        writeln!(file, "not json\n\n{{\"half\": ").unwrap();
        store.append(&record(1_700_100_000)).unwrap();
        assert_eq!(store.load().len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only_even_if_they_existed() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret.json");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        write_private(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn replacing_a_file_leaves_no_temporary_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log.json");
        fs::write(&path, "old").unwrap();

        write_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        let names: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["log.json"]);
    }

    #[test]
    fn a_failed_replacement_keeps_the_old_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log.json");
        fs::write(&path, "old").unwrap();
        // A directory where the temporary file would go makes the write fail.
        fs::create_dir(
            tmp.path()
                .join(format!(".log.json.{}.tmp", std::process::id())),
        )
        .unwrap();

        assert!(write_atomic(&path, b"new").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
    }
}
