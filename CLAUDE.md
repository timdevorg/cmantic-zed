# cmantic-zed

Port of the C-Mantic VS Code extension to Zed, as a language server plus a thin Zed extension.

## Rules

- Never publish the Claude Code session URL in commits, the README, or anywhere else in the repo.

## Layout

- `extension.toml`, `Cargo.toml`, `src/lib.rs`: the Zed extension (crate `cmantic`, built to `wasm32-wasip2`).
  It only finds or downloads `cmantic-ls` and launches it. Lookup order: `lsp.cmantic.binary.path`
  setting, `cmantic-ls` on PATH, cached download, latest GitHub release.
- `server/`: the language server (crate `cmantic-ls`, tower-lsp-server + tree-sitter-cpp).
  All refactoring logic goes here.
- `reference/vscode-cmantic/`: original C-Mantic TypeScript source (MIT). Git-ignored, read-only reference.
  `src/commands/addDefinition.ts`, `src/CSymbol.ts` and `src/SourceDocument.ts` hold the Add Definition logic.

## Commands

```sh
cargo build -p cmantic-ls
cargo test -p cmantic-ls
cargo clippy --workspace --all-targets
cargo build --target wasm32-wasip2 --release   # extension
```

## Zed behaviour to design around (checked in Zed source, v1.28)

- If a code action has an `edit`, Zed applies it and does NOT run its `command`. Use one or the other.
- Snippet edits (`$0`) only act as snippets in the focused file; elsewhere Zed inserts plain text.
  To place the cursor in another file, use `workspace/applyEdit` then `window/showDocument` with a selection.
- Zed supports codeAction/resolve, documentChanges, create/rename/delete file operations,
  and `window/showMessageRequest` (buttons only, no free-text input).

## Releases

Tag `vX.Y.Z` to run `.github/workflows/release.yml`, which uploads
`cmantic-ls-<target-triple>.tar.gz` (`.zip` on Windows). `src/lib.rs` expects exactly these names.
Keep the version in `extension.toml` and both `Cargo.toml` files in sync.
