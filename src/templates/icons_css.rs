use crate::templates::AssetUrl;

pub fn icons_css(download_base_url: &str, cache_hash: &str) -> String {
    let asset_url = |file_name| AssetUrl {
        download_base_url,
        file_name,
        cache_hash,
    };

    format!(
        r#"
a.main-link img {{ background-image: url("{}"); }}
.image a.main-link::before {{ background-image: url("{}"); }}
.directory a.main-link::before {{ background-image: url("{}"); }}
.file a.main-link::before {{ background-image: url("{}"); }};
"#,
        asset_url("image.svg"),
        asset_url("image.svg"),
        asset_url("directory.svg"),
        asset_url("file.svg")
    )
}
