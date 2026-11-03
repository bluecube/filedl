use atomic_write_file::AtomicWriteFile;
use serde::{Serialize, de::DeserializeOwned};
use snafu::prelude::*;
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{Instant, timeout_at},
};

/// Minimal time between two writes of the file.
/// A change is written right away, unless the previous write happened less than this ago;
/// changes made during that time are then written together once it passes.
pub const DUMP_COOLDOWN: Duration = Duration::from_secs(5);

/// Error of a modification of the storage.
#[derive(Debug, Snafu)]
pub enum WriteError {
    /// Serializing the storage failed. The modification is applied in memory,
    /// and will be persisted together with the next successful one.
    #[snafu(display("Failed to serialize storage at {location}"))]
    SerializationError {
        source: serde_json::Error,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    /// The storage is shut down. The modification was not applied.
    #[snafu(display("Storage is shut down at {location}"))]
    StorageAlreadyShutDown {
        #[snafu(implicit)]
        location: snafu::Location,
    },
}

/// An in-memory map persisted to a JSON file.
///
/// Every modification serializes the whole map and hands it to a background task owned
/// by the storage, which writes the latest version to the file, at most once per `DUMP_COOLDOWN`.
/// The task never accesses the map itself, so the storage needs no locking of its own.
/// `shutdown()` stops the task and writes any pending changes.
/// Dropping the storage also stops the write task and writes any pending changes, but
/// may be cancelled if the runtime ends early.
pub struct Storage<T> {
    map: HashMap<Arc<str>, T>,

    /// Channel for communicating with the dump task, gets set to None during shutdown.
    snapshot_tx: Option<watch::Sender<Arc<Vec<u8>>>>,
    /// Handle of the dump task, gets set to None during shutdown.
    handle: Option<JoinHandle<io::Result<()>>>,

    /// Buffer for serializing new writes before those are passed to the dump task.
    /// This serves as a double buffer, avoiding most allocations.
    spare_buf: Arc<Vec<u8>>,
}

impl<T: DeserializeOwned + Send + 'static> Storage<T> {
    /// Loads the storage from the file (empty if the file doesn't exist) and starts the dump task.
    /// If path is a symlink, the storage operates on its target, resolved once during construction.
    pub async fn new<P: Into<PathBuf>>(path: P) -> io::Result<Self> {
        let path = path.into();
        let (path, map) = tokio::task::spawn_blocking(move || {
            // AtomicWriteFile replaces a symlink with a regular file, so we write to its target.
            let path: Arc<Path> = match std::fs::canonicalize(&path) {
                Ok(target) => target.into(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => path.into(),
                Err(e) => return Err(e),
            };
            load(&path).map(|map| (path, map))
        })
        .await
        .map_err(io::Error::other)??;

        let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(Vec::new()));
        let handle = tokio::spawn(dump_task(path, snapshot_rx));

        Ok(Storage {
            map,
            snapshot_tx: Some(snapshot_tx),
            handle: Some(handle),
            spare_buf: Arc::new(Vec::new()),
        })
    }
}

impl<T> Storage<T> {
    pub fn get(&self, key: &str) -> Option<&T> {
        self.map.get(key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> Iterator<'_, T> {
        self.map.iter()
    }

    /// Stops the dump task, writing any pending changes to the file first.
    /// Changes made after this are not persisted anymore.
    /// Calling repeatedly returns Ok and does nothing.
    /// Cancel safe. The dump task will be stopped in background if cancelled,
    /// calling `shutdown()` again will wait for the task stopping.
    pub async fn shutdown(&mut self) -> io::Result<()> {
        // We clear the dump task channel, asking it to stop.
        // After this the task cannot accept any more writes.
        self.snapshot_tx = None;

        // Then we await the handle, ensuring the task is stopped.
        // This can be cancelled.
        let result = if let Some(ref mut handle) = self.handle {
            handle.await.map_err(io::Error::other).flatten()
        } else {
            return Ok(());
        };

        // As a final step we clear the handle, marking the task as fully shut down.
        self.handle = None;

        result
    }

    /// Returns a receiver that sees every snapshot sent to the dump task from now on.
    /// Panics after shutdown.
    #[cfg(test)]
    pub(crate) fn subscribe_snapshots(&self) -> watch::Receiver<Arc<Vec<u8>>> {
        self.snapshot_tx.as_ref().unwrap().subscribe()
    }
}

impl<T: Serialize> Storage<T> {
    /// Modifies the object using the closure.
    /// Returns true if the object existed, false otherwise.
    pub fn update(&mut self, key: &str, f: impl FnOnce(&mut T)) -> Result<bool, WriteError> {
        self.modify(|map| {
            let found = map.get_mut(key).map(f).is_some();
            (found, found)
        })
    }

