# FreeDOM

FreeDOM is an experimental web browser written in Rust. It implements its own
URL parser, HTML and CSS processing, layout, font and image decoding, SVG,
Canvas 2D, JavaScript, and WebAssembly rather than embedding an existing
browser engine.

The project is at an early stage. Many web platform features are missing or
partial, and the browser is not intended for everyday use.

## Repository layout

| Path                           | Contents                                                                                                              |
|--------------------------------|-----------------------------------------------------------------------------------------------------------------------|
| `src/`                         | Browser application: window and navigation, page loading, layout, painting, and local WPT tests                       |
| `crates/freedom-url`           | URL parsing ([URL Standard](https://url.spec.whatwg.org/))                                                            |
| `crates/freedom-html`          | HTML document model and parsing                                                                                       |
| `crates/freedom-css`           | CSS syntax, rules, cascade, and computed styles                                                                       |
| `crates/freedom-graphics-core` | Shared raster surfaces and drawing primitives                                                                         |
| `crates/freedom-font`          | SFNT, WOFF, and WOFF2 decoding, text layout, and glyph rendering                                                      |
| `crates/freedom-image`         | PNG, APNG, and JPEG decoders                                                                                          |
| `crates/freedom-svg`           | SVG parsing and rendering                                                                                             |
| `crates/freedom-canvas`        | Canvas 2D drawing core                                                                                                |
| `crates/freedom-fetch`         | HTTP resource fetching                                                                                                |
| `crates/freedom-js`            | JavaScript lexer, parser, interpreter, and RegExp engine, together with the DOM and Canvas bindings scripts use today |
| `crates/freedom-wasm`          | WebAssembly decoding and execution (not yet integrated into the browser)                                              |
| `testdata/`                    | Test fixtures (see [Test fixtures](#test-fixtures))                                                                   |
| `tools/`                       | Scripts that run the local WPT tests and summarize the results                                                        |

The JavaScript engine has its own crate but still contains its DOM and Canvas host bindings; separating them behind a
host interface is the next step.

## Building and running

The Rust toolchain is pinned in `rust-toolchain.toml`; `rustup` installs it automatically.

```bash
cargo run --release -- https://example.org
```

The first argument is the page to open; `https://example.org` is used when it is omitted.

Keyboard controls:

| Key | Action |
| --- | --- |
| Type, `Backspace`, `Enter` | Edit the address and navigate |
| `Escape` | Restore the current page address |
| `Alt+Left` / `Alt+Right` | Go back / forward in session history |
| `F3` | Toggle layout box outlines |
| `F4` | Print the styled tree to stderr |
| `F7` | Print the experimental box tree to stderr |
| `F8` | Describe the block under the cursor |
| `F9` | Toggle the experimental layout preview |
| `F10` | Toggle backgrounds in the layout preview |

Environment variables:

- `FREEDOM_FONT=<path>`: use a TrueType font file for text rendering.
- `FREEDOM_PROFILE_RENDER=1`: print frame timing information.

## Testing

```bash
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Some tests start a local HTTP server on `127.0.0.1`.

`python3 tools/wpt_report.py` runs the local WPT tests (`cargo test wpt::`) and
writes a summary to `artifacts/wpt/`; `python3 tools/wpt_dashboard.py` renders
it as `artifacts/wpt/index.html`. These are a curated subset o
[web-platform-tests](https://github.com/web-platform-tests/wpt) files checked
by FreeDOM's own harness, not an upstream WPT run.

`cargo run -- --probe-testharness` only checks that `testharness.js`
initializes in a synthetic host. It does not run WPT documents.

## Test fixtures

Third-party fixtures keep their own licenses:

- `testdata/wpt`: files from web-platform-tests, under the 3-Clause BSD
  License (`testdata/wpt/LICENSE.md`). Fonts in `testdata/wpt/fonts/noto` are
  under the SIL Open Font License 1.1 (`LICENSE-NOTO`).
- `testdata/test262`: files from Test262, under the license in
  `testdata/test262/LICENSE`.
- `testdata/fonts/noto`: a fixture generated from a WPT Noto font; see its
  `README.md`.
- `testdata/jpeg`: JPEG decoder fixtures and reference RGB output. Their
  provenance is not documented yet.

## License

FreeDOM is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. This does not apply to the third-party test fixtures listed
above.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in FreeDOM by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
