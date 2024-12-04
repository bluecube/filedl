use crate::error::Result;
use actix_web::web::Bytes;
use image::{
    imageops, DynamicImage, GenericImageView, ImageBuffer, ImageFormat, Pixel, Rgb, RgbImage,
};
use mime::Mime;
use quick_cache::{sync::Cache, Weighter};
use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    fs::Metadata,
    hash::{Hash, Hasher},
    io::Cursor,
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::SystemTime,
};

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

    fn image_output_format(&self) -> image::ImageFormat {
        match self {
            ThumbnailType::Jpeg => image::ImageFormat::Jpeg,
            ThumbnailType::Avif => image::ImageFormat::Avif,
        }
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

/// Wraps create_thumbnail, making it async, without blocking the Tokio runtime.
async fn spawn_create_thumbnail(key: CacheKey) -> Result<Bytes> {
    let path = key.path;
    let resolution = key.resolution;
    let thumbnail_type = key.thumbnail_type;

    tokio_rayon::spawn(move || create_thumbnail(&path, resolution, thumbnail_type)).await
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
