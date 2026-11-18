# C-Mantic for Zed

C/C++ refactorings for [Zed](https://zed.dev), ported from the
[C-Mantic](https://github.com/BigBahss/vscode-cmantic) VS Code extension.

> **Status: early development.** The extension and language server start, but no
> refactorings are implemented yet. **Add Definition** is the first one being ported.

## How it works

Zed extensions can't add refactorings directly. Instead, this project ships a small
language server, `cmantic-ls`, that runs next to clangd and offers its refactorings as
code actions (`cmd-.` / `ctrl-.`). It parses C/C++ with tree-sitter, so it doesn't run a
second clangd or index your project.

The Zed extension downloads `cmantic-ls` from this repo's GitHub releases and starts it.

## Planned features

| Feature | Status |
|---|---|
| Add Definition (matching source file / this file / inline) | in progress |
| Add Declaration | planned |
| Generate Getters and Setters | planned |
| Generate equality, relational and stream output operators | planned |
| Add Header Guard, Add Include, Create Matching Source File | planned |
| Move Definition | planned |
| Update Function Signature | planned |

Switch Header/Source is already provided by Zed and clangd.

## Setup

Enable the server for C and C++ in your Zed `settings.json`, keeping clangd:

```json
{
  "languages": {
    "C++": { "language_servers": ["clangd", "cmantic", "..."] },
    "C": { "language_servers": ["clangd", "cmantic", "..."] }
  }
}
```

### Settings

Server settings go under `lsp.cmantic.settings`. They will be documented as features land.

```json
{
  "lsp": {
    "cmantic": {
      "settings": {}
    }
  }
}
```

## Development

The repo contains two Rust crates:

- `/` (crate `cmantic`): the Zed extension, compiled to WebAssembly by Zed.
- `server/` (crate `cmantic-ls`): the language server.

```sh
cargo build -p cmantic-ls        # build the server
cargo test -p cmantic-ls         # run the server tests
cargo build --target wasm32-wasip2 --release   # check that the extension compiles
```

To try it in Zed:

1. Run `zed: install dev extension` from the command palette and pick this folder.
2. Point Zed at your local server build so you don't need a release:

   ```json
   { "lsp": { "cmantic": { "binary": { "path": "/path/to/cmantic-zed/target/debug/cmantic-ls" } } } }
   ```

3. After rebuilding the server, run `editor: restart language server` in Zed.

## AI disclosure

This project is developed with the help of AI coding assistants (Claude Code by
Anthropic). Code is generated and edited with AI assistance and reviewed by the
maintainer. Commits made with AI assistance carry a `Co-Authored-By` trailer.

## License

MIT. See [LICENSE](LICENSE). Based on C-Mantic by Tyler Dennis, also MIT licensed.
