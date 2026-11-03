use crate::templates::AssetUrl;

pub fn icons_css(base_url: &str, cache_hash: &str) -> String {
    let asset_url = |file_name| AssetUrl {
        base_url,
        file_name,
        cache_hash,
    };

    format!(
        r#"
li.directory {{ --icon: url("{}"); }}
li.file {{ --icon: url("{}"); }}
li.image {{ --icon: url("{}"); }}
li.error {{ --icon: url("{}"); }}
"#,
        asset_url("directory.svg"),
        asset_url("file.svg"),
        asset_url("image.svg"),
        asset_url("error.svg"),
    )
}
