use std::{
    env,
    fs::File,
    io::{Read, Write},
    path::Path,
};

use grass::{Options, OutputStyle};
use walkdir::WalkDir;

fn main() {
    let source_dir = Path::new("assets");
    let dest_dir = Path::new(&env::var("OUT_DIR").unwrap()).join("assets");

    std::fs::create_dir_all(&dest_dir).unwrap();

    println!("cargo::rerun-if-changed={}", source_dir.display());

    for entry in WalkDir::new(source_dir) {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        let Some(ext) = path.extension() else {
            continue;
        };

        let dest = dest_dir.join(path.strip_prefix(source_dir).unwrap());

        if ext == "js" {
            minify_js(path, &dest);
        } else if ext == "scss" {
            compile_scss(path, &dest.with_extension("css"));
        }
    }
}

fn process_file(source: &Path, dest: &Path, process: impl FnOnce(Vec<u8>) -> Vec<u8>) {
    println!("cargo::warning={}->{}", source.display(), dest.display());

    let mut source = File::open(source).unwrap();
    let mut source_buf = Vec::new();
    source.read_to_end(&mut source_buf).unwrap();
    let source_buf = source_buf;
    drop(source);

    let target_buf = process(source_buf);

    let mut dest = File::create(dest).unwrap();
    dest.write_all(&target_buf).unwrap();
}

fn minify_js(source: &Path, dest: &Path) {
    use minify_js::{minify, Session};

    process_file(source, dest, |source_buf| {
        let mut target_buf = Vec::new();
        minify(
            &Session::new(),
            minify_js::TopLevelMode::Module,
            &source_buf,
            &mut target_buf,
        )
        .unwrap();
        target_buf
    })
}

fn compile_scss(source: &Path, dest: &Path) {
    let options = Options::default().style(OutputStyle::Compressed);

    let compiled = grass::from_path(source, &options).unwrap();

    let minified = css_minify::optimizations::Minifier::default()
        .minify(&compiled, css_minify::optimizations::Level::Two)
        .unwrap();

    let mut dest = File::create(dest).unwrap();
    dest.write_all(&minified.into_bytes()).unwrap();
}