    /// Inserts the object, returning the previous one.
    /// On a serialization error, the previous object is lost.
    pub fn set(&mut self, key: Arc<str>, value: T) -> Result<Option<T>, WriteError> {
        self.modify(|map| (map.insert(key, value), true))
    }

    /// Creates the object if it doesn't exist.
    /// Returns true if it was created successfully, false if it already existed.
    pub fn create(&mut self, key: Arc<str>, value: T) -> Result<bool, WriteError> {
        self.modify(|map| match map.entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => (false, false),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(value);
                (true, true)
            }
        })
    }

    /// Removes the object and returns it.
    /// On a serialization error, the removed object is lost.
    pub fn remove(&mut self, key: &str) -> Result<Option<T>, WriteError> {
        self.modify(|map| {
            let removed = map.remove(key);
            let changed = removed.is_some();
            (removed, changed)
        })
    }

    /// Removes all elements for which the predicate returns `true` and returns them.
    /// On a serialization error, the removed elements are lost.
    ///
    /// Unlike `HashMap::extract_if`, this is not lazy and the predicate can't modify the elements.
    pub fn extract_if<F>(&mut self, mut f: F) -> Result<Vec<(Arc<str>, T)>, WriteError>
    where
        F: FnMut(&Arc<str>, &T) -> bool,
    {
        self.modify(|map| {
            let removed: Vec<_> = map.extract_if(|k, v| f(k, v)).collect();
            let changed = !removed.is_empty();
            (removed, changed)
        })
    }

    /// Applies `f` to the map and sends a snapshot to the dump task if `f` reports a change
    /// (the second element of its result). All modifications go through here.
    ///
    /// Fails with `StorageAlreadyShutDown` without calling `f` once shutdown has started,
    /// so a refused modification leaves the map unchanged.
    fn modify<R>(
        &mut self,
        f: impl FnOnce(&mut HashMap<Arc<str>, T>) -> (R, bool),
    ) -> Result<R, WriteError> {
        let snapshot_tx = self
            .snapshot_tx
            .as_ref()
            .context(StorageAlreadyShutDownSnafu)?;

        let (result, changed) = f(&mut self.map);
        if !changed {
            return Ok(result);
        }

        let buffer = {
            // With high frequency writes to the storage spare_buf might be still
            // held by the dump task, in that case we give up on buffer reuse and
            // allocate a fresh buffer.
            if let Some(buffer) = Arc::get_mut(&mut self.spare_buf) {
                buffer.clear();
                buffer
            } else {
                self.spare_buf = Arc::new(Vec::with_capacity(self.spare_buf.len()));
                Arc::get_mut(&mut self.spare_buf).unwrap()
            }
        };

        // On error the previous snapshot stays in place, the next change will try again.
        serde_json::to_writer(buffer, &self.map).context(SerializationSnafu)?;

        snapshot_tx.send_modify(|old_buffer| std::mem::swap(old_buffer, &mut self.spare_buf));
        Ok(result)
    }
}

fn load<T: DeserializeOwned>(path: &Path) -> io::Result<HashMap<Arc<str>, T>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e),
    };
    Ok(serde_json::from_reader(BufReader::new(file))?)
}

