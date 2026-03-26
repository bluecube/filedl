mod common;
use common::test_app;

use actix_web::test;
use chrono::Utc;
use test_case::test_case;

#[actix_web::test]
async fn duplicate_linked_object_returns_409() {
    let (dir, app) = test_app!();
    std::fs::write(dir.path().join("file.txt"), "content").unwrap();

    let req = test::TestRequest::put()
        .uri("/admin/objects/duplink?link=file.txt")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::put()
        .uri("/admin/objects/duplink?link=file.txt")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 409);
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
        .uri("/admin/objects/secret?unlisted_key=mysecretkey")
        .set_payload("secret content")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    let download_url = body["download_url"].as_str().unwrap();
    assert_eq!(download_url, "/download/secret?key=mysecretkey");

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
        .uri("/admin/objects/hidden?unlisted_key=hiddenkey")
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
async fn patch_public_to_unlisted() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/myfile")
        .set_payload("content")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::patch()
        .uri("/admin/objects/myfile")
        .set_json(serde_json::json!({"unlisted_key": "mykey", "expires": null}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/download/myfile?key=mykey")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/download/myfile")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn patch_unlisted_to_public() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/myfile?unlisted_key=somekey")
        .set_payload("content")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::patch()
        .uri("/admin/objects/myfile")
        .set_json(serde_json::json!({"unlisted_key": null, "expires": null}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/download/myfile")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn patch_changes_unlisted_key() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/myfile?unlisted_key=somekey")
        .set_payload("content")
        .to_request();
    let resp = test::call_service(&app, req).await;
    let body: serde_json::Value = test::read_body_json(resp).await;
    let old_url = body["download_url"].as_str().unwrap().to_owned();

    let req = test::TestRequest::patch()
        .uri("/admin/objects/myfile")
        .set_json(serde_json::json!({"unlisted_key": "newkey", "expires": null}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri("/download/myfile?key=newkey")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get().uri(&old_url).to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn patch_nonexistent_returns_404() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::patch()
        .uri("/admin/objects/nosuchobject")
        .set_json(serde_json::json!({"unlisted_key": null, "expires": null}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn patch_sets_expiry() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/myfile")
        .set_payload("content")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::patch()
        .uri("/admin/objects/myfile")
        .set_json(serde_json::json!({"unlisted_key": null, "expires": "2099-01-01T00:00:00Z"}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn patch_clears_expiry() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/myfile")
        .set_payload("content")
        .to_request();
    test::call_service(&app, req).await;

    // First set an expiry
    let req = test::TestRequest::patch()
        .uri("/admin/objects/myfile")
        .set_json(serde_json::json!({"unlisted_key": null, "expires": "2099-01-01T00:00:00Z"}))
        .to_request();
    test::call_service(&app, req).await;

    // Then clear it
    let req = test::TestRequest::patch()
        .uri("/admin/objects/myfile")
        .set_json(serde_json::json!({"unlisted_key": null, "expires": null}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn admin_create_with_expiry() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/expiring?expires=2099-01-01T00%3A00%3A00Z")
        .set_payload("content")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["download_url"], "/download/expiring");
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
async fn expired_object_is_deleted() {
    let (_dir, app) = test_app!();

    // Create object with expiry already in the past — the background task will pick it up
    // immediately when signalled.
    let expires = (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    let uri = format!(
        "/admin/objects/ephemeral?expires={}",
        percent_encoding::utf8_percent_encode(&expires, percent_encoding::NON_ALPHANUMERIC)
    );
    let req = test::TestRequest::put()
        .uri(&uri)
        .set_payload("temp content")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    // Give the background expiry task a moment to process the signal
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Object should be gone
    let req = test::TestRequest::get()
        .uri("/download/ephemeral")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);

    // Admin listing should not contain it
    let req = test::TestRequest::get().uri("/admin").to_request();
    let resp = test::call_service(&app, req).await;
    let body = test::read_body(resp).await;
    let html = std::str::from_utf8(&body).unwrap();
    assert!(!html.contains("ephemeral"));
}

#[actix_web::test]
async fn browse_linked_returns_entries() {
    let (dir, app) = test_app!();
    std::fs::write(dir.path().join("hello.txt"), "hi").unwrap();
    std::fs::create_dir(dir.path().join("mydir")).unwrap();

    let req = test::TestRequest::get()
        .uri("/admin/browse_linked?path=")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Vec<serde_json::Value> = test::read_body_json(resp).await;
    let names: Vec<&str> = body.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"hello.txt"));
    assert!(names.contains(&"mydir"));
}

#[actix_web::test]
async fn browse_linked_rejects_traversal() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/admin/browse_linked?path=..")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let req = test::TestRequest::get()
        .uri("/admin/browse_linked?path=foo/../..")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);
}

#[actix_web::test]
async fn browse_linked_subdirectory() {
    let (dir, app) = test_app!();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub").join("inner.txt"), "").unwrap();

    let req = test::TestRequest::get()
        .uri("/admin/browse_linked?path=sub")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: Vec<serde_json::Value> = test::read_body_json(resp).await;
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["name"], "inner.txt");
}

#[actix_web::test]
async fn browse_linked_nonexistent_returns_error() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/admin/browse_linked?path=no_such_dir")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(!resp.status().is_success());
}

#[test_case("/admin/objects/evil?link=../etc/passwd"; "leading dotdot in link path")]
#[test_case("/admin/objects/evil?link=foo/../../etc"; "embedded dotdot in link path")]
#[test_case("/admin/objects/../escaped?link=file.txt"; "dotdot in linked object id")]
#[test_case("/admin/objects/foo/bar?link=file.txt"; "slash in linked object id")]
#[actix_web::test]
async fn linked_object_rejects_invalid_path(uri: &str) {
    let (dir, app) = test_app!();
    std::fs::write(dir.path().join("file.txt"), "content").unwrap();

    let req = test::TestRequest::put().uri(uri).to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);
}

#[test_case("/admin/objects/../escaped"; "dotdot in upload object id")]
#[test_case("/admin/objects/foo/bar"; "slash in upload object id")]
#[actix_web::test]
async fn upload_rejects_invalid_object_id(uri: &str) {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri(uri)
        .set_payload("malicious")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);
}
