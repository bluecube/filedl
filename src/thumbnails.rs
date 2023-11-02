use actix_web::web::Bytes;
use anyhow;
use image::{imageops, GenericImageView, ImageBuffer, ImageFormat, Pixel, Rgb, Rgba};
use lru::LruCache;
use serde::Serialize;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::{fs, sync::Mutex, task::spawn_blocking};

#[derive(Hash, Debug, PartialEq, Eq)]
struct CacheKey {
    // First three arguments deal with the source file:
    path: PathBuf,
    size: u64,
    modtime: Option<SystemTime>,

    // Properties of the final thumbnail
    width: u32,
    height: u32,
}

impl CacheKey {
    fn hash_string(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{:X}", hasher.finish())
    }
}

struct Locked {
    cache: LruCache<CacheKey, Bytes>,
    used_size: usize,
    hit_rate: f32,
}

pub struct CachedThumbnails {
    locked: Mutex<Locked>,
    max_size: usize,
}

const HIT_RATE_SMOOTHING: f32 = 0.995;

#[derive(Clone, Debug, Serialize)]
pub struct CacheStats {
    pub count: usize,
    pub used_size: usize,
    pub max_size: usize,
    pub hit_rate: f32,
}

impl CachedThumbnails {
    pub fn new(max_size: usize) -> Self {
        CachedThumbnails {
            locked: Mutex::new(Locked {
                cache: LruCache::unbounded(),
                used_size: 0,
                hit_rate: 0.5,
            }),
            max_size,
        }
    }

    pub async fn get(&self, file: PathBuf, size: (u32, u32)) -> anyhow::Result<(Bytes, String)> {
        let mut key = CacheKey::new(file, size).await?; // Must be mutable because of the
                                                        // spawn_blocking trick below
        let hash = key.hash_string();
        {
            let mut locked = self.locked.lock().await;

            locked.hit_rate *= HIT_RATE_SMOOTHING;
            if let Some(thumbnail) = locked.cache.get(&key) {
                let ret = Ok((Bytes::clone(thumbnail), hash));
                locked.hit_rate += 1.0 - HIT_RATE_SMOOTHING;
                return ret;
            }
        }

        // Here we pass the path through the closure, so that the compiler understands
        // that it will live long enough.
        let (thumbnail, path) = spawn_blocking(move || {
            let path = key.path;
            let thumbnail = create_thumbnail(&path, size);
            (thumbnail, path)
        })
        .await?;
        key.path = path;
        let thumbnail = thumbnail?;

        if thumbnail.len() > self.max_size {
            // If the file is larger than the cache, we couldn't keep the size condition anyway,
            // so just return it without caching at all.
            return Ok((thumbnail, hash));
        }

        let mut locked = self.locked.lock().await;
        while locked.used_size + thumbnail.len() > self.max_size {
            let (_, evicted_thumbnail) = locked.cache.pop_lru().expect("cache should be non-empty");
            locked.used_size -= evicted_thumbnail.len();
        }
        if let Some(overwritten_thumbnail) = locked.cache.put(key, Bytes::clone(&thumbnail)) {
            // This should only happen fairly rarely -- one thread is working on the thumbnail,
            // while another thread requests it again, doesn't find it in cache and
            // starts working on it again.
            // In this case we just remove the version that is created first and replace it
            // with the newer one.
            // In this case we need to subtract the size that gets overwritten.
            locked.used_size -= overwritten_thumbnail.len();
        }
        locked.used_size += thumbnail.len();

        Ok((thumbnail, hash))
    }

    pub async fn cache_stats(&self) -> CacheStats {
        let locked = self.locked.lock().await;
        CacheStats {
            count: locked.cache.len(),
            used_size: locked.used_size,
            max_size: self.max_size,
            hit_rate: locked.hit_rate,
        }
    }
}

impl CacheKey {
    async fn new(path: PathBuf, size: (u32, u32)) -> anyhow::Result<CacheKey> {
        let metadata = fs::metadata(&path).await?;
        Ok(CacheKey {
            path,
            size: metadata.len(),
            modtime: metadata.modified().ok(),

            width: size.0,
            height: size.1,
        })
    }
}

pub fn create_thumbnail(file: &Path, size: (u32, u32)) -> anyhow::Result<Bytes> {
    let orientation = get_orientation(file)?;
    let img = image::open(file)?;

    // TODO: Fix orientation for non-square non-centered crops
    let (crop_x, crop_y, crop_width, crop_height) = crop_coordinates(img.dimensions(), size);
    let subimage = img.crop_imm(crop_x, crop_y, crop_width, crop_height);

    let resized = imageops::resize(&subimage, size.0, size.1, imageops::FilterType::Lanczos3);
    let resized_and_reoriented = fix_orientation(resized, orientation);
    let final_thumb = blend_background(resized_and_reoriented, [0xDA, 0xE1, 0xE4].into());
    // TODO: Don't hardcode background color

    // TODO: Go faster!

    let mut bytes: Vec<u8> = Vec::new();
    final_thumb.write_to(
        &mut Cursor::new(&mut bytes),
        image::ImageOutputFormat::Jpeg(85),
    )?;
    Ok(bytes.into())
}

pub fn is_thumbnailable(filename: &str) -> bool {
    let Some((_, extension)) = filename.rsplit_once('.') else {
        return false;
    };

    let Some(format) = ImageFormat::from_extension(extension) else {
        return false;
    };

    format.can_read()
}

fn get_orientation(path: &Path) -> anyhow::Result<u32> {
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

fn blend_background(
    mut img: ImageBuffer<Rgba<u8>, Vec<u8>>,
    background_color: Rgb<u8>,
) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let bg = background_color.to_rgba();
    img.pixels_mut().for_each(|px| {
        let a: u16 = px.channels()[3].into();
        let na = 255 - a;
        px.apply2(&bg, |fg, bg| -> u8 {
            ((a * u16::from(fg) + na * u16::from(bg)) / 255)
                .try_into()
                .unwrap()
        });
        px.channels_mut()[3] = 255;
    });

    img
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
