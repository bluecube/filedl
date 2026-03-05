use actix_web::{http::header, test};
use filedl::{app_data::AppData, build_app, config::Config};
use std::sync::Arc;

macro_rules! test_app {
    () => {
        test_app!("{}")
    };
    ($metadata:expr) => {{
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("owned_data")).unwrap();
        std::fs::write(dir.path().join("metadata.json"), $metadata).unwrap();
        let app_data = Arc::new(
            AppData::with_config(Config {
                bind_address: "localhost".into(),
                bind_port: 8080,
                data_path: dir.path().to_owned(),
                linked_objects_root: dir.path().to_owned(),
                download_url: "/download".into(),
                admin_url: "/admin".into(),
                app_name: "Test".into(),
                display_timezone: chrono_tz::UTC,
                thumbnail_cache_size: 1024 * 1024,
            })
            .unwrap(),
        );
        let app = test::init_service(build_app!(app_data)).await;
        (dir, app)
    }};
}

#[actix_web::test]
async fn root_returns_200() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get().uri("/").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn download_root_returns_200() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get().uri("/download").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn unknown_object_is_404() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/download/nonexistent")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn upload_and_retrieve_file() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/testfile")
        .set_payload("hello world")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/download/testfile")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body = test::read_body(resp).await;
    assert_eq!(body.as_ref(), b"hello world");
}

#[actix_web::test]
async fn duplicate_upload_is_rejected() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/myfile")
        .set_payload("first")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::put()
        .uri("/admin/objects/myfile")
        .set_payload("second")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(!resp.status().is_success());
}

#[actix_web::test]
async fn thumbnail_cache_stats_returns_200() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/admin/thumbnail_cache_stats")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn upload_response_contains_download_url() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/myupload")
        .set_payload("content")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["download_url"], "/download/myupload");
}

#[actix_web::test]
async fn force_download_sets_attachment_disposition() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/testdl")
        .set_payload("some content")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::get()
        .uri("/download/testdl?mode=download")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let cd = resp.headers().get(header::CONTENT_DISPOSITION).unwrap();
    assert!(cd.to_str().unwrap().contains("attachment"));
}

#[actix_web::test]
async fn linked_object_is_accessible() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("owned_data")).unwrap();
    std::fs::write(dir.path().join("linked_file.txt"), "linked content").unwrap();
    std::fs::write(
        dir.path().join("metadata.json"),
        r#"{"mylink":{"ownership":{"Linked":"linked_file.txt"}}}"#,
    )
    .unwrap();
    let app_data = Arc::new(
        AppData::with_config(Config {
            bind_address: "localhost".into(),
            bind_port: 8080,
            data_path: dir.path().to_owned(),
            linked_objects_root: dir.path().to_owned(),
            download_url: "/download".into(),
            admin_url: "/admin".into(),
            app_name: "Test".into(),
            display_timezone: chrono_tz::UTC,
            thumbnail_cache_size: 1024 * 1024,
        })
        .unwrap(),
    );
    let app = test::init_service(build_app!(app_data)).await;

    let req = test::TestRequest::get()
        .uri("/download/mylink")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body = test::read_body(resp).await;
    assert_eq!(body.as_ref(), b"linked content");
}

#[actix_web::test]
async fn unlisted_objects_not_shown_in_listing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("owned_data")).unwrap();
    std::fs::write(dir.path().join("owned_data/publicfile"), "public").unwrap();
    std::fs::write(dir.path().join("owned_data/hiddenfile"), "hidden").unwrap();
    std::fs::write(
        dir.path().join("metadata.json"),
        r#"{"publicfile":{"ownership":"Owned"},"hiddenfile":{"ownership":"Owned","unlisted_key":"secret"}}"#,
    )
    .unwrap();
    let app_data = Arc::new(
        AppData::with_config(Config {
            bind_address: "localhost".into(),
            bind_port: 8080,
            data_path: dir.path().to_owned(),
            linked_objects_root: dir.path().to_owned(),
            download_url: "/download".into(),
            admin_url: "/admin".into(),
            app_name: "Test".into(),
            display_timezone: chrono_tz::UTC,
            thumbnail_cache_size: 1024 * 1024,
        })
        .unwrap(),
    );
    let app = test::init_service(build_app!(app_data)).await;

    let req = test::TestRequest::get().uri("/download").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body = test::read_body(resp).await;
    let body_str = std::str::from_utf8(&body).unwrap();
    assert!(body_str.contains("publicfile"));
    assert!(!body_str.contains("hiddenfile"));
}

