mod cache;
mod cropping;
mod image_thumbnail;
mod pdf_thumbnail;

use std::{
    fmt::Display,
    hash::Hash,
    io::Cursor,
    path::Path,
    time::{Duration, Instant},
};

use actix_web::{HttpRequest, http::header, web::Bytes};
use image::{ImageBuffer, ImageFormat, Pixel, Rgb, RgbImage, RgbaImage};
use image_thumbnail::create_image_thumbnail;
use memchr::memmem;
use mime::Mime;
use serde::{Deserialize, Serialize};

use pdf_thumbnail::create_pdf_thumbnail;
use snafu::prelude::*;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum ThumbnailError {
    #[snafu(display("Image processing failed at {location}"))]
    ImageError {
        #[snafu(source(from(image::error::ImageError, Box::new)))]
        source: Box<image::error::ImageError>,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("IO error at {location}"))]
    IOError {
        source: std::io::Error,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("PDF loading failed at {location}"))]
    PdfLoadError {
        #[snafu(source(false))]
        source: hayro::hayro_syntax::LoadPdfError,
        #[snafu(implicit)]
        location: snafu::Location,
    },
}

pub type ThumbnailResult<T> = std::result::Result<T, ThumbnailError>;

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
    pub fn from_request(req: &HttpRequest) -> ThumbnailType {
        if req
            .headers()
            .get(header::ACCEPT)
            .is_some_and(|value| memmem::find(value.as_bytes(), b"image/avif").is_some())
        {
            ThumbnailType::Avif
        } else {
            ThumbnailType::Jpeg
        }
    }

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
) -> ThumbnailResult<(Bytes, Duration)> {
    let start_time = Instant::now();

    let thumb = if is_path_pdf(file) {
        create_pdf_thumbnail(file, resolution)
    } else {
        create_image_thumbnail(file, resolution)
    }?;
    let mut bytes: Vec<u8> = Vec::new();

    if thumbnail_type.has_alpha() {
        thumb
            .write_to(
                &mut Cursor::new(&mut bytes),
                thumbnail_type.image_output_format(),
            )
            .context(ImageSnafu)?;
    } else {
        let thumb = blend_background(thumb, BACKGROUND_COLOR);
        thumb
            .write_to(
                &mut Cursor::new(&mut bytes),
                thumbnail_type.image_output_format(),
            )
            .context(ImageSnafu)?;
    }

    let elapsed = Instant::now().duration_since(start_time);
    log::debug!(
        "Creating thumbnail for {} ({:?}, {}) took {:?}",
        file.display(),
        resolution,
        thumbnail_type,
        elapsed
    );

    Ok((bytes.into(), elapsed))
}

/// Returns a hash describing the source image, if it is thumbnailable,
/// otherwise returns None.
pub fn is_thumbnailable(path: &Path) -> bool {
    if is_path_pdf(path) {
        return true;
    }
    let Some(extension) = path.extension() else {
        return false;
    };
    let Some(format) = ImageFormat::from_extension(extension) else {
        return false;
    };

    format.can_read()
}

fn is_path_pdf(path: &Path) -> bool {
    let Some(extension) = path.extension() else {
        return false;
    };
    extension.eq_ignore_ascii_case("pdf")
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
