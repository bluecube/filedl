# FileDl

A simple web app for public file downloads.
Currently WIP.

## Features
- File downloading
- Directory listing
- Directory download as ZIP
- Image thumbnails
- Display images as a gallery
- Download expiry
- Unlisted downloads
  - Does not show up in directory listing, needs specific "key" in query string to download.
- Owned vs linked downloads
  - Owned objects are stored in FileDl's data directory, deleted when download expires
- Minimal admin interface
  - No built-in authentication, intended to be secured by a reverse proxy

## Deployment

FileDl is designed to run behind a reverse proxy (e.g. nginx, Caddy). The server exposes two route scopes:

- `/download/...` — public interface for browsing and downloading files
- `/admin/...` — admin interface for managing objects

**The admin interface has no built-in authentication and must not be exposed to the internet.** Use your reverse proxy to restrict access to `/admin/`, for example by limiting it to a private network, VPN, or adding authentication at the proxy layer.

A Dockerfile is included for containerized deployments. Configuration is loaded from an optional TOML file (`-c`/`--config`) merged with environment variables prefixed `FILEDL_`. See `src/config.rs` for all available fields and defaults.
