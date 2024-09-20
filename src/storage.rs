use serde::{de::DeserializeOwned, Serialize};
use std::{collections::HashMap, fs::File, io::BufReader, path::PathBuf, sync::Arc};

#[derive(Debug)]
pub struct Storage<T: Serialize + DeserializeOwned> {
    map: HashMap<Arc<str>, T>,
    file: PathBuf,
    dirty: bool,
}

impl<T: Serialize + DeserializeOwned> Storage<T> {
    pub fn new<P: Into<PathBuf>>(path: P) -> std::io::Result<Self> {
        let mut s = Storage {
            map: Default::default(),
            file: path.into(),
            dirty: false,
        };

        s.reload()?;
        Ok(s)
    }

    pub fn get(&self, key: &str) -> Option<&T> {
        self.map.get(key)
    }

    pub fn set(&mut self, key: Arc<str>, value: T) -> Option<T> {
        self.make_dirty();
        self.map.insert(key, value)
    }

    /// Creates the object if it doesn't exist.
    /// Returns true if it was created successfully, false if it already existed.
    pub fn create(&mut self, key: Arc<str>, value: T) -> bool {
        match self.map.entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => return false,
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(value);
            }
        }

        self.make_dirty();
        true
    }

    pub fn remove(&mut self, key: &str) -> Option<T> {
        self.make_dirty();
        self.map.remove(key)
    }

    /// Immediately (and unconditionally) dump the content to the file
    pub fn dump(&mut self) -> std::io::Result<()> {
        let f = File::create(&self.file)?;
        serde_json::to_writer(f, &self.map)?;
        self.dirty = false;
        Ok(())
    }

    pub fn reload(&mut self) -> std::io::Result<()> {
        let file = match File::open(&self.file) {
            Ok(file) => file,
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => return Ok(()),
                _ => return Err(e),
            },
        };
        let reader = BufReader::new(file);

        // Read the JSON contents of the file as an instance of `User`.
        self.map = serde_json::from_reader(reader)?;
        self.dirty = false;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn iter(&self) -> Iterator<T> {
        self.map.iter()
    }

    fn make_dirty(&mut self) {
        self.dirty = true;
    }
}

impl<T: Serialize + DeserializeOwned> Drop for Storage<T> {
    fn drop(&mut self) {
        if self.dirty {
            self.dump().expect("Dumping Storage failed");
        }
    }
}

pub type Iterator<'a, T> = std::collections::hash_map::Iter<'a, Arc<str>, T>;
