---
issue: Closes #860
raise: crate ai +27, crate types +168, tests +192
---
An image a provider would refuse no longer poisons a session. Every request now checks each image in its history where the request is built (`transform_messages`, which every wire calls), so an image saved before the kernel's check existed, or one the kernel's header check passed, is caught too. An image that is not png, jpeg, gif or webp, is over 10,000,000 base64 chars, is not strict base64, does not open as its type, is cut short (no PNG `IEND`, JPEG `FFD9`, GIF trailer, or a WebP whose RIFF size is not its length), or is a PNG or GIF over 8000 px a side, is replaced in that request by `[image omitted: <type>, <N> KB of base64; the image <reason>, which the provider refuses. To see the file, run `print(await attach_image(path))` in ipython]`. Before, a PNG signature over four zero bytes or a real PNG cut at any four-character boundary was kept, sent, and refused on every later turn.

The check is one function, `yi_types::image::image_defect`, which the ipython tool also runs before an attachment reaches the model, so its refusal row now names the reason. The base64 and magic-number helpers moved from the kernel to yi-types, and the notebook's kitty writer uses the same strict base64 check. JPEG and WebP sides are not read; `attach_image` already resizes to 8000 px.
