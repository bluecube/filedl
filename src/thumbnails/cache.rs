use std::{
    fs::Metadata,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime},
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

#[derive(Clone, Debug, Serialize)]
pub struct WorstRenderTime {
    path: PathBuf,
    resolution: (u32, u32),
    thumbnail_type: ThumbnailType,
    render_time: f64,
}

#[derive(Default, Clone, Debug)]
struct RenderTimeStats {
    pub smoothed_render_time: f64,
    pub slowest_threshold: f64,
    pub last_slowest: Option<WorstRenderTime>,
}

impl RenderTimeStats {
    fn update(
        &mut self,
        path: PathBuf,
        resolution: (u32, u32),
        thumbnail_type: ThumbnailType,
        render_time: Duration,
    ) {
        const ALPHA: f64 = 0.01;
        let render_time = render_time.as_secs_f64();

        if let Some(_) = self.last_slowest {
            self.smoothed_render_time =
                self.smoothed_render_time * (1.0 - ALPHA) + render_time * ALPHA;
            let slowest_threshold = self.slowest_threshold * (1.0 - ALPHA) + render_time * ALPHA;

            if render_time > slowest_threshold {
                self.slowest_threshold = render_time;
                self.last_slowest = Some(WorstRenderTime {
                    path,
                    resolution,
                    thumbnail_type,
                    render_time,
                })
            } else {
                self.slowest_threshold = slowest_threshold;
            }
        } else {
            self.smoothed_render_time = render_time;
            self.slowest_threshold = render_time;
            self.last_slowest = Some(WorstRenderTime {
                path,
                resolution,
                thumbnail_type,
                render_time,
            })
        }
    }
}

#[derive(Debug)]
pub struct CachedThumbnails {
    cache: Cache<CacheKey, Bytes, BytesWeighter>,
    render_time_stats: Mutex<RenderTimeStats>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CacheStats {
    pub count: usize,
    pub used_size: u64,
    pub hits: u64,
    pub misses: u64,

    pub smoothed_render_time: f64,
    pub last_slowest: Option<WorstRenderTime>,
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
            render_time_stats: Mutex::new(RenderTimeStats::default()),
        }
    }

    pub async fn get(
        &self,
        file: PathBuf,
        metadata: &Metadata,
        resolution: (u32, u32),
        thumbnail_type: ThumbnailType,
    ) -> Result<(Bytes, String)> {
        let key = CacheKey::new(file.clone(), metadata, resolution, thumbnail_type);
        let hash_str = key.hash_string();

        let thumbnail = match self.cache.get_value_or_guard_async(&key).await {
            Ok(thumbnail) => thumbnail,
            Err(guard) => {
                let (thumbnail, duration) = spawn_create_thumbnail(key).await?;
                guard.insert(thumbnail.clone()).unwrap();
                self.render_time_stats.lock().unwrap().update(
                    file,
                    resolution,
                    thumbnail_type,
                    duration,
                );
                thumbnail
            }
        };

        Ok((thumbnail, hash_str))
    }

    pub fn cache_stats(&self) -> CacheStats {
        let render_time_stats = self.render_time_stats.lock().unwrap().clone();
        CacheStats {
            count: self.cache.len(),
            used_size: self.cache.weight(),
            hits: self.cache.hits(),
            misses: self.cache.misses(),

            smoothed_render_time: render_time_stats.smoothed_render_time,
            last_slowest: render_time_stats.last_slowest,
        }
    }
}

/// Wraps create_thumbnail, making it async, without blocking the Tokio runtime.
async fn spawn_create_thumbnail(key: CacheKey) -> Result<(Bytes, Duration)> {
    let path = key.path;
    let resolution = key.resolution;
    let thumbnail_type = key.thumbnail_type;

    tokio_rayon::spawn(move || create_thumbnail(&path, resolution, thumbnail_type)).await
}
