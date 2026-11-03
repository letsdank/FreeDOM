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