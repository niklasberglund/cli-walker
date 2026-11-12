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
/// up front, so secrets never exist on disk under the default umask, even briefly; an
/// existing file is tightened too, because the mode only applies on creation.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents)?;
    file.flush()
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
        assert!(store.load().is_empty());

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
}
