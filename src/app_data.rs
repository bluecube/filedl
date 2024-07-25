use crate::config::Config;
use crate::error::{FiledlError, Result};
use crate::storage::Storage;
use crate::thumbnails::{is_thumbnailable, CacheStats, CachedThumbnails};
use actix_web::web::Bytes;
use chrono::prelude::*;
use chrono_tz::Tz;
use rand::{thread_rng, RngCore};
use relative_path::RelativePathBuf;
use serde::{Deserialize, Serialize};
use std::fs::Metadata;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLockReadGuard;
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
pub struct ResolvedObject<'a> {
    path: PathBuf,
    object: RwLockReadGuard<'a, Object>,
    metadata: Metadata,
    thumbnails: &'a CachedThumbnails,
}

impl<'a> ResolvedObject<'a> {
    async fn new(
        path: PathBuf,
        object: RwLockReadGuard<'a, Object>,
        thumbnails: &'a CachedThumbnails,
    ) -> Result<Self> {
        let metadata = fs::metadata(&path).await?;

        Ok(ResolvedObject {
            path,
            object,
            metadata,
            thumbnails,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn item_type(&self) -> ItemType {
        ItemType::new(&self.path, &self.metadata)
    }

    pub async fn into_thumbnail(self, size: (u32, u32)) -> Result<(Bytes, String)> {
        self.thumbnails.get(self.path, &self.metadata, size).await
    }

    pub async fn list(&self) -> Result<Vec<DirListingItem>> {
        let mut result = Vec::new();

        let mut dir = fs::read_dir(&self.path).await?;
        while let Some(entry) = dir.next_entry().await? {
            if let Some(item) = DirListingItem::with_dir_entry(entry).await? {
                result.push(item);
            }
        }

        Ok(result)
    }
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

    fn get_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

fn get_source_hash(path: &Path, metadata: &Metadata) -> Option<u64> {
    if metadata.is_dir() {
        None
    } else {
        Some(CacheSourceKey::with_metadata(path, metadata).get_hash())
    }
}

#[derive(Debug)]
pub struct DirListingItem {
    pub name: Arc<str>,
    pub item_type: ItemType,
    pub file_size: u64,
    pub modified: Option<DateTime<Utc>>,
    pub source_hash: Option<u64>,
}

impl DirListingItem {
    /// Create the dir listing item from directory entry.
    /// If the filename contains non-unicode characters, returns Ok(None).
    async fn with_dir_entry(entry: fs::DirEntry) -> std::io::Result<Option<Self>> {
        let Ok(name) = entry.file_name().into_string() else {
            return Ok(None);
        };
        Ok(Some(Self::with_metadata(
            &entry.path(),
            name.into(),
            &entry.metadata().await?,
        )))
    }

    fn with_metadata(path: &Path, name: Arc<str>, metadata: &Metadata) -> Self {
        let item_type = ItemType::new(path, metadata);
        DirListingItem {
            name,
            item_type,
            file_size: metadata.len(),
            modified: metadata.modified().ok().map(Into::into),
            source_hash: get_source_hash(path, metadata),
        }
    }
}

pub struct AppData {
    config: Config,
    objects: RwLock<Storage<Object>>,
    // The RwLock not only protects the Storage object, but also the data stored on the filesystem
    thumbnails: CachedThumbnails,
    static_content_hash: String,
}

impl AppData {
    pub fn with_config(config: Config) -> Result<Self> {
        let path = config.data_path.join("metadata.json");
        let objects = RwLock::new(Storage::new(path)?);
        let thumbnail_cache_size = config.thumbnail_cache_size;
        let static_content_hash = format!("{:X}", thread_rng().next_u32());
        Ok(AppData {
            config,
            objects,
            thumbnails: CachedThumbnails::new(thumbnail_cache_size),
            static_content_hash,
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

    pub fn get_static_content_hash(&self) -> &str {
        &self.static_content_hash
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

    pub async fn resolve_object<'a>(
        &'a self,
        path: &str,
        key: Option<&str>,
    ) -> Result<ResolvedObject<'a>> {
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
            return Err(FiledlError::Unlisted);
        }

        // TODO: Verify that subobject path is not weird
        // TODO: Handle expiry?

        let mut object_fs_path = self.get_object_path(object_id, &obj);
        if let Some(subobject_path) = subobject_path {
            object_fs_path.push(subobject_path);
        }

        let result = ResolvedObject::new(object_fs_path, obj, &self.thumbnails).await?;
        Ok(result)
    }

    async fn object_from_id<'a>(&'a self, id: &str) -> Result<RwLockReadGuard<'a, Object>> {
        RwLockReadGuard::try_map(self.objects.read().await, |objects| objects.get(id))
            .map_err(|_| FiledlError::ObjectNotFound)
    }

    pub async fn list_objects(&self) -> Result<Vec<DirListingItem>> {
        let mut result = Vec::new();

        for (key, obj) in self.objects.read().await.iter() {
            let path = self.get_object_path(key, obj);
            let metadata = fs::metadata(&path).await?;
            if obj.unlisted_key.is_none() {
                result.push(DirListingItem::with_metadata(
                    &path,
                    Arc::clone(key),
                    &metadata,
                ));
            }
        }

        Ok(result)
    }
}
