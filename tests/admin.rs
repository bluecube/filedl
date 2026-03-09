mod common;
use common::test_app;

use actix_web::test;
use filedl::{app_data::AppData, build_app, config::Config};
use std::sync::Arc;

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
