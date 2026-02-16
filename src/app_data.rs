use crate::{
    config::Config,
    error::{FiledlError, Result, StartupError},
    storage::Storage,
    templates::util::url_encode,
    thumbnails::{CacheStats, CachedThumbnails, ThumbnailType, is_thumbnailable},
};
use actix_web::web::Bytes;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::{Stream, pin_mut};
use rand::{Rng as _, rng};
use relative_path::RelativePathBuf;
use serde::{Deserialize, Serialize};
use std::{
    fs::Metadata,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{RwLock, RwLockReadGuard},
};

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
    object_path: String,
    storage_path: PathBuf,
    object: RwLockReadGuard<'a, Object>,
    metadata: Metadata,
    thumbnails: &'a CachedThumbnails,
}

impl<'a> ResolvedObject<'a> {
    async fn new(
        object_path: String,
        storage_path: PathBuf,
        object: RwLockReadGuard<'a, Object>,
        thumbnails: &'a CachedThumbnails,
    ) -> Result<Self> {
        let metadata = fs::metadata(&storage_path).await?;

        Ok(ResolvedObject {
            object_path,
            storage_path,
            object,
            metadata,
            thumbnails,
        })
    }

    /// Returns the path under which this object was accessed.
    pub fn object_path(&self) -> &str {
        &self.object_path
    }

