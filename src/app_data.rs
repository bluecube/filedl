use crate::config::Config;
use crate::storage::Storage;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::{fs, sync::{RwLock, RwLockReadGuard}, task::spawn_blocking};
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ObjectOwnership {
    Owned,
    Linked(PathBuf),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Object {
    pub ownership: ObjectOwnership,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub expires: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub unlisted_key: Option<Arc<str>>,
}

#[derive(Debug)]
pub struct ObjectIterGuard<'a> {
    guard: RwLockReadGuard<'a, Storage<Object>>,
}

impl<'a> IntoIterator for &'a ObjectIterGuard<'a> {
    type Item = (&'a Arc<str>, &'a Object);
    type IntoIter = crate::storage::Iterator<'a, Object>;

    fn into_iter(self) -> Self::IntoIter {
        self.guard.iter()
    }
}

#[derive(Debug)]
pub enum ResolvedObject {
    File(PathBuf),
    Directory(Vec<DirListingItem>),
}

#[derive(Debug)]
pub struct DirListingItem {
    pub name: String,
    pub link: String,
}

impl ResolvedObject {
    pub fn with_directory(path: &Path, url_base: Option<&str>) -> Result<Self, ObjectResolutionError> {
        Ok(ResolvedObject::Directory(
             std::fs::read_dir(path)?
                .filter_map(|entry| -> Option<Result<DirListingItem, ObjectResolutionError>> {
                    // Pass through errors in ReadDir::next(), filter out files that have invalid UTF-8.
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(e) => return Some(Err(e.into())),
                    };
                    let name = entry.file_name().into_string().ok()?;
                    let link = match url_base {
                        Some(url_base) => format!("{url_base}/{name}"),
                        None => name.clone(),
                    };
                    Some(Ok(DirListingItem {
                        name: name,
                        link
                    }))
                })
                .collect::<Result<Vec<DirListingItem>, ObjectResolutionError>>()?
        ))
    }
}

#[derive(Debug, Error)]
pub enum ObjectResolutionError {
    #[error("Object not found")]
    ObjectNotFound,
    #[error("Object exists, but is unlisted")]
    Unlisted,
    #[error("IO error")]
    IOError {
        #[from]
        #[source]
        source: std::io::Error
    },
}

pub struct AppData {
    config: Config,
    objects: RwLock<Storage<Object>>,
}

impl AppData {
    pub fn with_config(config: Config) -> anyhow::Result<Self> {
        let path = config.data_path.join("metadata.json");
        dbg!(&path);
        let objects = RwLock::new(Storage::new(path)?);
        dbg!(&objects);
        Ok(AppData { config, objects })
    }

    fn get_owned_object_path(&self, object_id: &str) -> PathBuf {
        let mut path = self.config.data_path.join("owned_data");
        path.push(object_id);
        path
    }

    fn get_linked_object_path(&self, link_path: &Path) -> PathBuf {
        self.config.linked_objects_root.join(link_path)
    }

    pub async fn resolve_object(
        &self,
        object_id: &str,
        subobject_path: Option<&str>,
        key: Option<&str>,
    ) -> Result<ResolvedObject, ObjectResolutionError> {
        dbg!(&object_id);
        let obj = self.object_from_id(object_id).await?;
        if obj
            .unlisted_key
            .as_ref()
            .is_some_and(|expected_key| key != Some(expected_key))
        {
            // Someone is snooping around for unlisted objects
            return Err(ObjectResolutionError::Unlisted);
        }

        // TODO: Verify that subobject path is not weird
        // TODO: Handle expiry?

        let mut path: PathBuf = match &obj.ownership {
            ObjectOwnership::Owned => self.get_owned_object_path(object_id),
            ObjectOwnership::Linked(path) => self.get_linked_object_path(path),
        };
        if let Some(subobject_path) = subobject_path {
            path.push(subobject_path);
        }
        dbg!(&path);

        let metadata = fs::metadata(&path).await?;

        if metadata.is_dir() {
            //spawn_blocking(move || ResolvedObject::with_directory(&path, subobject_path)).await.unwrap()
            ResolvedObject::with_directory(&path, subobject_path)
        } else {
            Ok(ResolvedObject::File(path))
        }
    }

    async fn object_from_id(&self, id: &str) -> Result<Object, ObjectResolutionError> {
        self.objects.read().await.get(id).ok_or(ObjectResolutionError::ObjectNotFound).cloned()
    }

    pub async fn iter_objects<'a>(&'a self) -> ObjectIterGuard<'a> {
        ObjectIterGuard {
            guard: self.objects.read().await,
        }
    }
}
