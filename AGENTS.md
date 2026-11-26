# Agent instructions for FreeDOM

## Communication and scope

- The user normally communicates in Russian; respond in Russian unless asked otherwise.
- Treat the user's latest request as the current priority. A reply such as "готово" usually means the previous patch was applied and the agreed next step may begin; inspect the actual repository before continuing.
- Briefly explain what changed, what was verified, any limitations, and what you would do next.
- Propose a concise commit title for every delivered change. Do not commit, push, stage changes in the user's repository, or rewrite history unless explicitly asked.
- Do not assume the repository has not moved. Its last known location is `/mnt/linux/src/RustroverProjects/freedom`; verify the active path and `git status` first.

## Preferred patch workflow

- By default, prepare a patch for the user to review and apply manually; do not directly modify their checkout unless they explicitly request it.
- Preserve all existing user changes. Inspect the worktree and avoid including unrelated files in the patch.
- Work in a temporary copy with its own Git index. Keep fixtures available without modifying across copies: compiled-in fixture paths can point at a previous checkout.
- Generate a complete patch including added files, renames, and binary data when applicable. Check it against the user's current checkout with `git apply --check`; verify that the patch matches the intended diff.
- Save the patch in an accessible workspace output directory. In the final response, link to it using its absolute path and provide exact `git apply` and relevant test commands. Do not say the checkout was changed before the user applies it.
- Remove only specifically identified, reproducible temporary build artifacts when necessary. Never reset or clean the user's checkout or `target` without explicit permission. Report material cleanup.

## Implementation and architecture

- Prefer cohesive components and reuse existing workspace crates rather than duplicating functionality or introducing parallel old/new implementations.
- Split crates along meaningful dependency boundaries, not simply to increase the crate count. Measure relevant build or test workflows before claiming a performance improvement; distinguish targeted tests from rebuilding the whole application.
- Keep JavaScript engine internals separate from browser host responsibilities when practical. Do not expose internal interpreter, DOM, or RegExp types merely to make a crate boundary compile.
- Prefer the project's own implementations for engine features and codecs. Do not silently replace them with external engines or codecs; discuss a new implementation dependency with the user when it changes that direction.
- Keep module and file organization proportionate. Avoid creating a directory containing only one small test module when an existing module or test file is clearer.
- Preserve the existing functionality during refactoring. If complete separation requires intermediate work, describe what remains coupled instead of claiming it is finished.

## Documentation and tests

- Write code comments and documentation in English. Where useful, link to the relevant primary specification and identify the section or algorithm; accurately state partial implementations and known limitations.
- Prefer meaningful production work and a small number of focused regression tests. Do not remove existing tests just to reduce the count, and do not introduce ignored tests to conceal failures.
- Run targeted tests first, then the relevant workspace tests, for example `cargo test -p <crate>` and `cargo test --workspace`. Check formatting with `cargo fmt --all -- --check`. Use a release build or tests when relevant.
- Fixtures required by committed tests should already be available in the project. Do not silently download, overwrite, or regenerate user fixtures.
- WPT summaries must be honest: distinguish upstream WPT runs from local fragment checks, synthetic harness probes, and experimental preview reftests. Do not claim full `testharness.js` or browser conformance from a bootstrap probe.
- If sandbox restrictions prevent tests requiring network or a local HTTP server, list the exact skipped tests and explain why. Do not report them as passed. Report build failures, existing warnings, and incomplete verification when relevant.

## Current context

- FreeDOM is a Rust browser workspace. URL, CSS, HTML, graphics core, fonts, image codecs, SVG, Canvas, fetch, and WebAssembly have separate crates; inspect `Cargo.toml` for the current layout.
- JavaScript is still integrated with browser host code. Its gradual separation should preserve DOM/Canvas behavior and existing tests rather than expose private internals or create a second engine.
- The fixture-only JSON reader is partial and uses the JavaScript tokenizer; it is not a full JSON validator. The synthetic testharness probe is not a complete HTML WPT runner.
