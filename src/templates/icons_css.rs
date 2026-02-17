use crate::templates::AssetUrl;

pub fn icons_css(download_base_url: &str, cache_hash: &str) -> String {
    let asset_url = |file_name| AssetUrl {
        download_base_url,
        file_name,
        cache_hash,
    };

    format!(
        r#"

.directory a.main-link.no-thumbnail::before, .directory a.main-link img.thumbnail-loading {{ background-image: url("{}"); }}
.file a.main-link.no-thumbnail::before, .file a.main-link img.thumbnail-loading {{ background-image: url("{}"); }}
.image a.main-link.no-thumbnail::before, .image a.main-link img.thumbnail-loading {{ background-image: url("{}"); }}
"#,
        asset_url("directory.svg"),
        asset_url("file.svg"),
        asset_url("image.svg"),
    )
}
