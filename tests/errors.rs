mod common;
use common::test_app;

use actix_web::test;
use chrono::Utc;

#[actix_web::test]
async fn nonexistent_asset_returns_404() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/download/nosuch?mode=assets")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn expired_object_returns_404() {
    let expires = (Utc::now() - chrono::Duration::seconds(3600)).to_rfc3339();
    let metadata = format!(
        r#"{{"expobj":{{"ownership":{{"Linked":"linked_file.txt"}},"expires":"{}"}}}}"#,
        expires
    );
    let (dir, app) = test_app!(&metadata, no_background);
    std::fs::write(dir.path().join("linked_file.txt"), "content").unwrap();

    let req = test::TestRequest::get()
        .uri("/download/expobj")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn directory_traversal_in_download_returns_400() {
    let (dir, app) = test_app!();

    // Create a linked directory object
    let sub = dir.path().join("mydir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("file.txt"), "hello").unwrap();

    let req = test::TestRequest::put()
        .uri("/admin/objects/mydir?link=mydir")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/download/mydir/../../etc/passwd")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);
}

#[actix_web::test]
async fn thumbnail_mode_on_directory_returns_404() {
    let (dir, app) = test_app!();

    let sub = dir.path().join("thumbdir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("file.txt"), "hello").unwrap();

    let req = test::TestRequest::put()
        .uri("/admin/objects/thumbdir?link=thumbdir")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/download/thumbdir?mode=thumbnail&size=64")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn download_mode_on_root_returns_404() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/download?mode=download")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn linked_object_with_missing_file_returns_404() {
    let (dir, app) = test_app!();

    let file_path = dir.path().join("vanishing.txt");
    std::fs::write(&file_path, "temporary content").unwrap();

    let req = test::TestRequest::put()
        .uri("/admin/objects/vanish?link=vanishing.txt")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    // Delete the underlying file
    std::fs::remove_file(&file_path).unwrap();

    let req = test::TestRequest::get()
        .uri("/download/vanish")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn error_response_is_html_with_status_and_hash() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/download/nonexistent")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);

    let body = String::from_utf8(test::read_body(resp).await.to_vec()).unwrap();
    assert!(body.contains("404"), "body should contain status code");
    assert!(
        body.contains("Not Found"),
        "body should contain reason phrase"
    );
    assert!(
        body.contains("Error reference:"),
        "body should contain error reference label"
    );
    // Error hash is 8 hex chars
    assert!(
        body.contains("<code>"),
        "body should contain the error hash in a code element"
    );
}

#[actix_web::test]
async fn admin_error_page_has_admin_class() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/admin/nonexistent")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);

    let body = String::from_utf8(test::read_body(resp).await.to_vec()).unwrap();
    assert!(
        body.contains(r#"class="admin""#),
        "admin error page should have the admin class on the content section"
    );
}

#[actix_web::test]
async fn download_error_page_lacks_admin_class() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::get()
        .uri("/download/nonexistent")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);

    let body = String::from_utf8(test::read_body(resp).await.to_vec()).unwrap();
    assert!(
        !body.contains(r#"class="admin""#),
        "download error page should not have the admin class"
    );
}

#[actix_web::test]
async fn thumbnail_of_non_image_returns_500() {
    let (_dir, app) = test_app!();

    let req = test::TestRequest::put()
        .uri("/admin/objects/readme.txt")
        .set_payload("This is just plain text, not an image.")
        .to_request();
    test::call_service(&app, req).await;

    let req = test::TestRequest::get()
        .uri("/download/readme.txt?mode=thumbnail&size=64")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 500);
}