/// Writes the latest snapshot to the file whenever it changes, waiting `DUMP_COOLDOWN`
/// after each write, until the sender is dropped. Then it writes any remaining snapshot
/// right away (also during the cooldown) and returns.
/// Failed writes are logged and retried after the cooldown; only the final write
/// returns its error.
async fn dump_task(
    path: Arc<Path>,
    mut snapshot_rx: watch::Receiver<Arc<Vec<u8>>>,
) -> io::Result<()> {
    log::info!("Starting storage dump task for {}", path.display());

    let mut cooldown_until = Instant::now();
    let mut pending = false;
    let mut result = Ok(());

    // Writes the bytes into the file, updates `result` and returns
    // new value for pending.
    let mut write = async |receiver: &mut watch::Receiver<Arc<Vec<u8>>>| -> bool {
        log::debug!("Writing storage to {}", path.display());

        result = {
            let path = Arc::clone(&path);
            let bytes = Arc::clone(&receiver.borrow_and_update());
            tokio::task::spawn_blocking(move || {
                let mut file = AtomicWriteFile::open(path)?;
                file.write_all(&bytes)?;
                file.commit()
            })
            .await
            .map_err(io::Error::other)
            .flatten()
        };
        if let Err(ref e) = result {
            log::error!("Failed to write storage to {}: {}", path.display(), e);
        }

        result.is_err()
    };

    loop {
        if pending {
            let now = Instant::now();
            if cooldown_until <= now {
                pending = write(&mut snapshot_rx).await;
                cooldown_until = now + DUMP_COOLDOWN;
            } else if let Ok(Err(_)) = timeout_at(cooldown_until, snapshot_rx.changed()).await {
                break;
            }
        } else {
            match snapshot_rx.changed().await {
                Ok(_) => pending = true,
                Err(_) => break,
            }
        }
    }

    if pending {
        write(&mut snapshot_rx).await;
    }

    log::info!("Finished storage dump task for {}", path.display());

    result
}

