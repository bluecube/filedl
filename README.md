# FileDl

A simple web app for public file downloads.
Currently WIP.

## Features
- [x] File downloading
- [x] Directory listing
- [ ] Directory download as ZIP
- [x] Image thumbnails
- [ ] Display images as a gallery
- [ ] Download expiry
- [ ] Unlisted downloads
  - Does not show up in directory listing, needs specific "key" in query string to download.
- [ ] Owned vs linked downloads
  - Owned objects are stored in FileDl's data directory, deleted when download expires
- [ ] Minimal admin interface
  - No authentization, using reverse proxy to limit access
