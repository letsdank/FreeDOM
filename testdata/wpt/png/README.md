# PNG fixtures

- `support/trns-high-bits-set.png` comes from
  [web-platform-tests](https://github.com/web-platform-tests/wpt/blob/master/png/support/trns-high-bits-set.png).
- Original browser test:
  [`png/trns-chunk.html`](https://github.com/web-platform-tests/wpt/blob/master/png/trns-chunk.html).
- SHA-256: `a5e866a28f4a19504981a8e839799ab1511b4bfa182e84068a657797cb52e315`.

FreeDOM currently checks the decoded source alpha directly. It does not yet
execute the original Canvas-based WPT test.

## 16-bit APNG fixture

- `apng/support/033.png` comes from
  [web-platform-tests](https://github.com/web-platform-tests/wpt/blob/master/png/apng/support/033.png).
- Original browser reftest:
  ['png/apng/fdAT-16bit.html`](https://github.com/web-platform-tests/wpt/blob/master/png/apng/fdAT-16bit.html).
- SHA-256: `82da2ad317a5b3ed2659f063354271ae4051fc3fad726c0092db8b7e6442d351`.

FreeDOM checks only the static IDAT default image. APNG frame playback
and the original browser reftest are not implemented by this test.

## PngSuite indexed-color fixtures

- `../css/css-ui/support/PTS/basi3p02.png` is the interlaced image:
  [WPT source](https://github.com/web-platform-tests/wpt/blob/master/css/css-ui/support/PTS/basi3p02.png).
- `../css/css-ui/support/PTS/basn3p02.png` is the non-interlaced image:
  [WPT source](https://github.com/web-platform-tests/wpt/blob/master/css/css-ui/support/PTS/basn3p02.png).
- Original manual browser test:
  [`css/css-ui/cursor-image-png-024-manual.html`](https://github.com/web-platform-tests/wpt/blob/master/css/css-ui/cursor-image-png-024-manual.html).
- Interlaced SHA-256: `85f304eaaf6831b2bcc2c08892b947fef52f06337aaf23312d6326e367d73f77`.
- Non-interlaced SHA-256: `0466bb7ed9984cf03b70704564bcffab1df8ec0e8167473ba0f75e4fedce5a8f`.

FreeDOM compares the decoded pixel data and checks selected pixel values.
This does not execute the original manual CSS cursor test.

## Unknown ancillary chunks

- `errors/support/invalid-unknown-ancillary.png`
- `errors/support/invalid-unknown-ancillary-after-IDAT.png`
- `errors/support/no-invalid-chunks.png`

These files come from [web-platform-tests](https://github.com/web-platform-tests/wpt/tree/master/png/errors/support).

Original browser reftests:

- [`unknown-ancillary-error-recovery.html`](https://github.com/web-platform-tests/wpt/blob/master/png/errors/unknown-ancillary-error-recovery.html)
- [`unknown-ancillary-error-recovery-2.html`](https://github.com/web-platform-tests/wpt/blob/master/png/errors/unknown-ancillary-error-recovery-2.html)

FreeDOM compares decoded RGBA pixels against the reference image. This test
does not execute the original HTML reftests or test browser rendering.