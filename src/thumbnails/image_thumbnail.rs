use std::path::Path;

use fast_image_resize::{ResizeOptions, Resizer};
use image::{DynamicImage, ImageBuffer, Pixel, RgbaImage, imageops};

use crate::{error::Result, thumbnails::cropping::crop_coordinates};

pub fn create_image_thumbnail(file: &Path, resolution: (u32, u32)) -> Result<RgbaImage> {
    let img = open_image(file)?;
    let img = img.into_rgba8();

    let orientation = get_orientation(file)?;

    // TODO: Fix orientation for non-square non-centered crops
    let crop_coords = crop_coordinates(
        img.dimensions(),
        resolution,
        30, /* A good looking compromise to almost centering the crop */
    );

    let resized = crop_and_resize(img, crop_coords, resolution);
    let resized_and_reoriented = fix_orientation(resized, orientation);

    Ok(resized_and_reoriented)
}

fn open_image(path: &Path) -> Result<DynamicImage> {
    let mut reader = image::ImageReader::open(path)?;
    reader.no_limits();
    Ok(reader.decode()?)
}

fn crop_and_resize(
    img: RgbaImage,
    crop_coords: (u32, u32, u32, u32),
    new_size: (u32, u32),
) -> RgbaImage {
    let mut dst_image = RgbaImage::new(new_size.0, new_size.1);

    let mut resizer = Resizer::new();
    resizer
        .resize(
            &img,
            &mut dst_image,
            &ResizeOptions::new().crop(
                crop_coords.0 as f64,
                crop_coords.1 as f64,
                crop_coords.2 as f64,
                crop_coords.3 as f64,
            ),
        )
        .unwrap();

    dst_image
}

fn get_orientation(path: &Path) -> Result<u32> {
    let file = std::fs::File::open(path)?;
    let mut bufreader = std::io::BufReader::new(file);
    let exifreader = exif::Reader::new();
    let Ok(exif_tags) = exifreader.read_from_container(&mut bufreader) else {
        return Ok(1);
    };

    Ok(exif_tags
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|o| o.value.get_uint(0))
        .filter(|&v| (1..=8).contains(&v))
        .unwrap_or(1))
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
