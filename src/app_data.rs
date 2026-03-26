use crate::{
    config::Config,
    error::StartupError,
    storage::Storage,
    templates::util::url_encode,
    thumbnails::{CacheStats, CachedThumbnails, ThumbnailType, is_thumbnailable},
};
use actix_web::{http::StatusCode, web::Bytes};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::{Stream, pin_mut};
use rand::{Rng as _, RngExt as _, rng};
use relative_path::RelativePathBuf;
use serde::{Deserialize, Serialize};
use snafu::{OptionExt as _, ResultExt as _, prelude::*};
use std::{
    fs::Metadata,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{RwLock, RwLockMappedWriteGuard, RwLockReadGuard, RwLockWriteGuard, watch},
    time::timeout,
};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum AppDataError {
    #[snafu(display("Object {object_id:?} not found at {location}"))]
    ObjectNotFound {
        object_id: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Object {object_id:?} has expired at {location}"))]
    Expired {
        object_id: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Object {object_id:?} already exists at {location}"))]
    ObjectExists {
        object_id: Arc<str>,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Unlisted object {path} accessed with wrong key {key:?} at {location}"))]
    Unlisted {
        path: String,
        key: Option<String>,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Directory traversal in path {path} at {location}"))]
    DirectoryTraversal {
        path: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("IO error at {location}"))]
    IOError {
        source: std::io::Error,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Thumbnail generation failed at {location}"))]
    #[snafu(context(false))]
    ThumbnailError {
        source: crate::thumbnails::ThumbnailError,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Request payload error at {location}"))]
    PayloadError {
        #[snafu(source(from(actix_web::error::PayloadError, Box::new)))]
        source: Box<actix_web::error::PayloadError>,
        #[snafu(implicit)]
        location: snafu::Location,
    },
}

impl AppDataError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::ObjectNotFound { .. } | Self::Expired { .. } | Self::Unlisted { .. } => {
                StatusCode::NOT_FOUND
            }
            Self::ObjectExists { .. } => StatusCode::CONFLICT,
            Self::DirectoryTraversal { .. } => StatusCode::BAD_REQUEST,
            Self::IOError { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                StatusCode::NOT_FOUND
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub type AppDataResult<T> = std::result::Result<T, AppDataError>;

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

impl Object {
    pub fn is_expired(&self) -> bool {
        self.is_expired_at(Utc::now())
    }

    pub fn is_expired_at(&self, at: DateTime<Utc>) -> bool {
        self.expires.is_some_and(|exp| exp <= at)
    }
}

#[derive(Debug)]
pub struct ResolvedObject<'a> {
    object_path: String,
    storage_path: PathBuf,
    metadata: Metadata,
    thumbnails: &'a CachedThumbnails,
    expires: Option<DateTime<Utc>>,
}

impl<'a> ResolvedObject<'a> {
    async fn new(
        object_path: String,
        storage_path: PathBuf,
        thumbnails: &'a CachedThumbnails,
        expires: Option<DateTime<Utc>>,
    ) -> AppDataResult<Self> {
        let metadata = fs::metadata(&storage_path).await.context(IOSnafu)?;

        Ok(ResolvedObject {
            object_path,
            storage_path,
            metadata,
            thumbnails,
            expires,
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

    pub fn get_expires(&self) -> Option<DateTime<Utc>> {
        self.expires
    }

    pub async fn into_thumbnail(
        self,
        resolution: (u32, u32),
        thumbnail_type: ThumbnailType,
    ) -> AppDataResult<(Bytes, String)> {
        Ok(self
            .thumbnails
            .get(
                self.storage_path,
                &self.metadata,
                resolution,
                thumbnail_type,
            )
            .await?)
    }

    pub async fn list(&self) -> AppDataResult<Vec<DirListingItem>> {
        let mut result = Vec::new();

        let mut dir = fs::read_dir(&self.storage_path).await.context(IOSnafu)?;
        while let Some(entry) = dir.next_entry().await.context(IOSnafu)? {
            if let Some(item) = DirListingItem::with_dir_entry(entry)
                .await
                .context(IOSnafu)?
            {
                result.push(item);
            }
        }

        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
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
pub struct AdminObjectInfo {
    pub item: DirListingItem,
    pub ownership: ObjectOwnership,
    pub unlisted_key: Option<Arc<str>>,
    pub expires: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct DirListingItem {
    pub name: Arc<str>,
    pub item_type: ItemType,
    pub is_thumbnailable: bool,
    pub file_size: u64,
    pub modified: Option<DateTime<Utc>>,
    pub source_hash: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<DateTime<Utc>>,
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
            expires: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BrowseLinkedEntry {
    pub name: String,
    pub is_dir: bool,
}

impl AppData {
    pub async fn browse_linked_directory(
        &self,
        path: &str,
    ) -> AppDataResult<Vec<BrowseLinkedEntry>> {
        if path.split('/').any(|part| part == "..") {
            return DirectoryTraversalSnafu {
                path: path.to_owned(),
            }
            .fail();
        }

        let full_path = RelativePathBuf::from(path).to_path(&self.config.linked_objects_root);
        let mut dir = tokio::fs::read_dir(&full_path).await.context(IOSnafu)?;
        let mut entries = Vec::new();

        while let Some(entry) = dir.next_entry().await.context(IOSnafu)? {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let file_type = entry.file_type().await.context(IOSnafu)?;
            entries.push(BrowseLinkedEntry {
                name,
                is_dir: file_type.is_dir(),
            });
        }

        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        Ok(entries)
    }
}

pub fn generate_unlisted_key() -> Arc<str> {
    format!("{:032x}", rand::rng().random::<u128>()).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppDataSignal {
    Rescan,
    Quit,
}

pub struct AppData {
    config: Config,
    objects: RwLock<Storage<Object>>,
    // The RwLock not only protects the Storage object, but also the data stored on the filesystem
    thumbnails: CachedThumbnails,
    static_content_hash: String,
    download_base_url: String,
    admin_objects_base_url: String,
    signal_tx: watch::Sender<AppDataSignal>,
    background_tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl AppData {
    pub fn with_config(
        config: Config,
        spawn_background_tasks: bool,
    ) -> std::result::Result<Arc<Self>, StartupError> {
        let path = config.data_path.join("metadata.json");
        let objects = RwLock::new(Storage::new(path)?);
        let thumbnail_cache_size = config.thumbnail_cache_size;
        let static_content_hash = format!("{:X}", rng().next_u32());
        let download_base_url = format!("{}", url_encode(&config.download_url))
            .trim_end_matches('/')
            .to_owned();
        let admin_objects_base_url = format!(
            "{}/objects",
            format!("{}", url_encode(&config.admin_url)).trim_end_matches('/')
        );

        let (signal_tx, _) = watch::channel(AppDataSignal::Rescan);

        let data = AppData {
            config,
            objects,
            thumbnails: CachedThumbnails::new(thumbnail_cache_size),
            static_content_hash,
            download_base_url,
            admin_objects_base_url,
            signal_tx,
            background_tasks: std::sync::Mutex::new(Vec::new()),
        };

        // Remove the upload temp area from possible previous failed uploads
        // Ignoring errors
        let _ = std::fs::remove_dir_all(data.get_upload_temp_path());

        let app = Arc::new(data);

        if spawn_background_tasks {
            let mut tasks = app.background_tasks.lock().unwrap();
            {
                let app = Arc::clone(&app);
                let signal_rx = app.signal_tx.subscribe();
                tasks.push(tokio::spawn(
                    async move { app.expiry_task(signal_rx).await },
                ));
            }
            {
                let app = Arc::clone(&app);
                let signal_rx = app.signal_tx.subscribe();
                tasks.push(tokio::spawn(async move {
                    app.storage_dump_task(signal_rx).await
                }));
            }
        }

        Ok(app)
    }

    pub fn get_download_base_url(&self) -> &str {
        &self.download_base_url
    }

    pub fn get_admin_objects_base_url(&self) -> &str {
        &self.admin_objects_base_url
    }

    pub fn get_admin_url(&self) -> &str {
        &self.config.admin_url
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
    ) -> AppDataResult<ResolvedObject<'a>> {
        let (object_id, subobject_path) = match path.split_once('/') {
            Some((object_id, subobject_path)) => {
                if subobject_path.split('/').any(|part| part == "..") {
                    return DirectoryTraversalSnafu {
                        path: path.to_owned(),
                    }
                    .fail();
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
            return UnlistedSnafu {
                path: path.to_owned(),
                key: key.map(|key| key.to_owned()),
            }
            .fail();
        }

        if obj.is_expired() {
            log::info!("Ignoring expired object {}", object_id);
            return ExpiredSnafu {
                object_id: object_id.to_owned(),
            }
            .fail();
        }

        let expires = obj.expires;
        let mut object_fs_path = self.get_object_storage_path(object_id, &obj);
        if let Some(subobject_path) = subobject_path {
            object_fs_path.push(subobject_path);
        }
        drop(obj);

        let result = ResolvedObject::new(path, object_fs_path, &self.thumbnails, expires).await?;
        Ok(result)
    }

    pub async fn list_objects_admin(&self) -> AppDataResult<Vec<AdminObjectInfo>> {
        let mut result = Vec::new();

        for (key, obj) in self.objects.read().await.iter() {
            let path = self.get_object_storage_path(key, obj);
            let metadata = fs::metadata(&path).await.context(IOSnafu)?;
            result.push(AdminObjectInfo {
                item: DirListingItem::with_metadata(&path, Arc::clone(key), &metadata),
                ownership: obj.ownership.clone(),
                unlisted_key: obj.unlisted_key.clone(),
                expires: obj.expires,
            });
        }

        Ok(result)
    }

    pub async fn delete_object(&self, object_id: &str) -> AppDataResult<()> {
        let mut guard = self.objects.write().await;
        let obj = guard
            .remove(object_id)
            .context(ObjectNotFoundSnafu { object_id })?;
        drop(guard);

        if matches!(obj.ownership, ObjectOwnership::Owned) {
            let path = self.get_owned_object_storage_path(object_id);
            remove_file_or_directory(&path).await.context(IOSnafu)?;
        }
        Ok(())
    }

    /// Validate that an object ID contains no path separators.
    /// Object IDs must be flat identifiers — slashes would create nested directories.
    fn validate_object_id(object_id: &str) -> AppDataResult<()> {
        if object_id.contains('/') {
            return DirectoryTraversalSnafu {
                path: object_id.to_string(),
            }
            .fail();
        }
        Ok(())
    }

    pub async fn create_linked_object(
        &self,
        object_id: Arc<str>,
        link_path: RelativePathBuf,
        unlisted_key: Option<Arc<str>>,
        expires: Option<DateTime<Utc>>,
    ) -> AppDataResult<()> {
        Self::validate_object_id(&object_id)?;
        if link_path.as_str().split('/').any(|part| part == "..") {
            return DirectoryTraversalSnafu {
                path: link_path.to_string(),
            }
            .fail();
        }

        // Verify the path actually exists at creation time
        let fs_path = link_path.to_path(&self.config.linked_objects_root);
        tokio::fs::metadata(&fs_path).await.context(IOSnafu)?;

        let mut guard = self.objects.write().await;
        if !guard.create(
            Arc::clone(&object_id),
            Object {
                ownership: ObjectOwnership::Linked(link_path),
                expires,
                unlisted_key,
            },
        ) {
            return ObjectExistsSnafu { object_id }.fail();
        }
        Ok(())
    }

    async fn object_from_id<'a>(&'a self, id: &str) -> AppDataResult<RwLockReadGuard<'a, Object>> {
        RwLockReadGuard::try_map(self.objects.read().await, |objects| objects.get(id))
            .map_err(|_| ObjectNotFoundSnafu { object_id: id }.build())
    }

    pub async fn list_objects(&self) -> AppDataResult<Vec<DirListingItem>> {
        let mut result = Vec::new();

        for (key, obj) in self.objects.read().await.iter() {
            if obj.unlisted_key.is_some() {
                continue;
            }
            if obj.is_expired() {
                log::info!("Ignoring expired object {} in listing", key);
                continue;
            }
            let path = self.get_object_storage_path(key, obj);
            let metadata = fs::metadata(&path).await.context(IOSnafu)?;
            let mut item = DirListingItem::with_metadata(&path, Arc::clone(key), &metadata);
            item.expires = obj.expires;
            result.push(item);
        }

        Ok(result)
    }

    pub async fn get_object_mut(
        &self,
        object_id: &str,
    ) -> AppDataResult<RwLockMappedWriteGuard<'_, Object>> {
        RwLockWriteGuard::try_map(self.objects.write().await, |storage| {
            storage.get_mut(object_id)
        })
        .map_err(|_| ObjectNotFoundSnafu { object_id }.build())
    }

    pub fn signal_expiry_change(&self) {
        let _ = self.signal_tx.send(AppDataSignal::Rescan);
    }

    pub async fn shutdown(&self) -> std::io::Result<()> {
        let _ = self.signal_tx.send(AppDataSignal::Quit);

        let tasks = std::mem::take(&mut *self.background_tasks.lock().unwrap());
        for task in tasks {
            let _ = task.await;
        }

        let mut objects = self.objects.write().await;
        if objects.is_dirty() {
            objects.dump()?;
        }

        Ok(())
    }

    /// Goes through all objects in the storage and removes any expired entries,
    /// including deleting the files for owned expired entries.
    /// Errors during file and directory deletion are logged, but ignored.
    async fn delete_expired_and_get_next(&self) -> Option<DateTime<Utc>> {
        let mut objects = self.objects.write().await;
        let mut next_expiry: Option<DateTime<Utc>> = None;
        let mut owned_to_delete: Vec<PathBuf> = Vec::new();
        let now = Utc::now();

        objects.retain(|id, object| {
            if object.is_expired_at(now) {
                log::info!("Deleting expired object {id}");

                if matches!(object.ownership, ObjectOwnership::Owned) {
                    owned_to_delete.push(self.get_owned_object_storage_path(id));
                }

                false
            } else {
                next_expiry = match (next_expiry, object.expires) {
                    (Some(e1), Some(e2)) => Some(e1.min(e2)),
                    (Some(e1), None) => Some(e1),
                    (None, Some(e2)) => Some(e2),
                    (None, None) => None,
                };

                true
            }
        });

        // We don't drop the lock now, because it is used to protect the files as well as the metadata

        for path in owned_to_delete {
            if let Err(e) = remove_file_or_directory(&path).await {
                log::error!("Ignoring error while removing {}: {}", path.display(), e);
            }
        }

        next_expiry
    }

    async fn expiry_task(&self, mut signal_rx: watch::Receiver<AppDataSignal>) {
        log::info!("Starting expiry task");
        loop {
            let timeout_duration = match self.delete_expired_and_get_next().await {
                Some(next_expiry) => (next_expiry - Utc::now())
                    .to_std()
                    .unwrap_or(Duration::ZERO),
                None => Duration::MAX, // It's easier to just wait a long time, the theoretical extra wakeup will not hurt anything.
            };

            match timeout(timeout_duration, signal_rx.changed()).await {
                Ok(Ok(())) => {
                    // Received a command without timeout
                    let command = *signal_rx.borrow_and_update();
                    match command {
                        AppDataSignal::Rescan => (),  // Just let the loop spin
                        AppDataSignal::Quit => break, // Quit the task
                    }
                }
                Ok(Err(_)) => {
                    // The watch channel reports an error
                    break;
                }
                Err(_) => {
                    // Timed out.
                    // This means that either an entry should be expiring by now, or that we're
                    // rescanning in the "just in case" situation.
                    // Nothing to do.
                }
            }
        }
        log::info!("Finished expiry task");
    }

    async fn storage_dump_task(&self, mut signal_rx: watch::Receiver<AppDataSignal>) {
        log::info!("Starting storage dump task");
        loop {
            if !self.objects.read().await.is_dirty() {
                // Wait for an event first:
                match signal_rx.changed().await {
                    Ok(()) => {
                        let command = *signal_rx.borrow_and_update();
                        match command {
                            AppDataSignal::Rescan => (),
                            AppDataSignal::Quit => break,
                        }
                    }
                    Err(_) => break,
                }

                if !self.objects.read().await.is_dirty() {
                    // If we were woken up and the storage is not dirty, just continue to next iteration
                    continue;
                }
            }

            // Actually dumping the storage:
            let dump_result = self.objects.write().await.dump();
            if let Err(e) = dump_result {
                log::error!("Ignoring error while periodically dumping storage: {}", e);
            }

            // Wait for the cooldown interval before dumping the storage next time
            match timeout(
                Duration::from_secs(60),
                signal_rx.wait_for(|command| matches!(command, AppDataSignal::Quit)),
            )
            .await
            {
                Ok(Ok(_quit)) => {
                    // Received a quit without timeout
                    break;
                }
                Ok(Err(_)) => {
                    // The watch channel reports an error
                    break;
                }
                Err(_) => {
                    // Timed out -- the cooldown has ended, let's continue normally
                }
            }
        }
        log::info!("Finished storage dump task");
    }

    pub async fn upload_simple_object(
        &self,
        object_id: Arc<str>,
        content: impl Stream<Item = std::result::Result<Bytes, actix_web::error::PayloadError>>,
        unlisted_key: Option<Arc<str>>,
        expires: Option<DateTime<Utc>>,
    ) -> AppDataResult<()> {
        use futures::StreamExt;

        Self::validate_object_id(&object_id)?;

        // 1. Optimistic check of the metadata, allowing us to reject duplicate uploads early.
        if self.object_from_id(&object_id).await.is_ok() {
            return ObjectExistsSnafu { object_id }.fail();
        }

        // 2. Copy the uploaded data to a temp file
        let upload_temp_path = self.get_upload_temp_path();
        tokio::fs::create_dir_all(&upload_temp_path)
            .await
            .context(IOSnafu)?;
        let (f, temp_path) = tempfile::NamedTempFile::new_in(upload_temp_path)
            .context(IOSnafu)?
            .into_parts();
        let mut f = tokio::fs::File::from_std(f);

        pin_mut!(content);

        // 3. Copy the content to file, this might take a long time
        while let Some(block) = content.next().await {
            let block = block.context(PayloadSnafu)?;
            f.write_all(&block).await.context(IOSnafu)?;
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
                expires,
                unlisted_key,
            },
        ) {
            return ObjectExistsSnafu { object_id }.fail();
        }

        tokio::fs::rename(
            temp_path.keep().unwrap(),
            self.get_owned_object_storage_path(&object_id),
        )
        .await
        .context(IOSnafu)?; // TODO: What happens if this fails?

        Ok(())
    }
}

async fn remove_file_or_directory(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path).await?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).await
    } else {
        fs::remove_file(path).await
    }
}

#[cfg(test)]
mod tests {
    mod expiry {
        use super::super::*;
        use chrono::Duration;

        fn make_object(expires: Option<DateTime<Utc>>) -> Object {
            Object {
                ownership: ObjectOwnership::Owned,
                expires,
                unlisted_key: None,
            }
        }

        #[test]
        fn is_expired_when_past() {
            let obj = make_object(Some(Utc::now() - Duration::hours(1)));
            assert!(obj.is_expired());
        }

        #[test]
        fn is_not_expired_when_future() {
            let obj = make_object(Some(Utc::now() + Duration::hours(1)));
            assert!(!obj.is_expired());
        }

        #[test]
        fn is_not_expired_when_none() {
            let obj = make_object(None);
            assert!(!obj.is_expired());
        }

        /// Creates an AppData with a test config in a temp directory, without
        /// background tasks so tests can call internal methods without racing
        /// the expiry or dump loops.
        fn test_app() -> (tempfile::TempDir, Arc<AppData>) {
            let dir = tempfile::tempdir().unwrap();
            let config = Config {
                bind_address: "localhost".into(),
                bind_port: 0,
                data_path: dir.path().to_path_buf(),
                linked_objects_root: dir.path().to_path_buf(),
                download_url: "/download".into(),
                admin_url: "/admin".into(),
                app_name: "test".into(),
                display_timezone: chrono_tz::UTC,
                thumbnail_cache_size: 1024,
            };
            let app = AppData::with_config(config, false).unwrap();
            (dir, app)
        }

        #[tokio::test]
        async fn delete_expired_removes_expired_object() {
            let (_dir, app) = test_app();

            let past = Utc::now() - Duration::hours(1);
            let future = Utc::now() + Duration::hours(1);

            {
                let mut guard = app.objects.write().await;
                guard.create("expired_obj".into(), make_object(Some(past)));
                guard.create("future_obj".into(), make_object(Some(future)));
            }

            let next = app.delete_expired_and_get_next().await;

            // The expired object should be removed
            assert!(app.objects.read().await.get("expired_obj").is_none());
            // The future object should still exist
            assert!(app.objects.read().await.get("future_obj").is_some());
            // Next expiry should be approximately the future time
            assert!(next.is_some());
            let next = next.unwrap();
            assert!((next - future).num_seconds().abs() < 2);
        }

        #[tokio::test]
        async fn resolve_expired_object_returns_error() {
            let (dir, app) = test_app();
            // Create a file so the linked path resolves
            std::fs::write(dir.path().join("testfile"), b"hello").unwrap();

            let past = Utc::now() - Duration::hours(1);
            {
                let mut guard = app.objects.write().await;
                guard.create(
                    "expired_link".into(),
                    Object {
                        ownership: ObjectOwnership::Linked("testfile".into()),
                        expires: Some(past),
                        unlisted_key: None,
                    },
                );
            }

            let result = app.resolve_object("expired_link".to_owned(), None).await;
            assert!(result.is_err());
            assert!(
                matches!(result.unwrap_err(), AppDataError::Expired { object_id, .. } if object_id == "expired_link")
            );
        }

        #[tokio::test]
        async fn delete_expired_cleans_up_owned_files() {
            let (dir, app) = test_app();

            let owned_data = dir.path().join("owned_data");
            std::fs::create_dir_all(&owned_data).unwrap();
            let expired_file = owned_data.join("expired_obj");
            let future_file = owned_data.join("future_obj");
            std::fs::write(&expired_file, b"expired").unwrap();
            std::fs::write(&future_file, b"future").unwrap();

            let past = Utc::now() - Duration::hours(1);
            let future = Utc::now() + Duration::hours(1);

            {
                let mut guard = app.objects.write().await;
                guard.create("expired_obj".into(), make_object(Some(past)));
                guard.create("future_obj".into(), make_object(Some(future)));
            }

            app.delete_expired_and_get_next().await;

            assert!(!expired_file.exists());
            assert!(future_file.exists());
        }

        #[tokio::test]
        async fn delete_expired_does_not_dirty_when_nothing_expired() {
            let (_dir, app) = test_app();

            let future = Utc::now() + Duration::hours(1);
            {
                let mut guard = app.objects.write().await;
                guard.create("future_obj".into(), make_object(Some(future)));
                guard.dump().unwrap();
            }

            assert!(!app.objects.read().await.is_dirty());
            app.delete_expired_and_get_next().await;
            assert!(!app.objects.read().await.is_dirty());
        }

        #[tokio::test]
        async fn shutdown_dumps_dirty_storage() {
            let (dir, app) = test_app();

            {
                let mut guard = app.objects.write().await;
                guard.create("obj".into(), make_object(None));
            }
            assert!(app.objects.read().await.is_dirty());

            app.shutdown().await.unwrap();

            // Verify the data was persisted to disk
            let contents = std::fs::read_to_string(dir.path().join("metadata.json")).unwrap();
            assert!(contents.contains("obj"));
        }

        #[tokio::test]
        async fn shutdown_skips_dump_when_clean() {
            let (dir, app) = test_app();

            // Storage is clean (no mutations)
            assert!(!app.objects.read().await.is_dirty());

            app.shutdown().await.unwrap();

            // metadata.json should not exist (never written since storage was empty and clean)
            assert!(!dir.path().join("metadata.json").exists());
        }
    }

    mod browse_linked {
        use super::super::*;

        fn test_app() -> (tempfile::TempDir, Arc<AppData>) {
            let dir = tempfile::tempdir().unwrap();
            let config = Config {
                bind_address: "localhost".into(),
                bind_port: 0,
                data_path: dir.path().to_path_buf(),
                linked_objects_root: dir.path().to_path_buf(),
                download_url: "/download".into(),
                admin_url: "/admin".into(),
                app_name: "test".into(),
                display_timezone: chrono_tz::UTC,
                thumbnail_cache_size: 1024,
            };
            let app = AppData::with_config(config, false).unwrap();
            (dir, app)
        }

        #[tokio::test]
        async fn lists_files_and_directories() {
            let (dir, app) = test_app();
            std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
            std::fs::create_dir(dir.path().join("subdir")).unwrap();

            let entries = app.browse_linked_directory("").await.unwrap();
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            assert!(names.contains(&"file.txt"));
            assert!(names.contains(&"subdir"));

            let subdir_entry = entries.iter().find(|e| e.name == "subdir").unwrap();
            assert!(subdir_entry.is_dir);
            let file_entry = entries.iter().find(|e| e.name == "file.txt").unwrap();
            assert!(!file_entry.is_dir);
        }

        #[tokio::test]
        async fn directories_sorted_first() {
            let (dir, app) = test_app();
            std::fs::write(dir.path().join("aaa_file"), "").unwrap();
            std::fs::create_dir(dir.path().join("zzz_dir")).unwrap();

            let entries = app.browse_linked_directory("").await.unwrap();
            assert!(entries[0].is_dir, "directories should come first");
        }

        #[tokio::test]
        async fn rejects_directory_traversal() {
            let (_dir, app) = test_app();

            let result = app.browse_linked_directory("..").await;
            assert!(result.is_err());

            let result = app.browse_linked_directory("foo/../..").await;
            assert!(result.is_err());

            let result = app.browse_linked_directory("../etc").await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn browses_subdirectory() {
            let (dir, app) = test_app();
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            std::fs::write(dir.path().join("sub").join("nested.txt"), "").unwrap();

            let entries = app.browse_linked_directory("sub").await.unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].name, "nested.txt");
        }

        #[tokio::test]
        async fn nonexistent_path_returns_error() {
            let (_dir, app) = test_app();
            let result = app.browse_linked_directory("no_such_dir").await;
            assert!(result.is_err());
        }
    }
}
