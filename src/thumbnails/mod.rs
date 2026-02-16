mod cache;
mod image_thumbnail;

use std::{fmt::Display, hash::Hash, io::Cursor, path::Path};

use actix_web::web::Bytes;
use image::{ImageBuffer, ImageFormat, Pixel, Rgb, RgbImage, RgbaImage};
use image_thumbnail::create_image_thumbnail;
use mime::Mime;
use serde::{Deserialize, Serialize};

use crate::error::Result;

pub use cache::{CacheStats, CachedThumbnails};

/// Background color used for JPEG fallback format
const BACKGROUND_COLOR: Rgb<u8> = Rgb([0xDA, 0xE1, 0xE4]);

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

    pub fn has_alpha(&self) -> bool {
        match self {
            ThumbnailType::Jpeg => false,
            ThumbnailType::Avif => true,
        }
    }

    fn image_output_format(&self) -> image::ImageFormat {
        match self {
            ThumbnailType::Jpeg => image::ImageFormat::Jpeg,
            ThumbnailType::Avif => image::ImageFormat::Avif,
        }
    }
}

/// Creates the thumbnail for a given path, resolution and type.
pub fn create_thumbnail(
    file: &Path,
    resolution: (u32, u32),
    thumbnail_type: ThumbnailType,
) -> Result<Bytes> {
    let thumb = create_image_thumbnail(file, resolution)?;
    let mut bytes: Vec<u8> = Vec::new();

    if thumbnail_type.has_alpha() {
        thumb.write_to(
            &mut Cursor::new(&mut bytes),
            thumbnail_type.image_output_format(),
        )?;
    } else {
        let thumb = blend_background(thumb, BACKGROUND_COLOR);
        thumb.write_to(
            &mut Cursor::new(&mut bytes),
            thumbnail_type.image_output_format(),
        )?;
    }
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

fn blend_background(img: RgbaImage, background_color: Rgb<u8>) -> RgbImage {
    let mut ret = ImageBuffer::new(img.width(), img.height());

    for (from, to) in img.pixels().zip(ret.pixels_mut()) {
        let from_channels = from.channels();
        let bg_channels = background_color.channels();

        let a: u32 = from.channels()[3].into();
        let na = 255 - a;

        let blend = |fg: u8, bg: u8| -> u8 {
            let fg: u32 = fg.into();
            let bg: u32 = bg.into();

            ((fg * a) / 255 + (bg * na) / 255).try_into().unwrap()
        };
        *to = Rgb([
            blend(from_channels[0], bg_channels[0]),
            blend(from_channels[1], bg_channels[1]),
            blend(from_channels[2], bg_channels[2]),
        ]);
    }

    ret
}
