# Third-party notices

eagleVoice includes or depends on the third-party work listed below. Rust crates and npm
packages pulled in through `app/src-tauri/Cargo.toml` and `app/package.json` keep their own
licenses, which are shipped with each package; this file covers the parts that are bundled in
this repository or that need a specific note.

## Handy

- Project: [Handy](https://github.com/cjpais/Handy) by CJ Pais
- License: MIT, full text in [`app/LICENSE`](app/LICENSE)
- What comes from it: eagleVoice is a fork of Handy. The Tauri shell, audio capture, model
  management, tray, the base settings UI and the NSIS installer template started as Handy's
  code.
- Handy's name, logo, icon and brand assets are not covered by its open-source license.
  eagleVoice uses its own name and icons and is not endorsed by or affiliated with Handy or
  CJ Pais.

## Silero VAD model

- File: `app/src-tauri/resources/models/silero-vad-v6.2-F32.gguf`
- Original model: [Silero VAD](https://github.com/snakers4/silero-vad) by the Silero Team
- Conversion: GGUF conversion published by `handy-computer` on Hugging Face
- License: MIT

```
MIT License

Copyright (c) 2020-present Silero Team

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## UI sounds

- Files: `app/src-tauri/resources/marimba_start.wav`, `marimba_stop.wav`, `pop_start.wav`,
  `pop_stop.wav`
- Inherited from Handy and distributed with it under its MIT license (see above).

## Git dependencies

These Rust dependencies are fetched from GitHub instead of crates.io:

- [`rustdesk-org/rdev`](https://github.com/rustdesk-org/rdev), default branch: a fork of
  [`Narsil/rdev`](https://github.com/Narsil/rdev) by Nicolas Patry, MIT (`LICENSE` in the
  repository, `license = "MIT"` in its `Cargo.toml`).
- [`ahkohd/tauri-nspanel`](https://github.com/ahkohd/tauri-nspanel), branch `v2.1`, macOS only
  (not built on Windows): by Victor Aremu, MIT or Apache-2.0 (the repository ships both
  `LICENSE_MIT` and `LICENSE_APACHE-2.0`).
- [`cjpais/hf-hub`](https://github.com/cjpais/hf-hub), branch `cancellable-downloads`: a fork of
  [`huggingface/hf-hub`](https://github.com/huggingface/hf-hub), Apache-2.0.
- [`cjpais/tao`](https://github.com/cjpais/tao), pinned revision used through
  `[patch.crates-io]` for `tao` and `tao-macros`: a fork of
  [`tauri-apps/tao`](https://github.com/tauri-apps/tao), Apache-2.0 as declared in its
  `Cargo.toml`.

## Downloadable speech recognition models

The models listed in `app/src-tauri/src/catalog/catalog.json` are not part of this repository.
The app downloads them on request from Handy's hosting (`blob.handy.computer` and the
`handy-computer` organization on Hugging Face). Each model has its own license, recorded in the
`license` field of the catalog: they include MIT, Apache-2.0, CC-BY-4.0, "other" and at least one
non-commercial license (for example Canary 1B, `cc-by-nc-4.0`). Check the license of a model
before using it, especially for commercial use.
