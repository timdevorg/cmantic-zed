# cmantic-zed

Port of the C-Mantic VS Code extension to Zed, as a language server plus a thin Zed extension.

## Rules

- Never publish the Claude Code session URL in commits, the README, or anywhere else in the repo.

## Layout

- `extension.toml`, `Cargo.toml`, `src/lib.rs`: the Zed extension (crate `cmantic`, built to `wasm32-wasip2`).
  It only finds or downloads `cmantic-ls` and launches it. Lookup order: `lsp.cmantic.binary.path`
  setting, `cmantic-ls` on PATH, cached download, latest GitHub release.
- `server/`: the language server (crate `cmantic-ls`, tower-lsp-server + tree-sitter-cpp).
  All refactoring logic goes here. It is a lib + thin `main.rs` so `server/tests/` can use it.
  - `text.rs`: byte offset <-> LSP position (UTF-16), line helpers, EOL/indentation detection.
  - `config.rs`: settings from `initializationOptions` / `didChangeConfiguration` (camelCase keys).
  - `symbol.rs`: tree-sitter symbol tree (namespaces, classes, functions, other) with scopes,
    template statements, params, specifier ranges. Replaces clangd document symbols;
    declarations are matched to definitions by qualified name + normalized param types + const/ref.
    Classes carry `ClassInfo` (class/struct/union key, base classes, access specifier labels;
    `Symbol::access_at`), variable declarations carry `VariableInfo` per declarator
    (static/const/reference/pointer/array/default initializer; `Symbol::non_static_member_variables`).
  - `edit_buffer.rs`: `EditBuffer` applies successive edits to an in-memory copy and turns them
    back into non-overlapping edits against the original. Multi-step refactorings apply a change,
    re-parse `buffer.text()`, compute the next change, then send `buffer.edits()` in one applyEdit.
  - `placement.rs`: `findSmartPositionForFunctionDefinition` and `formatTextToInsert` ports.
  - `header_source.rs`: matching header/source search and cache (`HeaderSourceCache.ts`).
  - `actions/add_definition.rs`: Add Definition, pure text in -> `Insertion` out (incl. `formatDeclaration`),
    constructor initializer lists (`Initializers`), and "Add all missing definitions" (`add_definitions`,
    built on `EditBuffer`).
  - `placement.rs` also has `find_position_for_new_member_function` (`CSymbol.findPositionForNewMemberFunction`).
  - `actions/member.rs`: `ClassEditor` adds new member functions (`NewMember`: declaration, inline text,
    body, `DefinitionLocation`, optional neighbour) to a class: declaration or inline definition into the
    class (adding `public:` when needed), out-of-line definitions via `add_definition::definition_text`
    below the class or into the matching source file. Re-parses after every insertion (EditBuffer);
    `finish()` gives a `GeneratedEdit` (`edits` for the current file, `other_edits`/`other_cursor` for the
    matching header/source file).
  - `actions/add_declaration.rs`: Add Declaration (`addDeclaration.ts`): member definitions into their
    class (via `ClassEditor`, `compact`), free functions into the matching header (namespace blocks
    added when missing) or the same file (before first use). `definition_at` (cursor on a definition
    head), `find_parent_class`, `member_declaration_text`, `neighbour_declaration` are shared with
    Move Definition.
  - `add_definition::format_head` formats any function head (declaration or definition, `HeadKind`) for a
    target position given the target's enclosing scopes; explicit qualifiers (`Foo<T>::`) count as
    scopes, so qualifiers the target is inside are dropped and their template statements with them.
  - `actions/move_definition.rs`: Move Definition (`moveDefinition.ts`): to the matching source/header
    file, into the class (`combineDefinition`, keeping the definition's parameter names via
    `FunctionInfo::param_ranges`/`param_names`; or via `ClassEditor::replace` + `add` without a
    declaration) and below the class. Bodies are copied and re-indented (`dedent_lines`), deletion
    removes whole lines plus one blank line (`deletion_range`).
  - `actions/accessors.rs`: Generate Getter/Setter (`Accessor.ts`, naming from `CSymbol.ts`), built on
    `ClassEditor`. Types judged by syntax only (`VariableInfo::leading_text`).
  - `actions/operators.rs`: Generate Equality / Relational / Stream Output Operators (`Operator.ts`),
    inline only, all base classes + non-static members as operands, built on `ClassEditor`.
  - `actions/mod.rs`: `may_offer_actions_at`, the cheap gate in `code_action`.
  - `server.rs`: LSP wiring. Actions that edit another file or need a cursor move are commands:
    `cmantic.applyAction` with `{uri, position, action}` (ids in `server::action_id`); executing it
    recomputes the action (with `matching_file`: the source of a header or the header of a source
    file), sends `workspace/applyEdit`, then `window/showDocument` (one applyEdit may change the header and its source file). The older
    `cmantic.addDefinitionInSourceFile` is still accepted. Same-file single edits carry an `edit`.
  - Tests: `server/tests/add_definition.rs`, `add_definitions.rs`, `add_declaration.rs`, `move_definition.rs`, `accessors.rs`, `operators.rs` (fixtures, `@` = cursor, plus an
    every-offset no-panic test), `header_source.rs` (temp dirs), `lsp.rs` (end-to-end through
    `LspService`; `Harness` sets up a temp project and a fake client that records applyEdit/showDocument).
- `reference/vscode-cmantic/`: original C-Mantic TypeScript source (MIT). Git-ignored, read-only reference.
  `src/commands/addDefinition.ts`, `src/CSymbol.ts` and `src/SourceDocument.ts` hold the Add Definition logic;
  `src/commands/generateGetterSetter.ts`, `src/Accessor.ts`, `src/commands/generateOperators.ts` and
  `src/Operator.ts` the accessor and operator generation.

## Commands

```sh
cargo build -p cmantic-ls
cargo test -p cmantic-ls
cargo fmt --all --check
cargo clippy -p cmantic-ls --all-targets -- -D warnings
cargo build -p cmantic --target wasm32-wasip2 --release   # extension
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
