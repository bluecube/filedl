use actix_web::{http::header, test};
use filedl::{app_data::AppData, build_app, config::Config};
use std::sync::Arc;

macro_rules! test_app {
    () => {{
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("owned_data")).unwrap();
        let app_data = Arc::new(
            AppData::with_config(Config {
                bind_address: "localhost".into(),
                bind_port: 8080,
                data_path: dir.path().to_owned(),
                linked_objects_root: dir.path().to_owned(),
                download_url: "/download".into(),
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
