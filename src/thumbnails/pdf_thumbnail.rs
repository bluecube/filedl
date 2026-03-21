use std::{fs::read, path::Path, sync::Arc};

use assert2::assert;
use fast_image_resize::{ResizeOptions, Resizer};
use hayro::{RenderSettings, hayro_interpret::InterpreterSettings, hayro_syntax::Pdf};
use image::{Rgba, RgbaImage};

use super::{IOSnafu, PdfLoadSnafu, ThumbnailResult as Result};
use snafu::ResultExt as _;

pub fn create_pdf_thumbnail(file: &Path, resolution: (u32, u32)) -> Result<RgbaImage> {
    // Calculate render resolution, targeting 512x512 render.
    let ratio = (512 / resolution.0).max(512 / resolution.1).max(1);
    let render_resolution = (resolution.0 * ratio, resolution.1 * ratio);

    let rendered = create_pdf_thumbnail_inner(file, render_resolution)?;

    let mut dst_image = RgbaImage::new(resolution.0, resolution.1);
    let mut resizer = Resizer::new();
    resizer
        .resize(&rendered, &mut dst_image, &ResizeOptions::new())
        .unwrap();

    Ok(dst_image)
}

fn create_pdf_thumbnail_inner(file: &Path, resolution: (u32, u32)) -> Result<RgbaImage> {
    let file_data = Arc::new(read(file).context(IOSnafu)?);
    let pdf = Pdf::new(file_data).map_err(|source| PdfLoadSnafu { source }.build())?;

    let first_page = pdf.pages().first().unwrap(); // TODO: What to do if there are no pages?

    let (page_w, page_h) = first_page.render_dimensions();

    // Scale has extra 0.5 to make sure the rounding doesn't make the image 1 px smaller than intended
    let scale_x = (resolution.0 as f32 + 0.5) / page_w;
    let scale_y = (resolution.1 as f32 + 0.5) / page_h;
    let scale = scale_x.min(scale_y);

    let render_settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        ..Default::default()
    };

    let pixmap = hayro::render(
        first_page,
        &InterpreterSettings::default(),
        &render_settings,
    );

    let rendered_w = pixmap.width() as u32;
    let rendered_h = pixmap.height() as u32;

    assert!(rendered_w <= resolution.0);
    assert!(rendered_h <= resolution.1);

    let converted_pixmap_pixel = |x: u32, y: u32| -> Rgba<u8> {
        assert!(x < pixmap.width().into(),);
        assert!(y < pixmap.height().into(),);

        let px = pixmap.sample(x as u16, y as u16);

        // Flatten alpha channel onto white background
        let convert = |v| v + (255 - px.a) as u8;
        Rgba([convert(px.r), convert(px.g), convert(px.b), 255])

        // Un-premultiply alpha, keep transparency
        // if px.a == 0 {
        //     Rgba([px.r, px.g, px.b, px.a])
        // } else {
        //     let convert = |v| ((v as u16) * 255u16 / (px.a as u16)) as u8;
        //     Rgba([convert(px.r), convert(px.g), convert(px.b), px.a])
        // }
    };

    let min_x = (resolution.0 - rendered_w) / 2;
    let max_x = min_x + rendered_w;
    let min_y = (resolution.1 - rendered_h) / 2;
    let max_y = min_y + rendered_h;
    let image = RgbaImage::from_fn(resolution.0, resolution.1, |x, y| {
        if x < min_x || x >= max_x || y < min_y || y >= max_y {
            Rgba([0, 0, 0, 0])
        } else {
            converted_pixmap_pixel(x - min_x, y - min_y)
        }
    });

    Ok(image)
}
