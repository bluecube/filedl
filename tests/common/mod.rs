macro_rules! test_app {
    () => {
        test_app!("{}")
    };
    ($metadata:expr) => {{
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("owned_data")).unwrap();
        std::fs::write(dir.path().join("metadata.json"), $metadata).unwrap();
        let app_data = AppData::with_config(
            Config {
                bind_address: "localhost".into(),
                bind_port: 8080,
                data_path: dir.path().to_owned(),
                linked_objects_root: dir.path().to_owned(),
                download_url: "/download".into(),
                admin_url: "/admin".into(),
                app_name: "Test".into(),
                display_timezone: chrono_tz::UTC,
                thumbnail_cache_size: 1024 * 1024,
            },
            true,
        )
        .unwrap();
        let app = actix_web::test::init_service(build_app!(app_data)).await;
        (dir, app)
    }};
}

pub(crate) use test_app;

/// Creates a valid empty PNG image with given resolution
#[allow(dead_code)] // not every test binary uses this, but it's shared via common/mod.rs
pub(crate) fn make_test_png(width: u32, height: u32) -> Vec<u8> {
    let img = image::DynamicImage::new_rgb8(width, height);
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    buf
}
