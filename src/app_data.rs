use crate::config::Config;
use crate::storage::Storage;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;
use relative_path::RelativePathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::{fs, sync::RwLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ObjectOwnership {
    Owned,
    Linked(RelativePathBuf),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Object {
    pub ownership: ObjectOwnership,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlisted_key: Option<Arc<str>>,
}

#[derive(Debug)]
pub enum ResolvedObject {
    File(PathBuf),
    Directory(Vec<DirListingItem>),
}

#[derive(Debug)]
pub struct DirListingItem {
    pub name: Arc<str>,
    pub link: String,
    pub is_directory: bool,
    pub file_size: u64,
    pub modified: Option<DateTime<Utc>>,
}

impl DirListingItem {
    /// Create the dir listing item from directory entry.
    /// If the filename contains non-unicode characters, returns Ok(None).
    async fn with_dir_entry(
        entry: fs::DirEntry,
        directory_base_url: &str,
    ) -> Result<Option<Self>, std::io::Error> {
        let Ok(name) = entry.file_name().into_string() else {
            return Ok(None);
        };
        let link = format!("{directory_base_url}/{name}");
        Ok(Some(Self::with_metadata(
            name.into(),
            link,
            entry.metadata().await?,
        )))
    }

    fn with_metadata(name: Arc<str>, link: String, metadata: std::fs::Metadata) -> Self {
        DirListingItem {
            name,
            link,
            is_directory: metadata.is_dir(),
            file_size: metadata.len(),
            modified: metadata.modified().ok().map(Into::into),
        }
    }
}

impl ResolvedObject {
    pub async fn with_directory(
        path: &Path,
        directory_base_url: &str,
    ) -> Result<Self, ObjectResolutionError> {
        let mut result = Vec::new();

        let mut dir = fs::read_dir(path).await?;
        while let Some(entry) = dir.next_entry().await? {
            if let Some(item) = DirListingItem::with_dir_entry(entry, directory_base_url).await? {
                result.push(item);
            }
        }

        Ok(ResolvedObject::Directory(result))
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
        source: std::io::Error,
    },
}

pub struct AppData {
    config: Config,
    objects: RwLock<Storage<Object>>,
}

impl AppData {
    pub fn with_config(config: Config) -> anyhow::Result<Self> {
        let path = config.data_path.join("metadata.json");
        let objects = RwLock::new(Storage::new(path)?);
        Ok(AppData { config, objects })
    }

    pub fn get_download_base_url(&self) -> &str {
        &self.config.download_url
    }

    pub fn get_app_name(&self) -> &str {
        &self.config.app_name
    }

    fn get_object_path(&self, object_id: &str, obj: &Object) -> PathBuf {
        match &obj.ownership {
            ObjectOwnership::Owned => {
                let mut path = self.config.data_path.join("owned_data");
                path.push(object_id);
                path
            }
            ObjectOwnership::Linked(link_path) => link_path.to_path(&self.config.linked_objects_root),
        }
    }

    pub async fn resolve_object(
        &self,
        path: &str,
        key: Option<&str>,
    ) -> Result<ResolvedObject, ObjectResolutionError> {
        let (object_id, subobject_path) = match path.split_once('/') {
            Some((object_id, subobject_path)) => (object_id, Some(subobject_path)),
            None => (path, None),
        };

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

        let mut object_fs_path = self.get_object_path(object_id, &obj);
        if let Some(subobject_path) = subobject_path {
            object_fs_path.push(subobject_path);
        }

        let metadata = fs::metadata(&object_fs_path).await?;

        if metadata.is_dir() {
            let base_url = format!("{}/{}", self.config.download_url, path);
            ResolvedObject::with_directory(&object_fs_path, &base_url).await
        } else {
            Ok(ResolvedObject::File(object_fs_path))
        }
    }

    async fn object_from_id(&self, id: &str) -> Result<Object, ObjectResolutionError> {
        self.objects
            .read()
            .await
            .get(id)
            .ok_or(ObjectResolutionError::ObjectNotFound)
            .cloned()
    }

    pub async fn list_objects(&self) -> Result<Vec<DirListingItem>, ObjectResolutionError> {
        let mut result = Vec::new();

        for (key, obj) in self.objects.read().await.iter() {
            let metadata = fs::metadata(self.get_object_path(key, obj)).await?;
            result.push(DirListingItem::with_metadata(
                Arc::clone(key),
                format!("{}/{}", self.config.download_url, key),
                metadata,
            ));
        }

        Ok(result)
    }
}
