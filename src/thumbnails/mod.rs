mod cache;
mod image_thumbnail;

use crate::error::Result;
use actix_web::web::Bytes;
use image::ImageFormat;
use image_thumbnail::create_image_thumbnail;
use mime::Mime;
use serde::{Deserialize, Serialize};
use std::{fmt::Display, hash::Hash, io::Cursor, path::Path};

pub use cache::{CacheStats, CachedThumbnails};

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

/// Creates the thumbnail for a given path, resolution and type.
pub fn create_thumbnail(
    file: &Path,
    resolution: (u32, u32),
    thumbnail_type: ThumbnailType,
) -> Result<Bytes> {
    let thumb = create_image_thumbnail(file, resolution)?;
    let mut bytes: Vec<u8> = Vec::new();
    thumb.write_to(
        &mut Cursor::new(&mut bytes),
        thumbnail_type.image_output_format(),
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
