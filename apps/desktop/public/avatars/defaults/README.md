# Fox bundled avatars

Place the built-in user avatars in this directory. Fox generates the avatar list
before starting the development server and before each production build, so adding
or deleting an image does not require updating a hard-coded file list. Restart the
development server or rebuild after changing the directory.

Recommended asset rules:

- PNG, WebP, JPG, JPEG, AVIF, or SVG
- Square canvas, preferably 512 x 512
- Transparent background when appropriate
- Short lowercase file names such as `fox-blue.webp`
- Keep each asset below 300 KB where practical

Fox will initially expose only these bundled avatars. User-uploaded avatars, cropping,
replacement, and deletion are deferred to a later product iteration.
