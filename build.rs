use std::{
    env,
    fs::{File, create_dir_all, read, read_to_string, write},
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use brotli::{BrotliCompress, enc::BrotliEncoderParams};
use walkdir::WalkDir;

fn main() {
    let do_minify = env::var("PROFILE").unwrap() != "debug";
    process_assets(
        Path::new("assets"),
        &Path::new(&env::var("OUT_DIR").unwrap()).join("assets"),
        do_minify,
    )
    .unwrap();
}

fn process_assets(source_dir: &Path, dest_dir: &Path, do_minify: bool) -> anyhow::Result<()> {
    create_dir_all(dest_dir).unwrap();

    println!("cargo::rerun-if-changed={}", source_dir.display());

    let mut assets_rs = File::create(dest_dir.join("assets.rs"))?;

    write!(
        assets_rs,
        r#"
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

        let (converted_name, content, mime) = match ext {
            Some("js") => minify_js(&path, name, do_minify)?,
            Some("scss") => compile_scss(&path, name, do_minify)?,
            Some("svg") => copied_asset(&path, name, "IMAGE_SVG")?,
            _ => copied_asset(&path, name, "APPLICATION_OCTET_STREAM")?,
        };

        let dest_path = dest_dir.join(&converted_name);
        write(&dest_path, &content)?;

        let dest_compressed_path = add_extension(&dest_path, ".br");
        brotli_compress(&content, &dest_compressed_path)?;

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
    }

    writeln!(assets_rs, "        _ => None\n    }}")?;
    writeln!(assets_rs, "}}")?;

    Ok(())
}

fn add_extension(path: &Path, extension: &str) -> PathBuf {
    // Create a new PathBuf from the original path
    let mut new_path = path.to_path_buf();

    // Extract the OsString from the file stem
    if let Some(file_name) = new_path.file_name() {
        let mut new_file_name = file_name.to_os_string();
        // Add the new extension
        new_file_name.push(extension);
        // Set the new file name back to the new PathBuf
        new_path.set_file_name(new_file_name);
    } else {
        new_path.push(extension);
    }

    new_path
}

fn minify_js(
    source: &Path,
    name: &Path,
    do_minify: bool,
) -> anyhow::Result<(PathBuf, Vec<u8>, &'static str)> {
    use oxc::{
        codegen::{Codegen, CodegenOptions},
        minifier::Minifier,
        parser::Parser,
        span::SourceType,
    };

    let source_buf = read_to_string(source)?;

    let minified_bytes = if do_minify {
        let allocator = Default::default();

        let source_type = SourceType::from_path(source)?;
        let parsed = Parser::new(&allocator, &source_buf, source_type).parse();
        let mut program = parsed.program;

        let minified = Minifier::new(Default::default()).minify(&allocator, &mut program);

        Codegen::new()
            .with_options(CodegenOptions::minify())
            .with_scoping(minified.scoping)
            .build(&program)
            .code
            .into_bytes()
    } else {
        source_buf.into_bytes()
    };

    Ok((
        name.to_path_buf(),
        minified_bytes,
        "APPLICATION_JAVASCRIPT_UTF_8",
    ))
}

fn compile_scss(
    source: &Path,
    name: &Path,
    do_minify: bool,
) -> anyhow::Result<(PathBuf, Vec<u8>, &'static str)> {
    use css_minify::optimizations::{Level, Minifier};
    use grass::{Options, OutputStyle};

    let options = Options::default().style(OutputStyle::Compressed);

    let compiled = grass::from_path(source, &options)?;
    let minified = Minifier::default()
        .minify(&compiled, Level::Two)
        .map_err(|e| anyhow!("{}", e))?;

    Ok((
        name.with_extension("css"),
        if do_minify {
            minified.into_bytes()
        } else {
            compiled.into_bytes()
        },
        "TEXT_CSS",
    ))
}

fn copied_asset(
    source: &Path,
    name: &Path,
    mime: &'static str,
) -> anyhow::Result<(PathBuf, Vec<u8>, &'static str)> {
    Ok((name.to_path_buf(), read(source)?, mime))
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