#[actix_web::test]
async fn root_listing_json_mode() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/visible")
        .set_payload("data")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::get()
        .uri("/download?mode=json")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let items: Vec<serde_json::Value> = test::read_body_json(resp).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "visible");
}

#[actix_web::test]
async fn directory_listing_json_mode() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("owned_data")).unwrap();
    let content_dir = dir.path().join("mydir");
    std::fs::create_dir(&content_dir).unwrap();
    std::fs::write(content_dir.join("alpha.txt"), "aaa").unwrap();
    std::fs::write(content_dir.join("beta.txt"), "bbb").unwrap();
    std::fs::write(
        dir.path().join("metadata.json"),
        r#"{"mydir":{"ownership":{"Linked":"mydir"}}}"#,
    )
    .unwrap();
    let app_data = Arc::new(
        AppData::with_config(Config {
            bind_address: "localhost".into(),
            bind_port: 8080,
            data_path: dir.path().to_owned(),
            linked_objects_root: dir.path().to_owned(),
            download_url: "/download".into(),
            admin_url: "/admin".into(),
            app_name: "Test".into(),
            display_timezone: chrono_tz::UTC,
            thumbnail_cache_size: 1024 * 1024,
        })
        .unwrap(),
    );
    let app = test::init_service(build_app!(app_data)).await;

    let req = test::TestRequest::get()
        .uri("/download/mydir?mode=json")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let mut items: Vec<serde_json::Value> = test::read_body_json(resp).await;
    items.sort_by_key(|v| v["name"].as_str().unwrap_or("").to_owned());
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "alpha.txt");
    assert_eq!(items[0]["item_type"], "file");
    assert_eq!(items[1]["name"], "beta.txt");
    assert_eq!(items[1]["item_type"], "file");
}

#[actix_web::test]
async fn json_mode_on_file_returns_404() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/afile")
        .set_payload("content")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::get()
        .uri("/download/afile?mode=json")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

fn make_test_png(width: u32, height: u32) -> Vec<u8> {
    let img = image::DynamicImage::new_rgb8(width, height);
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    buf
}

#[actix_web::test]
async fn thumbnail_returns_correct_size() {
    let (_dir, app) = test_app!();

    let png = make_test_png(200, 200);
    let req = test::TestRequest::put()
        .uri("/admin/objects/test.png")
        .set_payload(png)
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::get()
        .uri("/download/test.png?mode=thumbnail&size=64")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/jpeg"
    );

    let body = test::read_body(resp).await;
    let thumb = image::load_from_memory(&body).unwrap();
    assert_eq!(thumb.width(), 64);
    assert_eq!(thumb.height(), 64);
}

#[actix_web::test]
async fn thumbnail_size_rounds_up() {
    let (_dir, app) = test_app!();

    let png = make_test_png(200, 200);
    let req = test::TestRequest::put()
        .uri("/admin/objects/test.png")
        .set_payload(png)
        .to_request();
    test::call_service(&app, req).await;

    // size=100 is >64 and <=128, so should round up to 128
    let req = test::TestRequest::get()
        .uri("/download/test.png?mode=thumbnail&size=100")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body = test::read_body(resp).await;
    let thumb = image::load_from_memory(&body).unwrap();
    assert_eq!(thumb.width(), 128);
    assert_eq!(thumb.height(), 128);
}

#[actix_web::test]
async fn directory_zip_download() {
    // "fs_dirname" is the name on disk, "public_name" is the filedl object name.
    // The zip should use the filedl name, not the filesystem name.
    let (dir, app) = test_app!(r#"{"public_name":{"ownership":{"Linked":"fs_dirname"}}}"#);
    let content_dir = dir.path().join("fs_dirname");
    std::fs::create_dir(&content_dir).unwrap();
    std::fs::write(content_dir.join("file1.txt"), "content1").unwrap();
    std::fs::write(content_dir.join("file2.txt"), "content2").unwrap();

    let req = test::TestRequest::get()
        .uri("/download/public_name?mode=download")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let cd = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cd.contains("attachment"));
    assert!(cd.contains("public_name.zip"));

    let body = test::read_body(resp).await;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(body.as_ref())).unwrap();
    let names: std::collections::HashSet<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_owned())
        .collect();
    assert!(names.contains("public_name/file1.txt"));
    assert!(names.contains("public_name/file2.txt"));
    assert!(!names.iter().any(|n| n.starts_with("fs_dirname")));
}

