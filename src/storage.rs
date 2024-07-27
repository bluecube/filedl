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
        self.dirty = true;
        self.map.insert(key, value)
    }

    pub fn remove(&mut self, key: &str) -> Option<T> {
        self.dirty = true;
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
}

impl<T: Serialize + DeserializeOwned> Drop for Storage<T> {
    fn drop(&mut self) {
        if self.dirty {
            self.dump().expect("Dumping Storage failed");
        }
    }
}

pub type Iterator<'a, T> = std::collections::hash_map::Iter<'a, Arc<str>, T>;
