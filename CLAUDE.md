# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
# Build
cargo build
cargo build --release

# Run (requires a config file)
cargo run -- --config test_env/testing.toml
# Or use the convenience script (also uses cargo-watch for auto-reload):
./test_env/run_test_env.sh

# Test
cargo test
cargo test <test_name>   # run a single test

# Lint
cargo clippy

# Docker
# The project contains a dockerfile, to use the dockerfile with the test environment use:
./test_env/docker.sh
```

Logging is controlled via `RUST_LOG` (e.g., `RUST_LOG=debug`).

## Configuration

Config is loaded via [figment](https://docs.rs/figment): optional TOML file (`-c`/`--config`) merged with environment variables prefixed `FILEDL_`. Required fields are `data_path` and `linked_objects_root`. See `src/config.rs` for all fields and defaults.

The test config at `test_env/testing.toml` uses `linked_objects_root = "/"` to expose the entire filesystem.

## Architecture

### HTTP Layer (`src/main.rs`, `src/lib.rs`)
`main.rs` starts the Actix-web server and calls `AppData::shutdown()` once it stops. `AppData::shutdown()` runs even when the server fails, if both fail, `MainError::DoubleError` holds both errors. `main` returns errors wrapped in `snafu::Report`, so they are printed with their display messages and source chain instead of `Debug`. Routes are set up in `lib.rs` (`configure_services`, used by the `build_app!` macro, which the integration tests use too), with two scopes:
- `/download/...` → `pages::configure_pages` — public read-only interface
- `/admin/...` → `admin_pages::configure_admin_pages` — admin operations (no built-in auth; intended to be secured by reverse proxy)

### Central State (`src/app_data.rs`)
`AppData` (wrapped in `Arc<AppData>`) is shared across requests. It holds:
- `Config` — loaded once at startup
- `RwLock<Storage<Object>>` — in-memory metadata registry, persisted to `data_path/metadata.json`
- `CachedThumbnails` — in-memory LRU thumbnail cache keyed by (path, size, modtime, resolution, type)
- `static_content_hash` — random u32 generated at startup for cache-busting static assets

### Object Model
An `Object` is a registry entry (`Storage<Object>`, keyed by object ID) with two ownership modes:
- `Owned` — data lives at `data_path/owned_data/<id>`
- `Linked` — data lives at `linked_objects_root/<relative_path>`

A linked target can disappear from the filesystem at any time. The public root listing (`list_objects`) skips such objects, and the admin listing (`list_objects_admin`) still returns them with the IO error in `AdminObjectInfo::item`, so they can be fixed or deleted from the dashboard.

Objects can optionally have an `unlisted_key` (access requires `?key=<value>` in the URL) and an expiry time. Expired objects are rejected by `resolve_object` and skipped by `list_objects`, and the expiry task (started by `AppData::with_config` when `spawn_expiry_task` is set) deletes them, together with their owned data, when they expire. Its wait between scans is clamped to `MIN_EXPIRY_WAIT`..`MAX_EXPIRY_WAIT` (1 s..4 h), so it can't spin on an object it fails to remove, and it rechecks the wall clock after a suspend or clock change.

`resolve_object(path, key)` in `AppData` resolves a URL path to a `ResolvedObject`, validating the key and checking against directory traversal. The path arrives percent-decoded, so the sub-path can contain `%2F`-encoded slashes (e.g. `obj//etc/passwd`); it is joined with `RelativePath::to_path`, which ignores leading and repeated slashes (`PathBuf::push` would replace the base with an absolute path), and `..` components are rejected explicitly because `to_path` keeps them. `to_path` also drops a trailing slash, so a trailing slash on a non-directory is rejected explicitly (404, like `NotADirectory` from the filesystem); over HTTP this only happens with an encoded `%2F`, since the `NormalizePath::trim` middleware strips plain trailing slashes. The checks are purely textual: symlinks inside linked directories are followed. Object IDs are validated by `validate_object_id` to be a single path component (no `/`, not empty, `.` or `..`).

