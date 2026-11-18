# C-Mantic for Zed

C/C++ refactorings for [Zed](https://zed.dev), ported from the
[C-Mantic](https://github.com/BigBahss/vscode-cmantic) VS Code extension.

> **Status: early development.** **Add Definition** is implemented; the other
> refactorings are still being ported.

## How it works

Zed extensions can't add refactorings directly. Instead, this project ships a small
language server, `cmantic-ls`, that runs next to clangd and offers its refactorings as
code actions (`cmd-.` / `ctrl-.`). It parses C/C++ with tree-sitter, so it doesn't run a
second clangd or index your project.

The Zed extension downloads `cmantic-ls` from this repo's GitHub releases and starts it.

## Planned features

| Feature | Status |
|---|---|
| Add Definition (matching source file / this file / inline) | done |
| Add all missing definitions (matching source file / this file) | done |
| Generate Constructor with member initializers | done (all members, no picker) |
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

## Add Definition

Put the cursor on a function declaration that has no definition yet and press `cmd-.`
(`ctrl-.`). Depending on the declaration you get:

- **Add Definition in matching source file**: for declarations in a header that has a
  matching source file (same name, source extension; searched next to the header, then
  under its parent directory, then in the workspace). Not offered for `inline`,
  `constexpr`, `consteval` or template functions, which must stay in the header. The new
  definition is opened with the cursor inside its body.
- **Add Definition in this file**: after the class or next to related definitions. In a
  header the definition gets `inline` unless it is already `inline`/`constexpr`/`consteval`
  (also when it lands inside a namespace block, which C-Mantic missed).
- **Add Definition inline**: for class members, turns the declaration into a definition
  inside the class.

Like C-Mantic, the definition is placed next to the definitions of neighbouring
declarations when there are any, else at the end of the matching `namespace` block, else
at the end of the file. Scope qualifiers are added only for scopes the definition isn't
already inside, default arguments and specifiers like `virtual`, `static` and `override`
are dropped, and multi-line parameter lists keep their alignment.

Nothing is offered for pure virtual, `= default` and `= delete` functions, or when a
definition already exists in the file or its matching source file.

### Constructors

On a constructor declaration of a class with base classes or non-static data members, the
menu additionally offers **Generate Constructor in matching source file / in this file
(initialize all members)**. They add a member initializer list with every base class
followed by every non-static data member, in declaration order, and put the cursor in the
first initializer:

```cpp
Foo::Foo(int x)
    : Base(),
      count(),
      name()
{
}
```

`cpp.bracedInitialization` switches `()` to `{}`. C-Mantic instead asks which initializers
to use (including a delegating constructor); Zed has no multi-select picker, so this port
always initializes everything (delete what you don't need). The plain Add Definition actions
still give constructors an empty body. There is no inline variant, to keep the menu short:
use **Add Definition inline** and write the initializers in place.

## Add all missing definitions

Adds definitions for every function declaration in the file that has none, in one edit:

- **Add all missing definitions in matching source file**: skips declarations that must
  stay in the header (`inline`, `constexpr`, `consteval`, templates); use the "in this
  file" variant for those.
- **Add all missing definitions in this file**.

Offered when the cursor is on a class or struct head (its name, base list, ... but not its
body) and the file has at least one undefined declaration. Not offered on individual
declarations, to keep the menu there short.
Definitions are added in declaration order and placed like single ones, so a definition
follows the one just added for the previous declaration. Constructors get initializers only
for members that require them (references and `const` members without a default member
initializer). The cursor moves into the first new body when `revealNewDefinition` is on.

C-Mantic shows a picker to choose functions and the target file; this port always adds all
of them.

## Settings

Server settings go under `lsp.cmantic.settings` (the same keys are accepted in
`initialization_options`). All are optional; these are the defaults:

```json
{
  "lsp": {
    "cmantic": {
      "settings": {
        "headerExtensions": ["h", "hpp", "hh", "hxx"],
        "sourceExtensions": ["c", "cpp", "cc", "cxx"],
        "cpp": {
          "curlyBraceFormat": { "function": "newLineCtorDtor" },
          "bracedInitialization": false
        },
        "c": { "curlyBraceFormat": { "function": "newLine" } },
        "revealNewDefinition": true
      }
    }
  }
}
```

| Setting | Description |
|---|---|
| `headerExtensions`, `sourceExtensions` | File extensions (without the dot) used to tell headers and source files apart and to find the matching file. |
| `cpp.curlyBraceFormat.function`, `c.curlyBraceFormat.function` | Where the opening brace of a new function goes: `"newLine"`, `"sameLine"`, or `"newLineCtorDtor"` (new line for constructors and destructors only). The language of the file that receives the definition decides which one applies. |
| `cpp.bracedInitialization` | Use `member{}` instead of `member()` in generated constructor initializer lists. |
| `revealNewDefinition` | After adding a definition to the matching source file, or adding all missing definitions, open the file with the cursor in the (first) new body. |

Indentation is detected from the file being edited (4 spaces if it has none).

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
