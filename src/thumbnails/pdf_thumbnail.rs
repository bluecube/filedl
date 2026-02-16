use std::{fs::read, path::Path, sync::Arc};

use hayro::{RenderSettings, hayro_interpret::InterpreterSettings, hayro_syntax::Pdf};
use image::{GenericImageView as _, RgbaImage};

use crate::{
    error::{FiledlError, Result},
    thumbnails::cropping::crop_coordinates,
};

pub fn create_pdf_thumbnail(file: &Path, resolution: (u32, u32)) -> Result<RgbaImage> {
    let file_data = Arc::new(read(file)?);
    let pdf = Pdf::new(file_data).map_err(|e| FiledlError::PdfLoadError(e))?;

    let first_page = pdf.pages().first().unwrap(); // TODO: What to do if there are no pages?

    let (page_w, page_h) = first_page.render_dimensions();

    let scale_x = resolution.0 as f32 / page_w as f32;
    let scale_y = resolution.1 as f32 / page_h as f32;
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

    let subpixels = {
        let mut subpixels = Vec::with_capacity(rendered_w as usize * rendered_h as usize * 4);

        subpixels.extend(pixmap.take().into_iter().flat_map(|premul_rgba| {
            // TODO: Un-premultiply
            premul_rgba.to_u8_array().into_iter()
        }));

        subpixels
    };

    let image = RgbaImage::from_raw(rendered_w, rendered_h, subpixels).unwrap();

    let (crop_x, crop_y, crop_w, crop_h) = crop_coordinates(
        (rendered_w, rendered_h),
        resolution,
        0, /* Top of the page is most important */
    );

    let view = image.view(crop_x, crop_y, crop_w, crop_h);
    Ok(view.to_image())
}