### Download Modes (query param `?mode=`)
- `default` — serve file inline or render directory listing
- `download` — force-download (or ZIP for directories via `zippity`)
- `thumbnail` — return JPEG or AVIF thumbnail (size rounded to 64/128/256px)
- `assets` — serve embedded static assets (CSS, JS, SVG icons)

### Thumbnail Pipeline (`src/thumbnails/`)
CPU-intensive thumbnail generation runs on a rayon thread pool via `tokio-rayon`. Supports images (any format readable by the `image` crate) and PDFs (via `hayro`). Results are cached in a `quick_cache` weighted by byte size (default 20 MB).

### Asset Build Pipeline (`build.rs`)
At build time, files from `assets/` are processed and embedded into the binary via `include_bytes!`:
- `.scss` → compiled with `grass` + minified with `css-minify` → `.min.css`
  - `css-minify` rewrites `background-*` longhands (`background-image`, `background-color`, `background-position`, also inside `@keyframes`) into the `background` shorthand, which resets the other background properties. Put related background properties in the same rule (after the one that gets rewritten), and animate `background-position-x` instead of `background-position`. Check the generated `.min.css` in `target/*/build/filedl-*/out/assets/` after changing backgrounds.
- `.js` → minified with `oxc` → `.min.js` + source map
- All assets → brotli-compressed at quality 11

The server serves both the raw and brotli-compressed versions, selecting based on `Accept-Encoding: br`.

### HTML Templates (`src/templates/`)
HTML is generated server-side using the `horrorshow` crate (a Rust macro DSL). Templates are Rust structs implementing `RenderOnce`. The main template is `DirListing`, wrapped in a `Page` shell.

### Storage (`src/storage.rs`)
`Storage<T>` is a plain in-memory `HashMap<Arc<str>, T>` (sync API, no locking of its own) backed by a JSON file, and is self-contained: `Storage::new` (async, runs on Tokio) loads the file and spawns a dump task owned by the storage. Every modification serializes the whole map and sends the bytes to the task through a `watch` channel; the task writes the latest snapshot right away (atomically via the `atomic-write-file` crate, keeping the old file's permissions, on a blocking thread) and then waits `DUMP_COOLDOWN` (5 s) before the next write, so a change in a quiet period is written immediately and a burst costs at most one extra write. The task never touches the map. Modifying methods return `Result<_, WriteError>`: `StorageAlreadyShutDown` once shutdown has started (the map is left unchanged), or `SerializationError` (the change stays in memory and is persisted with the next successful one; any returned values are lost). `AppDataError::StorageError` wraps it, mapping `StorageAlreadyShutDown` to 503. Failed writes are logged and retried after the cooldown. Dropping the channel's sender stops the task after writing any pending snapshot right away, even during the cooldown: `Storage::shutdown(&mut self)` does that and waits for the write (returning its error), dropping the `Storage` does it without waiting; `AppData::shutdown()` calls it.

Callers hold `AppData`'s `RwLock` while doing filesystem operations that must be atomic with the metadata: delete → remove from storage, then remove data (under a downgraded read lock, so readers aren't blocked by slow removals); upload → create in storage, then move the data into place. Because the metadata file is written with a delay, a crash can leave the file slightly behind the data on disk (orphaned files, or objects whose data is gone); listings tolerate objects with missing data, and deleting an owned object whose data is already missing succeeds (with a warning). If removing the data fails for another reason, the metadata stays removed and the error is returned; the data is left orphaned rather than restoring metadata that could point at half-deleted data.

## Code style

Run `cargo fmt` after every finished change.

Do not introduce new unused code warnings. The codebase currently has some that are known and will be resolved soon — do not add to them.

Keep tests up to date, with as much as possible being tested in unit tests,
general functionality also in integration tests.

Keep CLAUDE.md up to date.

Don't use worktrees.

## Testing approaches
Utilizes property tests through `proptest`, the found proptest failures in `/proptest-regressions/` are kept commited in the repository for later regression testing.