#[actix_web::test]
async fn thumbnail_avif_with_accept_header() {
    let (_dir, app) = test_app!();

    let png = make_test_png(200, 200);
    let req = test::TestRequest::put()
        .uri("/admin/objects/test.png")
        .set_payload(png)
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::get()
        .uri("/download/test.png?mode=thumbnail&size=64")
        .insert_header((header::ACCEPT, "image/avif"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/avif"
    );

    // AVIF decoding is not supported by the image crate in this configuration;
    // verifying content-type and a non-empty body is sufficient.
    let body = test::read_body(resp).await;
    assert!(!body.is_empty());
}

#[actix_web::test]
async fn asset_can_be_downloaded() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/download/style.min.css?mode=assets")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("text/css")
    );
}

#[actix_web::test]
async fn asset_brotli_compressed_when_accepted() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/download/style.min.css?mode=assets")
        .insert_header((header::ACCEPT_ENCODING, "br"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get(header::CONTENT_ENCODING).unwrap(), "br");
}

#[actix_web::test]
async fn admin_dashboard_returns_200() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get().uri("/admin").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn admin_create_linked_object() {
    let (dir, app) = test_app!();
    std::fs::write(dir.path().join("linked.txt"), "linked content").unwrap();

    let req = test::TestRequest::put()
        .uri("/admin/objects/mylink?link=linked.txt")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["download_url"], "/download/mylink");

    let req = test::TestRequest::get()
        .uri("/download/mylink")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body = test::read_body(resp).await;
    assert_eq!(body.as_ref(), b"linked content");
}

#[actix_web::test]
async fn admin_delete_owned_object() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/todelete")
        .set_payload("content")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::delete()
        .uri("/admin/objects/todelete")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/download/todelete")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn admin_delete_nonexistent_returns_404() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::delete()
        .uri("/admin/objects/nosuchobject")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn admin_upload_unlisted_returns_key_in_url() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/secret?unlisted=true")
        .set_payload("secret content")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    let download_url = body["download_url"].as_str().unwrap();
    assert!(download_url.starts_with("/download/secret?key="));

    // object is accessible with the key
    let req = test::TestRequest::get().uri(download_url).to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    // object is not accessible without the key
    let req = test::TestRequest::get()
        .uri("/download/secret")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn admin_dashboard_lists_all_objects_including_unlisted() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/public")
        .set_payload("a")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::put()
        .uri("/admin/objects/hidden?unlisted=true")
        .set_payload("b")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::get().uri("/admin").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body = test::read_body(resp).await;
    let html = std::str::from_utf8(&body).unwrap();
    assert!(html.contains("public"));
    assert!(html.contains("hidden"));
}

#[actix_web::test]
async fn admin_link_with_slash_prefixed_path() {
    // A leading '/' should be treated as relative to linked_objects_root,
    // not as an absolute filesystem path.
    let (dir, app) = test_app!();
    std::fs::write(dir.path().join("linked.txt"), "content").unwrap();

    let req = test::TestRequest::put()
        .uri("/admin/objects/mylink?link=/linked.txt")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success(), "status: {}", resp.status());

    let req = test::TestRequest::get()
        .uri("/download/mylink")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(test::read_body(resp).await.as_ref(), b"content");
}

#[actix_web::test]
async fn unlisted_object_requires_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("owned_data")).unwrap();
    std::fs::write(dir.path().join("owned_data/secretfile"), "secret content").unwrap();
    std::fs::write(
        dir.path().join("metadata.json"),
        r#"{"secretfile":{"ownership":"Owned","unlisted_key":"mykey"}}"#,
    )
    .unwrap();
    let app_data = Arc::new(
        AppData::with_config(Config {
            bind_address: "localhost".into(),
            bind_port: 8080,
            data_path: dir.path().to_owned(),
            linked_objects_root: dir.path().to_owned(),
            download_url: "/download".into(),
            admin_url: "/admin".into(),
            app_name: "Test".into(),
            display_timezone: chrono_tz::UTC,
            thumbnail_cache_size: 1024 * 1024,
        })
        .unwrap(),
    );
    let app = test::init_service(build_app!(app_data)).await;

    let req = test::TestRequest::get()
        .uri("/download/secretfile")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);

    let req = test::TestRequest::get()
        .uri("/download/secretfile?key=mykey")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}
