use crate::{error::Result, util::simple_spawn_blocking};
use actix_web::web::Bytes;
use assert2::assert;
use image::{
    imageops, DynamicImage, GenericImageView, ImageBuffer, ImageFormat, Pixel, Rgb, RgbImage,
};
use lru::LruCache;
use mime::Mime;
use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    fs::Metadata,
    hash::{Hash, Hasher},
    io::Cursor,
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::atomic::AtomicU64,
    time::SystemTime,
};
use tokio::sync::{broadcast, Mutex, MutexGuard};

/// Describes a cached rendered thumbnail
#[derive(Clone, Hash, Debug, PartialEq, Eq)]
struct CacheKey {
    // First three arguments deal with the source file:
    path: PathBuf,
    file_size: u64,
    modtime: Option<SystemTime>,

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

/// Internal part of the thumbnail cache that is protected by the mutex.
#[derive(Debug)]
struct Locked {
    cache: LruCache<CacheKey, Option<Bytes>>,
    used_size: usize,
}

impl Locked {
    /// Makes space in the cache for size bytes, so that the size of cached data is
    /// less than or equal to max_size.
    /// size must be less than or equal to max_size
    fn make_space(&mut self, size: usize, max_size: usize) {
        assert!(size <= max_size);

        while self.used_size + size > max_size {
            let (_, evicted_thumbnail) = self.cache.pop_lru().expect("cache should be non-empty");
            self.used_size -= evicted_thumbnail.map_or(0, |t| t.len());
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ThumbnailType {
    #[default]
    Jpeg,
    Avif,
}

impl Display for ThumbnailType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.serialize(f)
    }
}

impl ThumbnailType {
    pub fn mime(&self) -> Mime {
        match self {
            ThumbnailType::Jpeg => mime::IMAGE_JPEG,
            ThumbnailType::Avif => "image/avif".parse().unwrap(),
        }
    }

    fn image_output_format(&self) -> image::ImageOutputFormat {
        match self {
            ThumbnailType::Jpeg => image::ImageOutputFormat::Jpeg(85),
            ThumbnailType::Avif => image::ImageOutputFormat::Avif,
        }
    }
}

#[derive(Debug)]
pub struct CachedThumbnails {
    locked: Mutex<Locked>,
    updates: broadcast::Sender<(CacheKey, std::result::Result<Bytes, ()>)>,
    max_size: usize,

    hits: AtomicU64,
    hits_with_wait: AtomicU64,
    misses: AtomicU64,
    wait_lags: AtomicU64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CacheStats {
    pub count: usize,
    pub used_size: usize,
    pub hits: u64,
    pub hits_with_wait: u64,
    pub misses: u64,
    pub wait_lags: u64,
}

impl CachedThumbnails {
    pub fn new(max_size: usize) -> Self {
        CachedThumbnails {
            locked: Mutex::new(Locked {
                cache: LruCache::unbounded(), // Cache size is managed manually, based on size, not count
                used_size: 0,
            }),
            updates: broadcast::Sender::new(8.max(num_cpus::get() * 2)),
            max_size,

            hits: AtomicU64::new(0),
            hits_with_wait: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            wait_lags: AtomicU64::new(0),
        }
    }

    pub async fn get(
        &self,
        file: PathBuf,
        metadata: &Metadata,
        resolution: (u32, u32),
        thumbnail_type: ThumbnailType,
    ) -> Result<(Bytes, String)> {
        // Must be mutable because of the spawn_blocking trick below
        let key = CacheKey::new(file, metadata, resolution, thumbnail_type);

        let hash = key.hash_string();
        {
            let mut locked = self.locked.lock().await;

            match locked.cache.get(&key) {
                Some(Some(thumbnail)) => {
                    // Found existing thumbnail
                    let thumbnail = Bytes::clone(thumbnail);
                    self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    Ok((thumbnail, hash))
                }
                Some(None) => {
                    // Thumbnail is being created by other task
                    self.hits_with_wait
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    Ok((self.wait_for_thumbnail(key, locked).await?, hash))
                }
                None => {
                    // Thumbnail is missing, we need to create it
                    self.misses
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    Ok((self.create_and_cache_thumbnail(key, locked).await?, hash))
                }
            }
        }
    }

