use crate::config::Config;
use crate::storage::Storage;
use crate::thumbnails::{is_thumbnailable, CacheStats, CachedThumbnails};
use actix_web::web::Bytes;
use chrono::prelude::*;
use chrono_tz::Tz;
use relative_path::RelativePathBuf;
use serde::{Deserialize, Serialize};
use std::fs::Metadata;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
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

#[derive(Clone, Debug)]
pub enum ItemType {
    Directory,
    Image,
    /// File of other/unknown type
    File,
}

impl ItemType {
    pub fn new(path: &Path, metadata: &Metadata) -> Self {
        if is_thumbnailable(path) {
            ItemType::Image
        } else if metadata.is_dir() {
            ItemType::Directory
        } else {
            ItemType::File
        }
    }

    pub fn is_directory(&self) -> bool {
        matches!(self, ItemType::Directory)
    }

    pub fn is_thumbnailable(&self) -> bool {
        matches!(self, ItemType::Image)
    }
}

impl std::fmt::Display for ItemType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", format!("{:?}", self).to_lowercase())
    }
}

/// Describes a source file for cache busting
#[derive(Hash, Debug, PartialEq, Eq)]
struct CacheSourceKey<'a> {
    path: &'a Path,
    size: u64,
    modtime: Option<SystemTime>,
}

impl<'a> CacheSourceKey<'a> {
    fn with_metadata(path: &'a Path, metadata: &Metadata) -> CacheSourceKey<'a> {
        CacheSourceKey {
            path,
            size: metadata.len(),
            modtime: metadata.modified().ok(),
        }
    }

    fn hash_string(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{:X}", hasher.finish())
    }
}

fn get_source_hash(path: &Path, metadata: &Metadata) -> Option<String> {
    if metadata.is_dir() {
        None
    } else {
        Some(CacheSourceKey::with_metadata(path, metadata).hash_string())
    }
}

#[derive(Debug)]
pub struct DirListingItem {
    pub name: Arc<str>,
    pub link: String,
    pub item_type: ItemType,
    pub file_size: u64,
    pub modified: Option<DateTime<Utc>>,
    pub source_hash: Option<String>,
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
            &entry.path(),
            name.into(),
            link,
            &entry.metadata().await?,
        )))
    }

    fn with_metadata(path: &Path, name: Arc<str>, link: String, metadata: &Metadata) -> Self {
        let item_type = ItemType::new(path, metadata);
        DirListingItem {
            name,
            link,
            item_type,
            file_size: metadata.len(),
            modified: metadata.modified().ok().map(Into::into),
            source_hash: get_source_hash(path, metadata),
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
    thumbnails: CachedThumbnails,
}

impl AppData {
    pub fn with_config(config: Config) -> anyhow::Result<Self> {
        let path = config.data_path.join("metadata.json");
        let objects = RwLock::new(Storage::new(path)?);
        let thumbnail_cache_size = config.thumbnail_cache_size;
        Ok(AppData {
            config,
            objects,
            thumbnails: CachedThumbnails::new(thumbnail_cache_size),
        })
    }

    pub fn get_download_base_url(&self) -> &str {
        &self.config.download_url
    }

    pub fn get_app_name(&self) -> &str {
        &self.config.app_name
    }

    pub fn get_display_timezone(&self) -> &Tz {
        &self.config.display_timezone
    }

    pub async fn get_thumbnail(
        &self,
        file: PathBuf,
        size: (u32, u32),
    ) -> anyhow::Result<(Bytes, String)> {
        self.thumbnails.get(file, size).await
    }

    pub async fn get_thumbnail_cache_stats(&self) -> CacheStats {
        self.thumbnails.cache_stats().await
    }

    fn get_object_path(&self, object_id: &str, obj: &Object) -> PathBuf {
        match &obj.ownership {
            ObjectOwnership::Owned => {
                let mut path = self.config.data_path.join("owned_data");
                path.push(object_id);
                path
            }
            ObjectOwnership::Linked(link_path) => {
                link_path.to_path(&self.config.linked_objects_root)
            }
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
            let path = self.get_object_path(key, obj);
            let metadata = fs::metadata(&path).await?;
            result.push(DirListingItem::with_metadata(
                &path,
                Arc::clone(key),
                format!("{}/{}", self.config.download_url, key),
                &metadata,
            ));
        }

        Ok(result)
    }
}
