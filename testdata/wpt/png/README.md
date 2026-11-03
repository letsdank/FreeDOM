# PNG fixtures

- `support/trns-high-bits-set.png` comes from
  [web-platform-tests](https://github.com/web-platform-tests/wpt/blob/master/png/support/trns-high-bits-set.png).
- Original browser test:
  [`png/trns-chunk.html`](https://github.com/web-platform-tests/wpt/blob/master/png/trns-chunk.html).
- SHA-256: `a5e866a28f4a19504981a8e839799ab1511b4bfa182e84068a657797cb52e315`.

FreeDOM currently checks the decoded source alpha directly. It does not yet
execute the original Canvas-based WPT test.