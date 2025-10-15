use std::{num::NonZeroU32, path::Path};

use image::{imageops, DynamicImage, GenericImageView as _, ImageBuffer, Pixel, Rgb, RgbImage};

use crate::error::Result;

pub fn create_image_thumbnail(file: &Path, resolution: (u32, u32)) -> Result<RgbImage> {
    let img = open_image(file)?;
    let orientation = get_orientation(file)?;

    // TODO: Fix orientation for non-square non-centered crops
    let crop_coords = crop_coordinates(img.dimensions(), resolution);

    // TODO: Don't hardcode background color
    let rgb_img = normalize_layers(img, [0xDA, 0xE1, 0xE4].into());
    let resized = crop_and_resize(rgb_img, crop_coords, resolution);
    let resized_and_reoriented = fix_orientation(resized, orientation);

    Ok(resized_and_reoriented)
}

fn open_image(path: &Path) -> Result<DynamicImage> {
    let mut reader = image::ImageReader::open(path)?;
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