    async fn wait_for_thumbnail<'a>(
        &self,
        key: CacheKey,
        locked: MutexGuard<'a, Locked>,
    ) -> Result<Bytes> {
        // Subscribing the receiver while the lock is still held means the update will not
        // happen before we're subscribed
        let mut receiver = self.updates.subscribe();
        drop(locked);

        loop {
            let (updated_key, updated_result) = receiver.recv().await.inspect_err(|_| {
                self.wait_lags
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })?;
            if updated_key == key {
                return updated_result.map_err(|_| crate::error::FiledlError::ThumbnailUpdateError);
            }
        }
    }

    /// Create the thumbnail in a background task,
    async fn create_and_cache_thumbnail<'a>(
        &self,
        key: CacheKey,
        mut locked: MutexGuard<'a, Locked>,
    ) -> Result<Bytes> {
        // Write a placeholder into the cache.
        assert!(
            locked.cache.put(key.clone(), None).is_none(),
            "At this point the guard is still locked, so we know we're not overwriting anything"
        );
        drop(locked);

        let (thumbnail_result, key) = spawn_create_thumbnail(key).await;
        self.store_cached_thumbnail(&key, &thumbnail_result).await;
        self.send_thumbnail_update(key, &thumbnail_result);

        thumbnail_result
    }

    fn send_thumbnail_update(&self, key: CacheKey, thumbnail_result: &Result<Bytes>) {
        let _ = self.updates.send((
            key,
            match thumbnail_result {
                Ok(ref thumbnail) => Ok(Bytes::clone(thumbnail)),
                Err(_) => Err(()),
            },
        ));
    }

    async fn store_cached_thumbnail(&self, key: &CacheKey, thumbnail_result: &Result<Bytes>) {
        let mut locked = self.locked.lock().await;

        match thumbnail_result {
            Ok(ref thumbnail) if thumbnail.len() < self.max_size / 2 => {
                locked.make_space(thumbnail.len(), self.max_size);
                locked.used_size += thumbnail.len();

                assert!(
                    locked.cache.put(key.clone(), Some(Bytes::clone(thumbnail))) == Some(None),
                    "Only the placeholder should be stored in the cache for this entry"
                );
            }
            _ => {
                assert!(
                    locked.cache.pop(key) == Some(None),
                    "Only the placeholder should be stored in the cache for this entry"
                );
            }
        };
    }

