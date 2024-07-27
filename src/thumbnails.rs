use crate::error::Result;
use actix_web::web::Bytes;
use image::{
    imageops, DynamicImage, GenericImageView, ImageBuffer, ImageFormat, Pixel, Rgb, RgbImage,
};
use lru::LruCache;
use serde::Serialize;
use std::{
    fs::Metadata,
    hash::{Hash, Hasher},
    io::Cursor,
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::SystemTime,
};
use tokio::{sync::Mutex, task::spawn_blocking};

/// Describes a cached rendered thumbnail
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
    fn new(path: PathBuf, metadata: &Metadata, size: (u32, u32)) -> Self {
        CacheKey {
            path,
            size: metadata.len(),
            modtime: metadata.modified().ok(),

            width: size.0,
            height: size.1,
        }
    }

    fn hash_string(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{:X}", hasher.finish())
    }
}

#[derive(Copy, Clone, Debug, Default)]
struct HitRate {
    pub rate: f32,
}

impl HitRate {
    const SMOOTHING: f32 = 0.995;

    fn count(&mut self, success: bool) {
        self.rate *= Self::SMOOTHING;
        if success {
            self.rate += 1f32 - Self::SMOOTHING;
        }
    }
}

#[derive(Debug)]
struct Locked {
    cache: LruCache<CacheKey, Bytes>,
    used_size: usize,

    hit_rate: HitRate,
    wasted_creation_rate: HitRate,
}

#[derive(Debug)]
pub struct CachedThumbnails {
    locked: Mutex<Locked>,
    max_size: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct CacheStats {
    pub count: usize,
    pub used_size: usize,
    pub max_size: usize,
    pub hit_rate: f32,
    pub wasted_creation_rate: f32,
}

impl CachedThumbnails {
    pub fn new(max_size: usize) -> Self {
        CachedThumbnails {
            locked: Mutex::new(Locked {
                cache: LruCache::unbounded(),
                used_size: 0,
                hit_rate: HitRate { rate: 0.5 },
                wasted_creation_rate: HitRate { rate: 0.5 },
            }),
            max_size,
        }
    }

    pub async fn get(
        &self,
        file: PathBuf,
        metadata: &Metadata,
        size: (u32, u32),
    ) -> Result<(Bytes, String)> {
        let mut key = CacheKey::new(file, metadata, size); // Must be mutable because of the
                                                           // spawn_blocking trick below
        let hash = key.hash_string();
        {
            let mut locked = self.locked.lock().await;

            if let Some(thumbnail) = locked.cache.get(&key) {
                let ret = Ok((Bytes::clone(thumbnail), hash));
                locked.hit_rate.count(true);
                return ret;
            } else {
                locked.hit_rate.count(false);
            }
        }

        // Here we pass the path through the closure, so that the compiler understands
        // that it will live long enough.
        let join_result = spawn_blocking(move || {
            let path = key.path;
            let thumbnail = create_thumbnail(&path, size);
            (thumbnail, path)
        })
        .await;

        let (thumbnail, path) = match join_result {
            Ok(x) => x,
            Err(e) => {
                if let Ok(reason) = e.try_into_panic() {
                    std::panic::resume_unwind(reason)
                } else {
                    unreachable!("We never cancel the join handle.")
                }
            }
        };

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
            locked.wasted_creation_rate.count(true);
        } else {
            locked.wasted_creation_rate.count(false);
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
            hit_rate: locked.hit_rate.rate,
            wasted_creation_rate: locked.wasted_creation_rate.rate,
        }
    }
}

pub fn create_thumbnail(file: &Path, size: (u32, u32)) -> Result<Bytes> {
    let img = open_image(file)?;
    let orientation = get_orientation(file)?;

    // TODO: Fix orientation for non-square non-centered crops
    let crop_coords = crop_coordinates(img.dimensions(), size);

    // TODO: Don't hardcode background color
    let rgb_img = normalize_layers(img, [0xDA, 0xE1, 0xE4].into());
    let resized = crop_and_resize(rgb_img, crop_coords, size);
    let resized_and_reoriented = fix_orientation(resized, orientation);

    let mut bytes: Vec<u8> = Vec::new();
    resized_and_reoriented.write_to(
        &mut Cursor::new(&mut bytes),
        image::ImageOutputFormat::Jpeg(85),
    )?;
    Ok(bytes.into())
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