pub type Iterator<'a, T> = std::collections::hash_map::Iter<'a, Arc<str>, T>;

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::assert;

    async fn fresh() -> (tempfile::TempDir, PathBuf, Storage<String>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("storage.json");
        let s = Storage::new(&path).await.unwrap();
        (dir, path, s)
    }

    fn read_file(path: &Path) -> HashMap<String, String> {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn create_and_get() {
        let (_dir, _path, mut s) = fresh().await;
        assert!(s.create(Arc::from("foo"), "hello".into()).unwrap());
        assert!(s.get("foo") == Some(&"hello".to_string()));
    }

    #[tokio::test]
    async fn create_duplicate_returns_false_and_preserves_original() {
        let (_dir, _path, mut s) = fresh().await;
        assert!(s.create(Arc::from("foo"), "first".into()).unwrap());
        let snapshots = s.subscribe_snapshots();
        assert!(!s.create(Arc::from("foo"), "second".into()).unwrap());
        assert!(s.get("foo") == Some(&"first".to_string()));
        assert!(!snapshots.has_changed().unwrap());
    }

    #[tokio::test]
    async fn set_overwrites() {
        let (_dir, _path, mut s) = fresh().await;
        s.set(Arc::from("foo"), "first".into()).unwrap();
        s.set(Arc::from("foo"), "second".into()).unwrap();
        assert!(s.get("foo") == Some(&"second".to_string()));
    }

    #[tokio::test]
    async fn update_existing_and_missing() {
        let (_dir, _path, mut s) = fresh().await;
        assert!(s.create(Arc::from("foo"), "hello".into()).unwrap());
        let snapshots = s.subscribe_snapshots();

        assert!(!s.update("bar", |v| v.push('!')).unwrap());
        assert!(!snapshots.has_changed().unwrap());

        assert!(s.update("foo", |v| v.push('!')).unwrap());
        assert!(s.get("foo") == Some(&"hello!".to_string()));
        assert!(snapshots.has_changed().unwrap());
    }

    #[tokio::test]
    async fn remove_existing_and_missing() {
        let (_dir, _path, mut s) = fresh().await;
        assert!(s.create(Arc::from("foo"), "hello".into()).unwrap());
        assert!(s.remove("foo").unwrap() == Some("hello".to_string()));
        assert!(s.get("foo") == None);

        let snapshots = s.subscribe_snapshots();
        assert!(s.remove("foo").unwrap() == None);
        assert!(!snapshots.has_changed().unwrap());
    }

    #[tokio::test]
    async fn extract_if_removes_and_returns_entries() {
        let (_dir, _path, mut s) = fresh().await;
        s.create(Arc::from("keep"), "yes".into()).unwrap();
        s.create(Arc::from("drop"), "no".into()).unwrap();

        let removed = s.extract_if(|_, v| v == "no").unwrap();
        assert!(removed == vec![(Arc::from("drop"), "no".to_string())]);
        assert!(s.get("keep") == Some(&"yes".to_string()));
        assert!(s.get("drop") == None);
        assert!(s.len() == 1);
    }

    #[tokio::test]
    async fn extract_if_snapshots_only_when_removed() {
        let (_dir, _path, mut s) = fresh().await;
        s.create(Arc::from("a"), "keep".into()).unwrap();
        s.create(Arc::from("b"), "drop".into()).unwrap();
        let snapshots = s.subscribe_snapshots();

        assert!(s.extract_if(|_, _| false).unwrap().is_empty());
        assert!(!snapshots.has_changed().unwrap());

        s.extract_if(|k, _| k.as_ref() != "a").unwrap();
        assert!(snapshots.has_changed().unwrap());
    }

    /// Long enough for the dump task to write the file, but well within the cooldown.
    const SHORT: Duration = Duration::from_millis(500);

    fn map(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn first_change_is_written_immediately() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("foo"), "bar".into()).unwrap();

        tokio::time::sleep(SHORT).await;
        assert!(read_file(&path) == map(&[("foo", "bar")]));
    }

    #[tokio::test(start_paused = true)]
    async fn changes_during_cooldown_are_written_together_after_it() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("a"), "1".into()).unwrap();
        tokio::time::sleep(SHORT).await;
        s.set(Arc::from("b"), "2".into()).unwrap();
        s.set(Arc::from("c"), "3".into()).unwrap();

        tokio::time::sleep(SHORT).await;
        assert!(read_file(&path) == map(&[("a", "1")]));

        tokio::time::sleep(DUMP_COOLDOWN).await;
        assert!(read_file(&path) == map(&[("a", "1"), ("b", "2"), ("c", "3")]));
    }

    #[tokio::test(start_paused = true)]
    async fn change_after_cooldown_is_written_immediately() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("a"), "1".into()).unwrap();
        tokio::time::sleep(DUMP_COOLDOWN * 2).await;
        s.remove("a").unwrap();

        tokio::time::sleep(SHORT).await;
        assert!(read_file(&path).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_writes_pending_changes() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("foo"), "bar".into()).unwrap();

        s.shutdown().await.unwrap();
        assert!(read_file(&path) == map(&[("foo", "bar")]));

        // Second shutdown is a no-op
        s.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_during_cooldown_writes_immediately() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("a"), "1".into()).unwrap();
        tokio::time::sleep(SHORT).await;
        s.set(Arc::from("b"), "2".into()).unwrap();

        let start = tokio::time::Instant::now();
        s.shutdown().await.unwrap();
        assert!(start.elapsed() < SHORT);
        assert!(read_file(&path) == map(&[("a", "1"), ("b", "2")]));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_during_cooldown_without_changes_returns_immediately() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("a"), "1".into()).unwrap();
        tokio::time::sleep(SHORT).await;

        let start = tokio::time::Instant::now();
        s.shutdown().await.unwrap();
        assert!(start.elapsed() < SHORT);
        assert!(read_file(&path) == map(&[("a", "1")]));
    }

    #[tokio::test(start_paused = true)]
    async fn drop_writes_pending_changes() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("foo"), "bar".into()).unwrap();
        drop(s);

        tokio::time::sleep(SHORT).await;
        assert!(read_file(&path) == map(&[("foo", "bar")]));
    }

    #[tokio::test]
    async fn shutdown_without_changes_does_not_write() {
        let (_dir, path, mut s) = fresh().await;
        s.shutdown().await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn dump_and_reload() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("key1"), "value1".into()).unwrap();
        s.set(Arc::from("key2"), "value2".into()).unwrap();
        s.shutdown().await.unwrap();

        let s: Storage<String> = Storage::new(&path).await.unwrap();
        assert!(s.get("key1") == Some(&"value1".to_string()));
        assert!(s.get("key2") == Some(&"value2".to_string()));
        assert!(s.len() == 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, path, mut s) = fresh().await;
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        s.set(Arc::from("foo"), "bar".into()).unwrap();
        s.shutdown().await.unwrap();

        assert!(read_file(&path) == map(&[("foo", "bar")]));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert!(mode & 0o777 == 0o640);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn load_and_write_go_through_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("storage.json");
        let target = dir.path().join("target.json");
        std::fs::write(&target, r#"{"a":"1"}"#).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let mut s: Storage<String> = Storage::new(&path).await.unwrap();
        assert!(s.get("a") == Some(&"1".to_string()));
        s.set(Arc::from("foo"), "bar".into()).unwrap();
        s.shutdown().await.unwrap();

        assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
        assert!(read_file(&target) == map(&[("a", "1"), ("foo", "bar")]));
    }

    #[tokio::test]
    async fn modifications_after_shutdown_fail_without_changes() {
        let (_dir, _path, mut s) = fresh().await;
        s.set(Arc::from("foo"), "bar".into()).unwrap();
        s.shutdown().await.unwrap();

        assert!(let Err(WriteError::StorageAlreadyShutDown { .. }) = s.set(Arc::from("new"), "x".into()));
        assert!(let Err(WriteError::StorageAlreadyShutDown { .. }) = s.create(Arc::from("new"), "x".into()));
        assert!(let Err(WriteError::StorageAlreadyShutDown { .. }) = s.update("foo", |v| v.push('!')));
        assert!(let Err(WriteError::StorageAlreadyShutDown { .. }) = s.remove("foo"));
        assert!(let Err(WriteError::StorageAlreadyShutDown { .. }) = s.extract_if(|_, _| true));

        assert!(s.len() == 1);
        assert!(s.get("foo") == Some(&"bar".to_string()));
    }

    /// Every write after the first two reuses a buffer that held an older snapshot.
    #[tokio::test(start_paused = true)]
    async fn repeated_writes_contain_only_latest_snapshot() {
        let (_dir, path, mut s) = fresh().await;
        let mut expected = HashMap::new();

        for i in 0..5 {
            let key = format!("key{i}");
            s.set(Arc::from(key.as_str()), i.to_string()).unwrap();
            expected.insert(key, i.to_string());

            tokio::time::sleep(DUMP_COOLDOWN + SHORT).await;
            assert!(read_file(&path) == expected);
        }
    }

    /// Writes fail until the parent directory exists.
    async fn in_missing_dir() -> (tempfile::TempDir, PathBuf, Storage<String>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("storage.json");
        let s = Storage::new(&path).await.unwrap();
        (dir, path, s)
    }

    #[tokio::test(start_paused = true)]
    async fn failed_write_is_retried_after_cooldown() {
        let (_dir, path, mut s) = in_missing_dir().await;
        s.set(Arc::from("foo"), "bar".into()).unwrap();

        tokio::time::sleep(SHORT).await;
        assert!(!path.exists());

        std::fs::create_dir(path.parent().unwrap()).unwrap();
        tokio::time::sleep(DUMP_COOLDOWN).await;
        assert!(read_file(&path) == map(&[("foo", "bar")]));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_returns_error_of_final_write() {
        let (_dir, path, mut s) = in_missing_dir().await;
        s.set(Arc::from("foo"), "bar".into()).unwrap();

        assert!(let Err(_) = s.shutdown().await);
        assert!(!path.exists());

        // The error is only reported once
        s.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_shutdown_can_be_resumed() {
        let (_dir, path, mut s) = fresh().await;
        s.set(Arc::from("foo"), "bar".into()).unwrap();

        // Polls the shutdown once, then drops it while it waits for the dump task.
        tokio::select! {
            biased;
            _ = s.shutdown() => panic!("Shutdown finished without waiting for the dump task"),
            _ = std::future::ready(()) => {}
        }

        s.shutdown().await.unwrap();
        assert!(read_file(&path) == map(&[("foo", "bar")]));
    }
}
