# IntentumDiff Patched Crates

This directory contains local crate patches used by the Rust engine and parser
workspace. Keep this set small. Every patch must have a clear reason, a removal
path, and tests that would fail if the patch stopped doing its job.

## Why Patches Exist

Patches are allowed only for release-critical reasons:

| Category | Crates | Why it exists | Removal path |
| --- | --- | --- | --- |
| Windows/AppControl build stability | `tree-sitter`, `tree-sitter-asm`, `tree-sitter-clojure`, `tree-sitter-dart`, `tree-sitter-dockerfile`, `tree-sitter-r`, `tree-sitter-squirrel`, `tree-sitter-typescript` | Some upstream crates rely on generated build scripts, missing vendored C sources, or build behavior that fails under Windows Application Control and WASI builds. The patches make native tests and Wasm parser builds deterministic. | Upstream the build-script/source changes, or switch to upstream versions that vendor the required sources and support the host/WASI split cleanly. |
| Missing or disabled grammar sources | `tree-sitter-abap`, `tree-sitter-freebasic`, `tree-sitter-pascal` | These grammars are not in a shape that can be consumed directly by the release parser workspace. | Replace with maintained upstream grammar crates or retire the language until a Rust/Wasm FullParse grammar is available. |
| Grammar correctness | `tree-sitter-json` (`grammar.js`, the regenerated `src/parser.c` and `src/grammar.json`, and the prebuilt `lib/libtree_sitter_json.a` linked for `wasm32-wasip2`) | Upstream 0.24.8 accepted only `-` in a number's exponent, but RFC 8259 allows `+` too: `1e+22` split into ERROR + `22`, so `1e+22` -> `2e+22` read as unchanged (json-parser#6). The exponent now takes an optional `+` or `-`. Test: `exponents_with_an_explicit_plus_sign_are_whole_numbers`. Regenerate with tree-sitter CLI 0.24.7 (it reproduces the committed parser byte for byte from the unmodified grammar) and rebuild the archive with wasi-sdk 28 (`clang --target=wasm32-wasip2 --sysroot=<wasi-sysroot> -std=c11 -O2 -Isrc -c src/parser.c`). | Upstream the grammar fix and move to a release with it. |
| Supply-chain and release determinism | `quote`, `serde_core`, `serde_json`, `wit-bindgen-rust-macro`, `zmij` | These are pinned local copies used to stabilize release builds and avoid dependency behavior that breaks the native wheel/Wasm build contract. | Remove once upstream versions satisfy the release constraints without local edits. |

## Rules For New Patches

- Prefer upstream crates first.
- Prefer a small target-aware `build.rs` patch over broad source changes.
- Do not patch to hide parser correctness failures.
- Do not patch to bypass security checks.
- Add a comment in the root `Cargo.toml` patch section if the reason is not obvious.
- Add or update a focused test before adding a patch.

## Review Checklist

- Can this be solved by upgrading the crate?
- Can this be upstreamed?
- Does the patch affect native tests, Wasm builds, or both?
- Is the patch still needed on Windows ARM64?
- Is the patch still needed after moving parser crates under `crates/parsers/`?

## Parser Crate Layout

Parser crates live under a dedicated parser namespace so the engine, SDK,
renderers, and parser implementations are visually separated.

Target layout:

```text
crates/
  parsers/
    python-parser/
    json-parser/
    ...
  rust-core-host/
  index-engine/
  renderers/
    terminal-renderer/
    patch-renderer/
    html-renderer/
    llm-renderer/
  sdk/
  patches/
```

Only parser crates are grouped today. Renderer/core grouping is intentionally
left as a smaller follow-up so parser path churn does not hide engine changes.