    /// Returns the filesystem path where the object's data can be found.
    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    /// Returns the filesystem path where the object's data can be found.
    pub fn into_storage_path(self) -> PathBuf {
        self.storage_path
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn item_type(&self) -> ItemType {
        ItemType::new(&self.metadata)
    }

    pub async fn into_thumbnail(
        self,
        resolution: (u32, u32),
        thumbnail_type: ThumbnailType,
    ) -> Result<(Bytes, String)> {
        self.thumbnails
            .get(
                self.storage_path,
                &self.metadata,
                resolution,
                thumbnail_type,
            )
            .await
    }

    pub async fn list(&self) -> Result<Vec<DirListingItem>> {
        let mut result = Vec::new();

        let mut dir = fs::read_dir(&self.storage_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            if let Some(item) = DirListingItem::with_dir_entry(entry).await? {
                result.push(item);
            }
        }

        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemType {
    Directory,
    /// File of other/unknown type
    File,
}

impl ItemType {
    pub fn new(metadata: &Metadata) -> Self {
        if metadata.is_dir() {
            ItemType::Directory
        } else {
            ItemType::File
        }
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
    pub is_thumbnailable: bool,
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
        DirListingItem {
            name,
            item_type: ItemType::new(metadata),
            is_thumbnailable: is_thumbnailable(path),
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
    download_base_url: String,
}

impl AppData {
    pub fn with_config(config: Config) -> std::result::Result<Self, StartupError> {
        let path = config.data_path.join("metadata.json");
        let objects = RwLock::new(Storage::new(path)?);
        let thumbnail_cache_size = config.thumbnail_cache_size;
        let static_content_hash = format!("{:X}", rng().next_u32());
        let download_base_url = format!("{}", url_encode(&config.download_url))
            .trim_end_matches('/')
            .to_owned();

        let data = AppData {
            config,
            objects,
            thumbnails: CachedThumbnails::new(thumbnail_cache_size),
            static_content_hash,
            download_base_url,
        };

        // Remove the upload temp area from possible previous failed uploads
        // Ignoring errors
        let _ = std::fs::remove_dir_all(data.get_upload_temp_path());

        Ok(data)
    }

    pub fn get_download_base_url(&self) -> &str {
        &self.download_base_url
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
        self.thumbnails.cache_stats()
    }

    fn get_owned_object_storage_path(&self, object_id: &str) -> PathBuf {
        let mut path = self.config.data_path.join("owned_data");
        path.push(object_id);
        path
    }

    fn get_object_storage_path(&self, object_id: &str, obj: &Object) -> PathBuf {
        match &obj.ownership {
            ObjectOwnership::Owned => self.get_owned_object_storage_path(object_id),
            ObjectOwnership::Linked(link_path) => {
                link_path.to_path(&self.config.linked_objects_root)
            }
        }
    }

    fn get_upload_temp_path(&self) -> PathBuf {
        self.config.data_path.join("temp_upload")
    }

    pub async fn resolve_object<'a>(
        &'a self,
        path: String,
        key: Option<&str>,
    ) -> Result<ResolvedObject<'a>> {
        let (object_id, subobject_path) = match path.split_once('/') {
            Some((object_id, subobject_path)) => {
                if subobject_path.split('/').any(|part| part == "..") {
                    return Err(FiledlError::DirectoryTraversal {
                        path: path.to_owned(),
                    });
                }
                (object_id, Some(subobject_path))
            }
            None => (path.as_str(), None),
        };

        let obj = self.object_from_id(object_id).await?;
        if obj
            .unlisted_key
            .as_ref()
            .is_some_and(|expected_key| key != Some(expected_key))
        {
            return Err(FiledlError::Unlisted {
                path: path.to_owned(),
                key: key.map(|key| key.to_owned()),
            });
        }

        // TODO: Handle expiry?

        let mut object_fs_path = self.get_object_storage_path(object_id, &obj);
        if let Some(subobject_path) = subobject_path {
            object_fs_path.push(subobject_path);
        }

        let result = ResolvedObject::new(path, object_fs_path, obj, &self.thumbnails).await?;
        Ok(result)
    }

    async fn object_from_id<'a>(&'a self, id: &str) -> Result<RwLockReadGuard<'a, Object>> {
        RwLockReadGuard::try_map(self.objects.read().await, |objects| objects.get(id))
            .map_err(|_| FiledlError::ObjectNotFound)
    }

    pub async fn list_objects(&self) -> Result<Vec<DirListingItem>> {
        let mut result = Vec::new();

        for (key, obj) in self.objects.read().await.iter() {
            let path = self.get_object_storage_path(key, obj);
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

    /// Creates an owned object that contains just a single file with the given content.
    pub async fn upload_simple_object<S, E>(&self, object_id: Arc<str>, content: S) -> Result<()>
    where
        S: Stream<Item = std::result::Result<Bytes, E>>,
        E: Into<FiledlError>,
    {
        use futures::StreamExt;

        // 1. Optimistic check of the metadata, allowing us to reject duplicate uploads early.
        if self.object_from_id(&object_id).await.is_ok() {
            return Err(FiledlError::ObjectExists { object_id });
        }

        // 2. Copy the uploaded data to a temp file
        let upload_temp_path = self.get_upload_temp_path();
        tokio::fs::create_dir_all(&upload_temp_path).await?;
        let (f, temp_path) = tempfile::NamedTempFile::new_in(upload_temp_path)?.into_parts();
        let mut f = tokio::fs::File::from_std(f);

        pin_mut!(content);

        // 3. Copy the content to file, this might take a long time
        while let Some(block) = content.next().await {
            let block = block.map_err(|e| e.into())?;
            f.write_all(&block).await?;
        }

        drop(f); // We're done with the file, only using the temp_path from now on

        // 4. Lock the storage for writing, create the object and move downloaded file
        // to the final location.
        // This can still fail if someone created the file while we were uploading.
        let mut guard = self.objects.write().await;

        if !guard.create(
            Arc::clone(&object_id),
            Object {
                ownership: ObjectOwnership::Owned,
                expires: None,
                unlisted_key: None,
            },
        ) {
            return Err(FiledlError::ObjectExists { object_id });
        }

        tokio::fs::rename(
            temp_path.keep().unwrap(),
            self.get_owned_object_storage_path(&object_id),
        )
        .await?; // TODO: What happens if this fails?

        Ok(())
    }
}
