use std::{
    env,
    ffi::OsStr,
    fs::{File, create_dir_all, metadata, read, read_to_string, write},
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use brotli::{BrotliCompress, enc::BrotliEncoderParams};
use walkdir::WalkDir;

fn main() {
    process_assets(
        Path::new("assets"),
        &Path::new(&env::var("OUT_DIR").unwrap()).join("assets"),
    )
    .unwrap();
}

fn process_assets(source_dir: &Path, dest_dir: &Path) -> anyhow::Result<()> {
    create_dir_all(dest_dir).unwrap();

    println!("cargo::rerun-if-changed={}", source_dir.display());

    let mut assets_rs = File::create(dest_dir.join("assets.rs"))?;

    let mut asset_count: usize = 0;
    let mut uncompressed_asset_size: u64 = 0;
    let mut compressed_asset_size: u64 = 0;

    write!(
        assets_rs,
        r#"
// use std::str::FromStr as _;

fn assets(name: &str) -> Option<(&'static [u8], &'static [u8], mime::Mime)> {{
    match name {{
"#
    )?;

    for entry in WalkDir::new(source_dir) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.into_path();
        let name = path.strip_prefix(source_dir)?;
        let ext = path.extension().and_then(|ext| ext.to_str());

        for (converted_name, content, mime) in match ext {
            Some("js") => minify_js(&path, name)?,
            Some("scss") => compile_scss(&path, name)?,
            Some("svg") => copied_asset(&path, name, "IMAGE_SVG")?,
            _ => copied_asset(&path, name, "APPLICATION_OCTET_STREAM")?,
        } {
            asset_count += 1;
            append_asset(
                &mut assets_rs,
                &mut uncompressed_asset_size,
                &mut compressed_asset_size,
                dest_dir,
                converted_name,
                content,
                mime,
            )?;
        }
    }

    writeln!(assets_rs, "        _ => None\n    }}")?;
    writeln!(assets_rs, "}}")?;

    println!(
        "cargo::warning=build.rs processed {} assets, {}kB uncompressed, {}kB compressed, {}kB total",
        asset_count,
        uncompressed_asset_size / 1024,
        compressed_asset_size / 1024,
        (uncompressed_asset_size + compressed_asset_size) / 1024
    );

    Ok(())
}

fn append_asset(
    assets_rs: &mut File,
    uncompressed_asset_size: &mut u64,
    compressed_asset_size: &mut u64,
    dest_dir: &Path,
    converted_name: PathBuf,
    content: Vec<u8>,
    mime: &'static str,
) -> anyhow::Result<()> {
    let dest_path = dest_dir.join(&converted_name);
    write(&dest_path, &content)?;

    let dest_compressed_path = dest_path.with_added_extension("br");
    brotli_compress(&content, &dest_compressed_path)?;

    *uncompressed_asset_size += content.len() as u64;
    *compressed_asset_size += metadata(dest_compressed_path)?.len();

    writeln!(
        assets_rs,
        "        \"{}\" => Some((",
        converted_name.display(),
    )?;
    writeln!(
        assets_rs,
        "            include_bytes!(concat!(env!(\"OUT_DIR\"), \"/assets/{}\")).as_slice(),",
        converted_name.display(),
    )?;
    writeln!(
        assets_rs,
        "            include_bytes!(concat!(env!(\"OUT_DIR\"), \"/assets/{}.br\")).as_slice(),",
        converted_name.display(),
    )?;
    writeln!(assets_rs, "            mime::{}", mime)?;
    writeln!(assets_rs, "        )),",)?;

    Ok(())
}

fn minify_js(source: &Path, name: &Path) -> anyhow::Result<Vec<(PathBuf, Vec<u8>, &'static str)>> {
    use oxc::{
        codegen::{Codegen, CodegenOptions},
        minifier::Minifier,
        parser::Parser,
        span::SourceType,
    };
    let mut ret = Vec::new();

    let source_buf = read_to_string(source)?;

    let allocator = Default::default();

    let sourcemap_name = name.with_added_extension("map");

    let source_type = SourceType::from_path(source)?;
    let parsed = Parser::new(&allocator, &source_buf, source_type).parse();
    let mut program = parsed.program;

    let minified = Minifier::new(Default::default()).minify(&allocator, &mut program);

    let minified_codegen = Codegen::new()
        .with_options(CodegenOptions {
            source_map_path: Some(name.to_path_buf()),
            ..CodegenOptions::minify()
        })
        .with_scoping(minified.scoping)
        .build(&program);
    let minified_code = format!(
        "{}\n//# sourceMappingURL={}?mode=assets",
        minified_codegen.code,
        sourcemap_name.display(),
    )
    .into_bytes();

    ret.push((
        name.to_path_buf(),
        source_buf.into_bytes(),
        "APPLICATION_JAVASCRIPT_UTF_8",
    ));
    ret.push((
        sourcemap_name,
        minified_codegen.map.unwrap().to_json_string().into_bytes(),
        "APPLICATION_JSON",
    ));
    ret.push((
        add_before_ext(name, "min").unwrap(),
        minified_code,
        "APPLICATION_JAVASCRIPT_UTF_8",
    ));

    Ok(ret)
}

fn compile_scss(
    source: &Path,
    name: &Path,
) -> anyhow::Result<Vec<(PathBuf, Vec<u8>, &'static str)>> {
    let mut ret = Vec::new();
    // TODO: Figure out how to do source map for grass and scss, then include the scss source
    // let mut ret = copied_asset(source, name, "Mime::from_str(\"text/x-scss\").unwrap()")?;

    use css_minify::optimizations::{Level, Minifier};
    use grass::{Options, OutputStyle};

    let options = Options::default().style(OutputStyle::Compressed);

    let compiled = grass::from_path(source, &options)?;
    let minified = Minifier::default()
        .minify(&compiled, Level::Two)
        .map_err(|e| anyhow!("{}", e))?;

    ret.push((
        name.with_extension("min.css"),
        minified.into_bytes(),
        "TEXT_CSS",
    ));

    Ok(ret)
}

fn copied_asset(
    source: &Path,
    name: &Path,
    mime: &'static str,
) -> anyhow::Result<Vec<(PathBuf, Vec<u8>, &'static str)>> {
    Ok(vec![(name.to_path_buf(), read(source)?, mime)])
}

fn brotli_compress(source: &[u8], dest: &Path) -> anyhow::Result<()> {
    let mut dest = File::create(dest)?;
    let params = BrotliEncoderParams {
        quality: 11,
        size_hint: source.len(),
        ..Default::default()
    };

    BrotliCompress(&mut Cursor::new(source), &mut dest, &params)?;

    Ok(())
}

fn add_before_ext(path: &Path, s: impl AsRef<OsStr>) -> Option<PathBuf> {
    let extension = path.extension()?;
    let mut path = path.with_extension(s);
    path.add_extension(extension);
    Some(path)
}