    pub async fn cache_stats(&self) -> CacheStats {
        let locked = self.locked.lock().await;
        CacheStats {
            count: locked.cache.len(),
            used_size: locked.used_size,
            hits: self.hits.load(std::sync::atomic::Ordering::Relaxed),
            hits_with_wait: self
                .hits_with_wait
                .load(std::sync::atomic::Ordering::Relaxed),
            misses: self.misses.load(std::sync::atomic::Ordering::Relaxed),
            wait_lags: self.wait_lags.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

/// Creates the thumbnail for a given path, resolution and type.
pub fn create_thumbnail(
    file: &Path,
    resolution: (u32, u32),
    thumbnail_type: ThumbnailType,
) -> Result<Bytes> {
    let img = open_image(file)?;
    let orientation = get_orientation(file)?;

    // TODO: Fix orientation for non-square non-centered crops
    let crop_coords = crop_coordinates(img.dimensions(), resolution);

    // TODO: Don't hardcode background color
    let rgb_img = normalize_layers(img, [0xDA, 0xE1, 0xE4].into());
    let resized = crop_and_resize(rgb_img, crop_coords, resolution);
    let resized_and_reoriented = fix_orientation(resized, orientation);

    let mut bytes: Vec<u8> = Vec::new();
    resized_and_reoriented.write_to(
        &mut Cursor::new(&mut bytes),
        thumbnail_type.image_output_format(),
    )?;
    Ok(bytes.into())
}

/// Wraps create_thumbnail, making the cache creating async, without blocking
/// the Tokio runtime.
/// Passes the cache key through to avoid cloning (because passing a reference into
/// the spawned task is not possible).
async fn spawn_create_thumbnail(mut key: CacheKey) -> (Result<Bytes>, CacheKey) {
    // TODO: Spawn in rayon thread pool
    let path = key.path;
    let resolution = key.resolution;
    let thumbnail_type = key.thumbnail_type;

    let (thumbnail_result, path) = simple_spawn_blocking(move || {
        let thumbnail = create_thumbnail(&path, resolution, thumbnail_type);
        (thumbnail, path)
    })
    .await;

    key.path = path;

    (thumbnail_result, key)
}

/// Returns a hash describing the source image, if it is thumbnailable,
/// otherwise returns None.
pub fn is_thumbnailable(path: &Path) -> bool {
    let Some(filename) = path.file_name() else {
        return false;
    };
    let Some(filename) = filename.to_str() else {
        return false;
    };
    let Some((_, extension)) = filename.rsplit_once('.') else {
        return false;
    };
    let Some(format) = ImageFormat::from_extension(extension) else {
        return false;
    };

    format.can_read()
}

fn open_image(path: &Path) -> Result<DynamicImage> {
    let mut reader = image::io::Reader::open(path)?;
    reader.no_limits();
    Ok(reader.decode()?)
}

fn crop_and_resize(
    img: RgbImage,
    crop_coords: (u32, u32, u32, u32),
    new_size: (u32, u32),
) -> RgbImage {
    use fast_image_resize::{CropBox, FilterType, Image, PixelType, ResizeAlg, Resizer};

    let src_image = Image::from_vec_u8(
        NonZeroU32::new(img.width()).unwrap(),
        NonZeroU32::new(img.height()).unwrap(),
        img.into_raw(),
        PixelType::U8x3,
    )
    .unwrap();

    // Create container for data of destination image
    let mut dst_image = Image::new(
        NonZeroU32::new(new_size.0).unwrap(),
        NonZeroU32::new(new_size.1).unwrap(),
        PixelType::U8x3,
    );

    let mut src_view = src_image.view();
    src_view
        .set_crop_box(CropBox {
            left: crop_coords.0,
            top: crop_coords.1,
            width: NonZeroU32::new(crop_coords.2)
                .expect("Guaranteed to succeed by crop_coordinates()"),
            height: NonZeroU32::new(crop_coords.3)
                .expect("Guaranteed to succeed by crop_coordinates()"),
        })
        .expect("Guaranteed to succeed by crop_coordinates()");

    // Get mutable view of destination image data
    let mut dst_view = dst_image.view_mut();

    // Create Resizer instance and resize source image
    // into buffer of destination image
    let mut resizer = Resizer::new(ResizeAlg::Convolution(FilterType::Lanczos3));

    resizer.resize(&src_view, &mut dst_view).unwrap();

    RgbImage::from_vec(new_size.0, new_size.1, dst_image.into_vec()).unwrap()
}

fn get_orientation(path: &Path) -> Result<u32> {
    let file = std::fs::File::open(path)?;
    let mut bufreader = std::io::BufReader::new(file);
    let exifreader = exif::Reader::new();
    let Ok(exif_tags) = exifreader.read_from_container(&mut bufreader) else {
        return Ok(1);
    };

    Ok(
        match exif_tags.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
            Some(orientation) => match orientation.value.get_uint(0) {
                Some(v @ 1..=8) => v,
                _ => 1,
            },
            None => 1,
        },
    )
}

fn fix_orientation<Px: 'static + Pixel>(
    mut img: ImageBuffer<Px, Vec<Px::Subpixel>>,
    orientation: u32,
) -> ImageBuffer<Px, Vec<Px::Subpixel>> {
    match orientation {
        1 => img,
        2 => {
            imageops::flip_horizontal_in_place(&mut img);
            img
        }
        3 => {
            imageops::rotate180_in_place(&mut img);
            img
        }
        4 => {
            imageops::flip_vertical_in_place(&mut img);
            img
        }
        5 => {
            imageops::flip_horizontal_in_place(&mut img);
            imageops::rotate270(&img)
        }
        6 => imageops::rotate90(&img),
        7 => {
            imageops::flip_horizontal_in_place(&mut img);
            imageops::rotate90(&img)
        }
        8 => imageops::rotate270(&img),
        _ => unreachable!(),
    }
}

fn normalize_layers(img: DynamicImage, background_color: Rgb<u8>) -> RgbImage {
    if img.color().has_alpha() {
        blend_background(img.into_rgba8(), background_color)
    } else {
        img.into_rgb8()
    }
}

fn blend_background<Px>(
    img: ImageBuffer<Px, Vec<Px::Subpixel>>,
    background_color: Rgb<u8>,
) -> RgbImage
where
    Px: Pixel,
    <Px as image::Pixel>::Subpixel: Into<u32>,
{
    let mut ret = ImageBuffer::new(img.width(), img.height());

    use image::Primitive;
    let max: u32 = (Px::Subpixel::DEFAULT_MAX_VALUE).into();
    let scale: u32 = max * max / 255;

    for (from, to) in img.pixels().zip(ret.pixels_mut()) {
        let from_channels = from.channels();
        let bg_channels = background_color.channels();

        let a: u32 = from.channels()[3].into();
        let na = max - a;

        let blend = |fg: Px::Subpixel, bg: u8| -> u8 {
            let fg: u32 = fg.into();
            let bg: u32 = bg.into();

            ((fg * a) / scale + (bg * na) / max).try_into().unwrap()
        };
        *to = Rgb([
            blend(from_channels[0], bg_channels[0]),
            blend(from_channels[1], bg_channels[1]),
            blend(from_channels[2], bg_channels[2]),
        ]);
    }

    ret
}

/// Given original image size and target thumbnail size, finds subimage x, y, width, height in the
/// original image, so that the cropped image is centered, maximally sized and has identical aspect
/// ratio to target_size. The output crop is also always non-empty.
fn crop_coordinates(orig_size: (u32, u32), target_size: (u32, u32)) -> (u32, u32, u32, u32) {
    let ow = orig_size.0 as u64;
    let oh = orig_size.1 as u64;
    let tw = target_size.0 as u64;
    let th = target_size.1 as u64;

    if ow * th > tw * oh {
        // Original is wider than target
        let height = orig_size.1;
        let width = ((tw * oh + th / 2) / th) as u32;
        let x = (orig_size.0 - width) / 2;
        (x, 0, width, height)
    } else {
        // Original is narrower than target
        let width = orig_size.0;
        let height = ((th * ow + tw / 2) / tw) as u32;
        let y = (orig_size.1 - height) / 2;
        (0, y, width, height)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert2::assert;
    use proptest::prop_assume;
    use test_strategy::proptest;

    #[test]
    fn crop_coordinates_example() {
        assert!(crop_coordinates((200, 100), (50, 50)) == (50, 0, 100, 100));
    }

    #[proptest]
    fn crop_coordinates_all(orig_size: (u32, u32), target_size: (u32, u32)) {
        prop_assume!(orig_size.0 > 0);
        prop_assume!(orig_size.1 > 0);
        prop_assume!(target_size.0 > 0);
        prop_assume!(target_size.1 > 0);

        let (x, y, w, h) = crop_coordinates(orig_size, target_size);

        assert!(w > 0);
        assert!(h > 0);

        // We're staying in bounds:
        assert!(x + w <= orig_size.0);
        assert!(y + h <= orig_size.1);

        // The output is maximum sized.
        assert!(w == orig_size.0 || h == orig_size.1);

        // Output is centered in input, +- 1 pixel
        assert!(orig_size.0 - w + 1 >= 2 * x);
        assert!(orig_size.0 - w <= 2 * x + 1);
        assert!(orig_size.1 - h + 1 >= 2 * y);
        assert!(orig_size.1 - h <= 2 * y + 1);

        // Target aspect ratio is kept
        //assert!((h as u64) * (target_size.0 as u64) / (target_size.1 as u64) + 1 >= (w as u64)); // TODO: Rounding!
        //assert!((w as u64) * (target_size.1 as u64) / (target_size.0 as u64) + 1 >= (h as u64)); // TODO: Rounding!
    }
}
