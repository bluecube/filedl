use std::{
    env,
    fs::File,
    io::{Read, Write},
    path::Path,
};

use anyhow::anyhow;
use walkdir::WalkDir;

fn main() {
    process_assets(
        &Path::new("assets"),
        &Path::new(&env::var("OUT_DIR").unwrap()).join("assets"),
    )
    .unwrap();
}

fn process_assets(source_dir: &Path, dest_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(&dest_dir).unwrap();

    println!("cargo::rerun-if-changed={}", source_dir.display());

    let mut assets_rs = File::create(dest_dir.join("assets.rs"))?;

    write!(
        assets_rs,
        r#"
fn assets(name: &str) -> Option<(&'static [u8], mime::Mime)> {{
    match name {{
"#
    )?;

    for entry in WalkDir::new(source_dir) {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        let ext = path.extension().and_then(|ext| ext.to_str());

        let dest = dest_dir.join(path.strip_prefix(source_dir)?);
        let mut reference_path = path.to_owned();

        let (generated, mime) = match ext {
            Some("js") => {
                minify_js(path, &dest)?;
                (true, "APPLICATION_JAVASCRIPT_UTF_8")
            }
            Some("scss") => {
                compile_scss(path, &dest.with_extension("css"))?;
                reference_path.set_extension("css");
                (true, "TEXT_CSS")
            }
            Some("svg") => (false, "IMAGE_SVG"),
            _ => (false, "APPLICATION_OCTET_STREAM"),
        };

        write!(
            assets_rs,
            "        \"{}\" => Some((include_bytes!(concat!(env!(\"{}\"), \"/{}\")).as_slice(), mime::{})),\n",
            reference_path.strip_prefix(source_dir)?.display(),
            if generated { "OUT_DIR" } else { "CARGO_MANIFEST_DIR" },
            reference_path.display(),
            mime
        )?;
    }

    write!(assets_rs, "        _ => None\n    }}\n}}\n")?;

    Ok(())
}

fn minify_js(source: &Path, dest: &Path) -> anyhow::Result<()> {
    use minify_js::{minify, Session};

    let mut source = File::open(source).unwrap();
    let mut source_buf = Vec::new();
    source.read_to_end(&mut source_buf).unwrap();
    let source_buf = source_buf;
    drop(source);

    let mut target_buf = Vec::new();
    minify(
        &Session::new(),
        minify_js::TopLevelMode::Module,
        &source_buf,
        &mut target_buf,
    )
    .map_err(|e| anyhow!("{}", e))?;

    let mut dest = File::create(dest).unwrap();
    dest.write_all(&target_buf).unwrap();

    Ok(())
}

fn compile_scss(source: &Path, dest: &Path) -> anyhow::Result<()> {
    use css_minify::optimizations::{Level, Minifier};
    use grass::{Options, OutputStyle};

    let options = Options::default().style(OutputStyle::Compressed);

    let compiled = grass::from_path(source, &options)?;
    let minified = Minifier::default()
        .minify(&compiled, Level::Two)
        .map_err(|e| anyhow!("{}", e))?;

    let mut dest = File::create(dest)?;
    dest.write_all(&minified.into_bytes())?;

    Ok(())
}
