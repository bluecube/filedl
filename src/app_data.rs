use crate::{
    config::Config,
    error::{MainError, MainIOSnafu},
    storage::{Storage, WriteError},
    thumbnails::{CacheStats, CachedThumbnails, ThumbnailType, is_thumbnailable},
};
use actix_web::{http::StatusCode, web::Bytes};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::{Stream, pin_mut};
use rand::{Rng as _, rng};
use relative_path::{RelativePath, RelativePathBuf};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt as _, ResultExt as _, prelude::*};
use std::{
    fs::Metadata,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{Mutex, Notify, RwLock, RwLockReadGuard},
    task::JoinHandle,
    time::timeout,
};

/// Identifies which interface (download or admin) a request is being served from.
/// Registered as scope-level `web::Data` so handlers can use the correct base URLs.
#[derive(Copy, Clone, Debug)]
pub enum InterfaceContext {
    Download,
    Admin,
}

impl InterfaceContext {
    pub fn get_base_url<'a>(&self, app: &'a AppData) -> &'a str {
        match self {
            InterfaceContext::Download => app.get_download_base_url(),
            InterfaceContext::Admin => app.get_admin_url(),
        }
    }

    pub fn get_objects_base_url<'a>(&self, app: &'a AppData) -> &'a str {
        match self {
            InterfaceContext::Download => app.get_download_base_url(),
            InterfaceContext::Admin => app.get_admin_objects_base_url(),
        }
    }

    pub fn is_admin(&self) -> bool {
        matches!(self, InterfaceContext::Admin)
    }
}

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

    #[snafu(display("Invalid object ID {object_id:?} at {location}"))]
    InvalidObjectId {
        object_id: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Invalid link path {path:?} at {location}"))]
    InvalidLinkPath {
        path: String,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("IO error ({path:?}) at {location}"))]
    IOError {
        path: PathBuf,
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

    #[snafu(display("Metadata storage error at {location}"))]
    #[snafu(context(false))]
    StorageError {
        source: WriteError,
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
            Self::DirectoryTraversal { .. }
            | Self::InvalidObjectId { .. }
            | Self::InvalidLinkPath { .. } => StatusCode::BAD_REQUEST,
            Self::IOError { source, .. }
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                StatusCode::NOT_FOUND
            }
            Self::StorageError {
                source: WriteError::StorageAlreadyShutDown { .. },
                ..
            } => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns a user-facing message for errors that are safe to expose,
    /// or `None` for internal errors whose details should not be leaked.
    pub fn user_message(&self) -> Option<String> {
        match self {
            Self::ObjectNotFound { object_id, .. } => {
                Some(format!("Object {:?} not found", object_id))
            }
            Self::Expired { object_id, .. } => Some(format!("Object {:?} has expired", object_id)),
            Self::ObjectExists { object_id, .. } => {
                Some(format!("Object {:?} already exists", object_id))
            }
            // Same message as ObjectNotFound to avoid revealing that an object is unlisted
            Self::Unlisted { path, .. } => {
                let object_id = path.split('/').next().unwrap_or(path);
                Some(format!("Object {:?} not found", object_id))
            }
            Self::InvalidObjectId { object_id, .. } => {
                Some(format!("Invalid object ID {:?}", object_id))
            }
            Self::InvalidLinkPath { path, .. } => Some(format!("Invalid link path {:?}", path)),
            Self::DirectoryTraversal { .. }
            | Self::IOError { .. }
            | Self::ThumbnailError { .. }
            | Self::PayloadError { .. }
            | Self::StorageError { .. } => None,
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
        let metadata = fs::metadata(&storage_path)
            .await
            .with_context(|_| IOSnafu {
                path: storage_path.clone(),
            })?;

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
        ItemType::new(&self.storage_path, &self.metadata)
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

        let mut dir = fs::read_dir(&self.storage_path)
            .await
            .with_context(|_| IOSnafu {
                path: self.storage_path.clone(),
            })?;
        while let Some(entry) = dir.next_entry().await.with_context(|_| IOSnafu {
            path: self.storage_path.clone(),
        })? {
            if let Some(item) = DirListingItem::with_dir_entry(entry)
                .await
                .with_context(|_| IOSnafu {
                    path: self.storage_path.clone(),
                })?
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
    /// Image that can be displayed by the browser (and shown in the gallery)
    Image,
    /// File of other/unknown type
    File,
}

/// Lower bound on the expiry task's wait, so that an expired object that can't be removed
/// doesn't make the task spin. Expired objects are already hidden, so the delay is invisible.
const MIN_EXPIRY_WAIT: Duration = Duration::from_secs(1);
/// Upper bound on the expiry task's wait. The wait is computed from the wall clock but measured
/// on a monotonic timer, so this bounds the extra delay after a suspend or a clock change.
const MAX_EXPIRY_WAIT: Duration = Duration::from_secs(4 * 60 * 60);

/// Extensions of image formats that browsers can display.
const BROWSER_IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "avif", "gif", "svg"];

impl ItemType {
    pub fn new(path: &Path, metadata: &Metadata) -> Self {
        if metadata.is_dir() {
            ItemType::Directory
        } else if path.extension().is_some_and(|ext| {
            BROWSER_IMAGE_EXTENSIONS
                .iter()
                .any(|img_ext| ext.eq_ignore_ascii_case(img_ext))
        }) {
            ItemType::Image
        } else {
            ItemType::File
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ItemType::Directory => "directory",
            ItemType::Image => "image",
            ItemType::File => "file",
        }
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
    pub name: Arc<str>,
    /// Filesystem details of the object, or the error encountered while reading them
    /// (e.g. the target of a linked object no longer exists).
    pub item: Result<DirListingItem, std::io::Error>,
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
            item_type: ItemType::new(path, metadata),
            is_thumbnailable: is_thumbnailable(path, metadata),
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
        let mut dir = tokio::fs::read_dir(&full_path)
            .await
            .with_context(|_| IOSnafu {
                path: full_path.clone(),
            })?;
        let mut entries = Vec::new();

        while let Some(entry) = dir.next_entry().await.with_context(|_| IOSnafu {
            path: full_path.clone(),
        })? {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let file_type = entry.file_type().await.with_context(|_| IOSnafu {
                path: full_path.join(&name),
            })?;
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

pub struct AppData {
    config: Config,
    objects: RwLock<Storage<Object>>,
    // The RwLock not only protects the Storage object, but also the data stored on the filesystem
    thumbnails: CachedThumbnails,
    static_content_hash: String,
    download_base_url: String,
    full_download_base_url: String,
    admin_objects_base_url: String,
    owned_object_storage_root: PathBuf,
    /// Wakes up the expiry task, to rescan the objects or to check `expiry_quit`.
    /// `notify_one` keeps a permit when the task isn't waiting, so no wakeup is lost.
    expiry_notify: Notify,
    /// Set once by `shutdown()`, never cleared, so a later rescan can't override it.
    expiry_quit: AtomicBool,
    /// Handle of the expiry task, `None` if it wasn't started or has finished.
    /// Kept until the task finishes, so that a cancelled `shutdown()` can be resumed.
    expiry_task: Mutex<Option<JoinHandle<()>>>,
}

impl AppData {
    /// Loads the metadata and creates the app data.
    /// The storage dump task always runs, `spawn_expiry_task` controls only the expiry task.
    pub async fn with_config(
        config: Config,
        spawn_expiry_task: bool,
    ) -> std::result::Result<Arc<Self>, MainError> {
        let path = config.data_path.join("metadata.json");
        let objects = RwLock::new(
            Storage::new(path.clone())
                .await
                .context(MainIOSnafu { path })?,
        );
        let thumbnail_cache_size = config.thumbnail_cache_size;
        let static_content_hash = format!("{:X}", rng().next_u32());
        let download_base_url = config.download_url.trim_end_matches('/').to_owned();
        let full_download_base_url = format!(
            "{}{}",
            config
                .download_origin
                .as_deref()
                .unwrap_or_default()
                .trim_end_matches('/'),
            download_base_url
        );
        let admin_objects_base_url = format!("{}/objects", config.admin_url.trim_end_matches('/'));

        let owned_object_storage_root = config.data_path.join("owned_data");
        tokio::fs::create_dir_all(&owned_object_storage_root)
            .await
            .with_context(|_| MainIOSnafu {
                path: owned_object_storage_root.clone(),
            })?;

        let data = AppData {
            config,
            objects,
            thumbnails: CachedThumbnails::new(thumbnail_cache_size),
            static_content_hash,
            download_base_url,
            full_download_base_url,
            admin_objects_base_url,
            owned_object_storage_root,
            expiry_notify: Notify::new(),
            expiry_quit: AtomicBool::new(false),
            expiry_task: Mutex::new(None),
        };

        // Remove the upload temp area from possible previous failed uploads
        // Ignoring errors
        let _ = std::fs::remove_dir_all(data.get_upload_temp_path());

        let app = Arc::new(data);

        if spawn_expiry_task {
            let task_app = Arc::clone(&app);
            *app.expiry_task.lock().await =
                Some(tokio::spawn(async move { task_app.expiry_task().await }));
        }

        Ok(app)
    }

    pub fn get_download_base_url(&self) -> &str {
        &self.download_base_url
    }

    pub fn get_full_download_base_url(&self) -> &str {
        &self.full_download_base_url
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

    pub fn get_thumbnail_cache_stats(&self) -> CacheStats {
        self.thumbnails.cache_stats()
    }

    fn get_owned_object_storage_path(&self, object_id: &str) -> PathBuf {
        let mut path = self.owned_object_storage_root.clone();
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
            // RelativePath ignores leading and repeated slashes, so a sub-path decoded from
            // `%2F` can't escape to an absolute path, as it would with `PathBuf::push`.
            object_fs_path = RelativePath::new(subobject_path).to_path(object_fs_path);
        }
        drop(obj);

        // RelativePath also drops a trailing slash, so check it here, like the filesystem would.
        let trailing_slash = path.ends_with('/');
        let result = ResolvedObject::new(path, object_fs_path, &self.thumbnails, expires).await?;
        if trailing_slash && !result.metadata().is_dir() {
            return Err(std::io::ErrorKind::NotADirectory.into()).context(IOSnafu {
                path: result.into_storage_path(),
            });
        }
        Ok(result)
    }

    /// Reads the filesystem details of an object's data for use in a listing.
    async fn object_listing_item(
        &self,
        key: Arc<str>,
        obj: &Object,
    ) -> std::io::Result<DirListingItem> {
        let path = self.get_object_storage_path(&key, obj);
        let metadata = fs::metadata(&path).await?;
        let mut item = DirListingItem::with_metadata(&path, key, &metadata);
        item.expires = obj.expires;
        Ok(item)
    }

    pub async fn list_objects_admin(&self) -> Vec<AdminObjectInfo> {
        let mut result = Vec::new();

        for (key, obj) in self.objects.read().await.iter() {
            result.push(AdminObjectInfo {
                name: Arc::clone(key),
                item: self.object_listing_item(Arc::clone(key), obj).await,
                ownership: obj.ownership.clone(),
                unlisted_key: obj.unlisted_key.clone(),
                expires: obj.expires,
            });
        }

        result
    }

    pub async fn delete_object(&self, object_id: &str) -> AppDataResult<()> {
        let mut guard = self.objects.write().await;
        let obj = guard
            .remove(object_id)?
            .context(ObjectNotFoundSnafu { object_id })?;

        // The metadata is removed before the data, so the object never points at missing data
        // while running. The metadata file is written later by the storage's dump task, so
        // a crash shortly after can still leave the object around without its data.
        // Removing the data can take a while, so we only keep a read lock for it. That is still
        // enough to keep out other writers, and the object is already invisible to readers.
        let _guard = guard.downgrade();
        self.remove_object_data(object_id, &obj).await
    }

    /// Removes the data of an object that was already removed from the storage.
    /// Linked objects have no data of their own, so nothing is removed for them.
    async fn remove_object_data(&self, object_id: &str, obj: &Object) -> AppDataResult<()> {
        if matches!(obj.ownership, ObjectOwnership::Owned) {
            let path = self.get_owned_object_storage_path(object_id);
            match remove_file_or_directory(&path).await {
                Ok(()) => {}
                // The data is already gone, which is what we wanted.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    log::warn!(
                        "Data of object {object_id} was already missing at {}",
                        path.display()
                    );
                }
                Err(e) => return Err(e).context(IOSnafu { path }),
            }
        }
        Ok(())
    }

    /// Validate that an object ID is a single path component.
    /// Object IDs must be flat identifiers — slashes would create nested directories,
    /// and an empty ID, `.` or `..` would point at the owned data directory or its parent.
    fn validate_object_id(object_id: &str) -> AppDataResult<()> {
        if matches!(object_id, "" | "." | "..") || object_id.contains('/') {
            return InvalidObjectIdSnafu {
                object_id: object_id.to_string(),
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
            return InvalidLinkPathSnafu {
                path: link_path.to_string(),
            }
            .fail();
        }

        // Verify the path actually exists at creation time
        let fs_path = link_path.to_path(&self.config.linked_objects_root);
        tokio::fs::metadata(&fs_path)
            .await
            .with_context(|_| IOSnafu {
                path: fs_path.clone(),
            })?;

        let mut guard = self.objects.write().await;
        if !guard.create(
            Arc::clone(&object_id),
            Object {
                ownership: ObjectOwnership::Linked(link_path),
                expires,
                unlisted_key,
            },
        )? {
            return ObjectExistsSnafu { object_id }.fail();
        }
        Ok(())
    }

    async fn object_from_id<'a>(&'a self, id: &str) -> AppDataResult<RwLockReadGuard<'a, Object>> {
        RwLockReadGuard::try_map(self.objects.read().await, |objects| objects.get(id))
            .map_err(|_| ObjectNotFoundSnafu { object_id: id }.build())
    }

    pub async fn list_objects(&self) -> Vec<DirListingItem> {
        let mut result = Vec::new();

        for (key, obj) in self.objects.read().await.iter() {
            if obj.unlisted_key.is_some() {
                continue;
            }
            if obj.is_expired() {
                log::info!("Ignoring expired object {} in listing", key);
                continue;
            }
            match self.object_listing_item(Arc::clone(key), obj).await {
                Ok(item) => result.push(item),
                Err(e) => log::warn!(
                    "Skipping object {} with unreadable data at {:?}: {}",
                    key,
                    self.get_object_storage_path(key, obj),
                    e
                ),
            }
        }

        result
    }

    /// Modifies an object using the provided closure.
    pub async fn update_object(
        &self,
        object_id: &str,
        f: impl FnOnce(&mut Object),
    ) -> AppDataResult<()> {
        let updated = self.objects.write().await.update(object_id, f)?;
        ensure!(updated, ObjectNotFoundSnafu { object_id });
        Ok(())
    }

    pub fn signal_expiry_change(&self) {
        self.expiry_notify.notify_one();
    }

    pub async fn shutdown(&self) -> std::io::Result<()> {
        // The flag must be set before the wakeup, the task checks it after waking up.
        self.expiry_quit.store(true, Ordering::Release);
        self.expiry_notify.notify_one();

        let mut expiry_task = self.expiry_task.lock().await;
        if let Some(handle) = &mut *expiry_task {
            if let Err(e) = handle.await {
                log::error!("Expiry task failed: {e}");
            }
            *expiry_task = None;
        }
        drop(expiry_task);

        self.objects.write().await.shutdown().await
    }

    /// Goes through all objects in the storage and removes any expired entries,
    /// including deleting the files for owned expired entries.
    /// Returns the time of the next expiry.
    /// Errors during file and directory deletion are logged, but ignored.
    async fn delete_expired_and_get_next(&self) -> Option<DateTime<Utc>> {
        let mut objects = self.objects.write().await;
        let now = Utc::now();

        let removed = objects
            .extract_if(|_, obj| obj.is_expired_at(now))
            .unwrap_or_else(|e| {
                log::error!("Failed to remove expired objects: {e}");
                Vec::new()
            });

        // We only downgrade the lock while deleting the data, because it is used to protect
        // the files as well as the metadata. A read lock is enough to keep out other writers,
        // and the removed objects are already invisible to readers.
        let objects = objects.downgrade();
        for (id, obj) in removed {
            log::info!("Deleted expired object {id}");
            if let Err(e) = self.remove_object_data(&id, &obj).await {
                log::error!("Ignoring error while removing data of expired object {id}: {e}");
            }
        }

        objects.iter().filter_map(|(_, obj)| obj.expires).min()
    }

    async fn expiry_task(&self) {
        log::info!("Starting expiry task");
        while !self.expiry_quit.load(Ordering::Acquire) {
            let next_expiry = self.delete_expired_and_get_next().await;
            let now = Utc::now();

            let timeout_duration = next_expiry
                .map(|next_expiry| {
                    (next_expiry - now)
                        .to_std()
                        .unwrap_or(MIN_EXPIRY_WAIT)
                        .clamp(MIN_EXPIRY_WAIT, MAX_EXPIRY_WAIT)
                })
                .unwrap_or(MAX_EXPIRY_WAIT);

            // Woken up either by a rescan or quit request, or by the timeout when an entry
            // should be expiring by now. Either way we loop to check the quit flag and rescan.
            let _ = timeout(timeout_duration, self.expiry_notify.notified()).await;
        }
        log::info!("Finished expiry task");
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
            .with_context(|_| IOSnafu {
                path: upload_temp_path.clone(),
            })?;
        let (f, temp_path) = tempfile::NamedTempFile::new_in(&upload_temp_path)
            .with_context(|_| IOSnafu {
                path: upload_temp_path,
            })?
            .into_parts();
        let mut f = tokio::fs::File::from_std(f);

        pin_mut!(content);

        // 3. Copy the content to file, this might take a long time
        while let Some(block) = content.next().await {
            let block = block.context(PayloadSnafu)?;
            f.write_all(&block).await.with_context(|_| IOSnafu {
                path: temp_path.to_path_buf(),
            })?;
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
        )? {
            return ObjectExistsSnafu { object_id }.fail();
        }

        let dest = self.get_owned_object_storage_path(&object_id);
        tokio::fs::rename(temp_path.keep().unwrap(), &dest)
            .await
            .with_context(|_| IOSnafu { path: dest })?; // TODO: What happens if this fails?

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
    use super::*;

    /// Creates an AppData with a test config in a temp directory, without
    /// the expiry task so tests can call internal methods without racing it.
    async fn test_app() -> (tempfile::TempDir, Arc<AppData>) {
        test_app_with_expiry_task(false).await
    }

    async fn test_app_with_expiry_task(
        spawn_expiry_task: bool,
    ) -> (tempfile::TempDir, Arc<AppData>) {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            bind_address: "localhost".into(),
            bind_port: 0,
            data_path: dir.path().to_path_buf(),
            linked_objects_root: dir.path().to_path_buf(),
            download_url: "/download".into(),
            download_origin: None,
            admin_url: "/admin".into(),
            app_name: "test".into(),
            display_timezone: chrono_tz::UTC,
            thumbnail_cache_size: 1024,
        };
        let app = AppData::with_config(config, spawn_expiry_task)
            .await
            .unwrap();
        (dir, app)
    }

    mod expiry {
        use super::super::*;
        use super::{test_app, test_app_with_expiry_task};
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

        #[tokio::test]
        async fn delete_expired_removes_expired_object() {
            let (_dir, app) = test_app().await;

            let past = Utc::now() - Duration::hours(1);
            let future = Utc::now() + Duration::hours(1);

            {
                let mut guard = app.objects.write().await;
                guard
                    .create("expired_obj".into(), make_object(Some(past)))
                    .unwrap();
                guard
                    .create("future_obj".into(), make_object(Some(future)))
                    .unwrap();
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
            let (dir, app) = test_app().await;
            // Create a file so the linked path resolves
            std::fs::write(dir.path().join("testfile"), b"hello").unwrap();

            let past = Utc::now() - Duration::hours(1);
            {
                let mut guard = app.objects.write().await;
                guard
                    .create(
                        "expired_link".into(),
                        Object {
                            ownership: ObjectOwnership::Linked("testfile".into()),
                            expires: Some(past),
                            unlisted_key: None,
                        },
                    )
                    .unwrap();
            }

            let result = app.resolve_object("expired_link".to_owned(), None).await;
            assert!(result.is_err());
            assert!(
                matches!(result.unwrap_err(), AppDataError::Expired { object_id, .. } if object_id == "expired_link")
            );
        }

        #[tokio::test]
        async fn delete_expired_cleans_up_owned_files() {
            let (dir, app) = test_app().await;

            let owned_data = dir.path().join("owned_data");
            let expired_file = owned_data.join("expired_obj");
            let future_file = owned_data.join("future_obj");
            std::fs::write(&expired_file, b"expired").unwrap();
            std::fs::write(&future_file, b"future").unwrap();

            let past = Utc::now() - Duration::hours(1);
            let future = Utc::now() + Duration::hours(1);

            {
                let mut guard = app.objects.write().await;
                guard
                    .create("expired_obj".into(), make_object(Some(past)))
                    .unwrap();
                guard
                    .create("future_obj".into(), make_object(Some(future)))
                    .unwrap();
            }

            app.delete_expired_and_get_next().await;

            assert!(!expired_file.exists());
            assert!(future_file.exists());
        }

        #[tokio::test]
        async fn delete_expired_does_not_dirty_when_nothing_expired() {
            let (_dir, app) = test_app().await;

            let future = Utc::now() + Duration::hours(1);
            {
                let mut guard = app.objects.write().await;
                guard
                    .create("future_obj".into(), make_object(Some(future)))
                    .unwrap();
            }

            let snapshots = app.objects.read().await.subscribe_snapshots();
            app.delete_expired_and_get_next().await;
            assert!(!snapshots.has_changed().unwrap());
        }

        #[tokio::test(start_paused = true)]
        async fn shutdown_dumps_dirty_storage() {
            let (dir, app) = test_app().await;

            // The first change is written right away, the second one would wait for
            // the cooldown, so only the shutdown can get it to disk in time.
            app.objects
                .write()
                .await
                .create("first".into(), make_object(None))
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            app.objects
                .write()
                .await
                .create("second".into(), make_object(None))
                .unwrap();

            let start = tokio::time::Instant::now();
            app.shutdown().await.unwrap();
            assert!(start.elapsed() < crate::storage::DUMP_COOLDOWN);

            let contents: std::collections::HashMap<String, serde_json::Value> =
                serde_json::from_slice(&std::fs::read(dir.path().join("metadata.json")).unwrap())
                    .unwrap();
            assert!(contents.contains_key("first"));
            assert!(contents.contains_key("second"));
        }

        #[tokio::test]
        async fn modification_after_shutdown_returns_503() {
            let (dir, app) = test_app().await;
            std::fs::write(dir.path().join("testfile"), b"hello").unwrap();
            app.shutdown().await.unwrap();

            let err = app
                .create_linked_object("obj".into(), "testfile".into(), None, None)
                .await
                .unwrap_err();
            assert!(matches!(
                err,
                AppDataError::StorageError {
                    source: WriteError::StorageAlreadyShutDown { .. },
                    ..
                }
            ));
            assert!(err.status_code() == StatusCode::SERVICE_UNAVAILABLE);
            assert!(app.objects.read().await.get("obj").is_none());
        }

        /// Polls the shutdown once, so that it signals the expiry task and starts waiting for it.
        async fn start_shutdown(
            shutdown: &mut std::pin::Pin<&mut impl Future<Output = std::io::Result<()>>>,
        ) {
            tokio::select! {
                biased;
                _ = shutdown.as_mut() => panic!("Shutdown finished without waiting for the expiry task"),
                _ = std::future::ready(()) => {}
            }
        }

        #[tokio::test(start_paused = true)]
        async fn rescan_after_shutdown_does_not_prevent_quit() {
            let (_dir, app) = test_app_with_expiry_task(true).await;
            // Let the expiry task do its initial scan and start waiting.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let shutdown = app.shutdown();
            tokio::pin!(shutdown);
            start_shutdown(&mut shutdown).await;
            app.signal_expiry_change();

            tokio::time::timeout(std::time::Duration::from_secs(60), shutdown)
                .await
                .expect("Shutdown hangs")
                .unwrap();
        }

        #[tokio::test(start_paused = true)]
        async fn shutdown_during_scan_is_not_lost() {
            let (_dir, app) = test_app_with_expiry_task(true).await;

            // Blocks the expiry task inside its scan, so it isn't waiting for the signals.
            let guard = app.objects.write().await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let shutdown = app.shutdown();
            tokio::pin!(shutdown);
            start_shutdown(&mut shutdown).await;
            app.signal_expiry_change();
            drop(guard);

            tokio::time::timeout(std::time::Duration::from_secs(60), shutdown)
                .await
                .expect("Shutdown hangs")
                .unwrap();
        }

        #[tokio::test]
        async fn shutdown_skips_dump_when_clean() {
            let (dir, app) = test_app().await;

            // Storage is clean (no mutations)
            app.shutdown().await.unwrap();

            // metadata.json should not exist (never written since storage was empty and clean)
            assert!(!dir.path().join("metadata.json").exists());
        }
    }

    mod delete {
        use super::super::*;
        use super::test_app;

        fn owned_object() -> Object {
            Object {
                ownership: ObjectOwnership::Owned,
                expires: None,
                unlisted_key: None,
            }
        }

        #[tokio::test]
        async fn removes_owned_data() {
            let (dir, app) = test_app().await;
            let owned_data = dir.path().join("owned_data");
            std::fs::write(owned_data.join("obj"), b"hello").unwrap();
            app.objects
                .write()
                .await
                .create("obj".into(), owned_object())
                .unwrap();

            app.delete_object("obj").await.unwrap();

            assert!(app.objects.read().await.get("obj").is_none());
            assert!(!owned_data.join("obj").exists());
        }

        #[tokio::test]
        async fn succeeds_when_owned_data_is_missing() {
            let (_dir, app) = test_app().await;
            app.objects
                .write()
                .await
                .create("obj".into(), owned_object())
                .unwrap();

            app.delete_object("obj").await.unwrap();

            assert!(app.objects.read().await.get("obj").is_none());
        }
    }

    mod browse_linked {
        use super::test_app;

        #[tokio::test]
        async fn lists_files_and_directories() {
            let (dir, app) = test_app().await;
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
            let (dir, app) = test_app().await;
            std::fs::write(dir.path().join("aaa_file"), "").unwrap();
            std::fs::create_dir(dir.path().join("zzz_dir")).unwrap();

            let entries = app.browse_linked_directory("").await.unwrap();
            assert!(entries[0].is_dir, "directories should come first");
        }

        #[tokio::test]
        async fn rejects_directory_traversal() {
            let (_dir, app) = test_app().await;

            let result = app.browse_linked_directory("..").await;
            assert!(result.is_err());

            let result = app.browse_linked_directory("foo/../..").await;
            assert!(result.is_err());

            let result = app.browse_linked_directory("../etc").await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn browses_subdirectory() {
            let (dir, app) = test_app().await;
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            std::fs::write(dir.path().join("sub").join("nested.txt"), "").unwrap();

            let entries = app.browse_linked_directory("sub").await.unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].name, "nested.txt");
        }

        #[tokio::test]
        async fn nonexistent_path_returns_error() {
            let (_dir, app) = test_app().await;
            let result = app.browse_linked_directory("no_such_dir").await;
            assert!(result.is_err());
        }
    }

    mod listing {
        use super::super::*;
        use super::test_app;

        /// Registers a linked object `ok_link` pointing at an existing file and
        /// `broken_link` pointing at a file that doesn't exist.
        async fn add_ok_and_broken_links(dir: &tempfile::TempDir, app: &AppData) {
            std::fs::write(dir.path().join("existing.txt"), b"hello").unwrap();
            let mut guard = app.objects.write().await;
            for (id, target) in [("ok_link", "existing.txt"), ("broken_link", "missing.txt")] {
                guard
                    .create(
                        id.into(),
                        Object {
                            ownership: ObjectOwnership::Linked(target.into()),
                            expires: None,
                            unlisted_key: None,
                        },
                    )
                    .unwrap();
            }
        }

        #[tokio::test]
        async fn public_listing_skips_missing_objects() {
            let (dir, app) = test_app().await;
            add_ok_and_broken_links(&dir, &app).await;

            let items = app.list_objects().await;
            let names: Vec<&str> = items.iter().map(|item| item.name.as_ref()).collect();
            assert_eq!(names, ["ok_link"]);
        }

        #[tokio::test]
        async fn admin_listing_reports_missing_objects() {
            let (dir, app) = test_app().await;
            add_ok_and_broken_links(&dir, &app).await;

            let items = app.list_objects_admin().await;
            assert_eq!(items.len(), 2);

            let ok = items.iter().find(|i| i.name.as_ref() == "ok_link").unwrap();
            assert_eq!(ok.item.as_ref().unwrap().file_size, 5);

            let broken = items
                .iter()
                .find(|i| i.name.as_ref() == "broken_link")
                .unwrap();
            assert_eq!(
                broken.item.as_ref().unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );
        }
    }

    mod resolve {
        use super::test_app;

        #[tokio::test]
        async fn absolute_subobject_path_stays_inside_object() {
            let (dir, app) = test_app().await;
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            let secret = dir.path().join("secret.txt");
            std::fs::write(&secret, b"secret").unwrap();
            app.create_linked_object("lnk".into(), "sub".into(), None, None)
                .await
                .unwrap();

            // "lnk//tmp/.../secret.txt", as it arrives with the slashes encoded as %2F
            let path = format!("lnk/{}", secret.display());
            let Err(e) = app.resolve_object(path, None).await else {
                panic!("resolved a path outside of the object");
            };
            assert_eq!(e.status_code(), actix_web::http::StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn resolves_file_in_linked_directory() {
            let (dir, app) = test_app().await;
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            std::fs::write(dir.path().join("sub").join("file.txt"), b"hello").unwrap();
            app.create_linked_object("lnk".into(), "sub".into(), None, None)
                .await
                .unwrap();

            let resolved = app
                .resolve_object("lnk/file.txt".into(), None)
                .await
                .unwrap();
            assert_eq!(resolved.storage_path(), dir.path().join("sub/file.txt"));
        }

        #[test_case::test_case("lnk/file.txt/"; "trailing slash on file in directory")]
        #[test_case::test_case("lnk/file.txt//"; "double trailing slash on file in directory")]
        #[test_case::test_case("lnk/file.txt/x"; "path through file")]
        #[test_case::test_case("file/"; "trailing slash on linked file")]
        #[tokio::test]
        async fn file_used_as_directory_is_not_found(path: &str) {
            let (dir, app) = test_app().await;
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            std::fs::write(dir.path().join("sub").join("file.txt"), b"hello").unwrap();
            app.create_linked_object("lnk".into(), "sub".into(), None, None)
                .await
                .unwrap();
            app.create_linked_object("file".into(), "sub/file.txt".into(), None, None)
                .await
                .unwrap();

            let Err(e) = app.resolve_object(path.into(), None).await else {
                panic!("resolved a file as a directory");
            };
            assert_eq!(e.status_code(), actix_web::http::StatusCode::NOT_FOUND);
        }

        #[test_case::test_case("lnk/"; "linked directory")]
        #[test_case::test_case("lnk/inner/"; "subdirectory")]
        #[tokio::test]
        async fn trailing_slash_on_directory_resolves(path: &str) {
            let (dir, app) = test_app().await;
            std::fs::create_dir_all(dir.path().join("sub").join("inner")).unwrap();
            app.create_linked_object("lnk".into(), "sub".into(), None, None)
                .await
                .unwrap();

            let resolved = app.resolve_object(path.into(), None).await.unwrap();
            assert!(resolved.metadata().is_dir());
        }
    }

    #[tokio::test]
    async fn startup_creates_owned_data_directory() {
        let (dir, _app) = test_app().await;
        assert!(dir.path().join("owned_data").is_dir());
    }

    mod validate_object_id {
        use super::super::*;
        use test_case::test_case;

        #[test_case(""; "empty")]
        #[test_case("."; "dot")]
        #[test_case(".."; "dotdot")]
        #[test_case("foo/bar"; "slash")]
        #[test_case("/foo"; "leading slash")]
        fn rejects(object_id: &str) {
            assert!(matches!(
                AppData::validate_object_id(object_id),
                Err(AppDataError::InvalidObjectId { .. })
            ));
        }

        #[test_case("foo"; "plain")]
        #[test_case("..."; "three dots")]
        #[test_case(".hidden"; "leading dot")]
        #[test_case("foo..bar"; "embedded dotdot")]
        fn accepts(object_id: &str) {
            AppData::validate_object_id(object_id).unwrap();
        }
    }

    mod item_type {
        use super::super::*;

        #[test]
        fn detects_item_types() {
            let dir = tempfile::tempdir().unwrap();
            let sub = dir.path().join("photos.jpg");
            std::fs::create_dir(&sub).unwrap();
            for (name, expected) in [
                ("a.jpg", ItemType::Image),
                ("b.JPEG", ItemType::Image),
                ("c.svg", ItemType::Image),
                ("d.tiff", ItemType::File),
                ("e.pdf", ItemType::File),
                ("jpg", ItemType::File),
                ("f.txt", ItemType::File),
            ] {
                let path = dir.path().join(name);
                std::fs::write(&path, b"").unwrap();
                let metadata = std::fs::metadata(&path).unwrap();
                assert_eq!(ItemType::new(&path, &metadata), expected, "{name}");
            }
            let metadata = std::fs::metadata(&sub).unwrap();
            assert_eq!(ItemType::new(&sub, &metadata), ItemType::Directory);
        }
    }
}
