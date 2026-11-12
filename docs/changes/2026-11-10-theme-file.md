---
ledger+: TUI | a theme file named in config replaces the built-in palette
growth: +356 weighed keeping the parser beside Theme::new and the five library tests in that module; a second palette, a theme picker, and approximating the file on a 16-color terminal were weighed and left out
---

`~/.yi/config.json` may name `theme.dark` and `theme.light`. Each name loads `~/.yi/themes/<name>.json`, an omp theme file, and fills the colors the solo TUI and the console already paint. With no name, or a missing or unusable file, the built-in palette stays and startup prints one warning. A 16-color terminal keeps its named colors and does not approximate the file.
