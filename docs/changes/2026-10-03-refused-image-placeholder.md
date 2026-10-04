---
issue: Closes #860
raise: crate ai +28, crate types +164, tests +294
---
An image a provider would refuse no longer poisons a session. Every request now checks each image in its history where the request is built (`transform_messages`, which every wire calls), so an image saved before the kernel's check existed, or one the kernel's twelve-byte check passed, is caught too. An image that is not png, jpeg, gif or webp, is over 10,000,000 base64 chars, is not strict base64, does not open as its type, has no whole header (its first 24 bytes, a PNG's `IHDR` chunk, a WebP's `VP8`/`VP8L`/`VP8X` chunk), or is a PNG or GIF over 8000 px a side, is replaced in that request by `[image omitted: <type>, <N> KB of base64; the image <reason>, which the provider refuses. <remedy>]`. Before, a PNG signature over four zero bytes was kept, sent, and refused on every later turn.

The remedy follows the defect: an image too large or too wide names `attach_image`, which resizes; any other defect names a Pillow re-encode to JPEG and then `attach_image`; both run in the kernel venv in a test. Only the header is read, so a trailer after an image's end marker (a motion photo's MP4) or a body cut short is still sent, as decoders read it.

The check is one function, `yi_types::image::image_defect`, which the ipython tool also runs once per attachment, so its refusal row names the reason and the same remedy. The base64 and magic-number helpers moved from the kernel to yi-types, and the notebook's kitty writer uses the same strict base64 check.
