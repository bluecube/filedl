use std::{
    fs::Metadata,
    hash::{Hash, Hasher},
    path::PathBuf,
    time::SystemTime,
};

use actix_web::web::Bytes;
use quick_cache::{Weighter, sync::Cache};
use serde::Serialize;

use super::{ThumbnailResult as Result, ThumbnailType, create_thumbnail};

/// Describes a cached rendered thumbnail
#[derive(Clone, Hash, Debug, PartialEq, Eq)]
struct CacheKey {
    // First three arguments deal with the source file:
    path: PathBuf,
    file_size: u64,
    modtime: Option<SystemTime>,
    // TODO: Reuse AppData::CacheSourceKey?

    // Properties of the final thumbnail
    resolution: (u32, u32),
    thumbnail_type: ThumbnailType,
}

impl CacheKey {
    fn new(
        path: PathBuf,
        metadata: &Metadata,
        resolution: (u32, u32),
        thumbnail_type: ThumbnailType,
    ) -> Self {
        CacheKey {
            path,
            file_size: metadata.len(),
            modtime: metadata.modified().ok(),

            resolution,
            thumbnail_type,
        }
    }

    fn hash_string(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{:X}", hasher.finish())
    }
}
#[derive(Debug, Clone)]
struct BytesWeighter();

impl<K> Weighter<K, Bytes> for BytesWeighter {
    fn weight(&self, _key: &K, val: &Bytes) -> u64 {
        val.len() as u64
    }
}

#[derive(Debug)]
pub struct CachedThumbnails {
    cache: Cache<CacheKey, Bytes, BytesWeighter>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CacheStats {
    pub count: usize,
    pub used_size: u64,
    pub hits: u64,
    pub misses: u64,
}

impl CachedThumbnails {
    pub fn new(max_size: u64) -> Self {
        const EXPECTED_THUMBNAIL_SIZE: u64 = 3 * 1024;
        CachedThumbnails {
            cache: Cache::with_weighter(
                (max_size / EXPECTED_THUMBNAIL_SIZE) as usize,
                max_size,
                BytesWeighter(),
            ),
        }
    }

    pub async fn get(
        &self,
        file: PathBuf,
        metadata: &Metadata,
        resolution: (u32, u32),
        thumbnail_type: ThumbnailType,
    ) -> Result<(Bytes, String)> {
        let key = CacheKey::new(file, metadata, resolution, thumbnail_type);
        let hash_str = key.hash_string();

        let thumbnail = match self.cache.get_value_or_guard_async(&key).await {
            Ok(thumbnail) => thumbnail,
            Err(guard) => {
                let thumbnail = spawn_create_thumbnail(key).await?;
                guard.insert(thumbnail.clone()).unwrap();
                thumbnail
            }
        };

        Ok((thumbnail, hash_str))
    }

    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            count: self.cache.len(),
            used_size: self.cache.weight(),
            hits: self.cache.hits(),
            misses: self.cache.misses(),
        }
    }
}

/// Wraps create_thumbnail, making it async, without blocking the Tokio runtime.
async fn spawn_create_thumbnail(key: CacheKey) -> Result<Bytes> {
    let path = key.path;
    let resolution = key.resolution;
    let thumbnail_type = key.thumbnail_type;

    tokio_rayon::spawn(move || create_thumbnail(&path, resolution, thumbnail_type)).await
}
