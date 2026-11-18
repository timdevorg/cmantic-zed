# C-Mantic for Zed

C/C++ refactorings for [Zed](https://zed.dev), ported from the
[C-Mantic](https://github.com/BigBahss/vscode-cmantic) VS Code extension.

> **Status: early development.** Add Definition, Add Declaration, Move Definition,
> Generate Getters and Setters and Generate Operators are implemented; the other refactorings are still being ported.

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
| Add Declaration (in class / matching header / this file) | done (always public) |
| Generate Getters and Setters | done |
| Generate equality, relational and stream output operators | done (all operands, inline) |
| Add Header Guard, Add Include, Create Matching Source File | planned |
| Move Definition (matching source/header file, into/out of class) | done |
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

## Add Declaration

Put the cursor on the head of a function definition (its name, return type, parameters, ...
but not its body) that has no declaration:

- **Add Declaration in class "Foo"** (or `struct "Foo"`): for an out-of-class member
  definition (`void Foo::bar() {}`) whose class is in this file or the matching header. The
  declaration goes next to the declaration of a neighbouring definition when that one is
  public, else after the last public member; a `public:` section is added if needed.
- **Add Declaration in matching header file**: for a free function in a source file whose
  matching header exists. It goes next to the declaration of a neighbouring definition,
  else at the end of the matching `namespace` block, else after the last symbol. Namespaces
  missing from the header are added around the declaration (`namespace b {` ... `}`).
- **Add Declaration in this file**: for free functions, as a forward declaration next to the
  declarations of neighbouring definitions, else before the first top-level symbol that uses
  the function, else right before the definition.

The declaration is the definition's head without the body: scope qualifiers the position is
already inside are dropped (`Foo::`), as are default arguments and `inline`; `virtual`,
`static` and the like can't be known and are not added. Template statements are kept
(except those of the enclosing class template when declaring inside the class).

Not offered for definitions inside a class body, when a declaration exists (in the class, in
the matching header, or for "this file" in the file itself), for `main`, or when the class of
a member definition can't be found.

Differences from C-Mantic:

- C-Mantic asks for the access specifier of a new member; Zed has no picker, so it is always
  `public` (move it if needed). C-Mantic always uses the end of the public section; this port
  prefers the neighbour's declaration.
- `static` functions and functions in an anonymous namespace are only offered "in this file",
  and their forward declaration keeps `static` (C-Mantic removes it, which doesn't compile).
- "In this file" is placed before the first use. C-Mantic places it after the last symbol,
  where it doesn't help.
- Offered on the whole head of the definition, not only its name.

## Move Definition

Put the cursor on the head of a function definition (not its body):

- **Move Definition to matching source file** (in a header): moves the definition to the
  source file, next to the definitions of neighbouring declarations like Add Definition.
  An in-class definition, or a free function without a separate declaration, leaves its
  declaration behind (the head as written, with default arguments). `inline` is dropped.
- **Move Definition to matching header file** (in a source file): moves the definition to the
  header next to its declaration's neighbours, adding `inline`. Offered when the declaration
  is in the header, or for a free function without one.
- **Move Definition into class "Foo"** (or `struct`): an out-of-class member definition replaces
  its declaration in the class (in this file or the matching header): the declaration's head
  (keeping `virtual`, default arguments, ...) with the definition's body. Without a
  declaration, the whole definition goes into the public section.
- **Move Definition below class body** (or `struct`): an in-class definition leaves its
  declaration in the class and goes below the class with `Foo::` and, in a header, `inline`.

The body is kept as it is, re-indented for its new place; scope qualifiers are added or
dropped as needed (`namespace` blocks and qualified names both work), template statements of
class templates are added when moving out of the class. Leading comments move with the
definition. When a declaration is left behind, `alwaysMoveComments: false` keeps them with
the declaration instead. Moving to another file opens it at the moved definition
(`revealNewDefinition`).

Not offered to the source file for `inline` functions (unless only the definition says
`inline`), `constexpr`, `consteval`, templates and members of class templates, `static` free
functions and functions in an anonymous namespace; not offered to the header for a definition
whose declaration is in the source file itself. Into/out of class only in C++, and not for
anonymous classes or friends defined in class templates.

Differences from C-Mantic:

- Moving into the class keeps the definition's parameter names when the declaration's differ
  (or are missing), since the body uses them. Both leading comments are kept when the
  declaration and the definition have one (C-Mantic drops the definition's).
- When a deleted definition's comment would not be moved (`alwaysMoveComments: false` but no
  declaration left behind), it is moved anyway instead of being lost.
- For a member without a declaration, C-Mantic asks for the access specifier; this port uses
  `public` (next to a neighbour's declaration when possible).
- Titles say "matching source/header file" instead of the file name. Moving a definition of a
  class declared in a source file to the header is not offered.

## Generate Getters and Setters

Put the cursor on a member variable of a class or struct (in C++ files) to get:

- **Generate Getter and Setter for "x"**
- **Generate Getter for "x"**
- **Generate Setter for "x"**

Only what doesn't exist yet is offered (a class member with the getter's or setter's name
counts as existing), and there is no setter for `const`, `constexpr` and reference members.
Static members get static accessors. With the cursor on the shared type of `int a, b;` the
first declarator is used; put it on `b` for `b`.

```cpp
class Foo {
public:
    int count() const { return m_count; }
    void setCount(int count) { m_count = count; }

private:
    int m_count;
};
```

Naming follows C-Mantic: `m_`, `s_` and leading/trailing `_` are stripped (`m_count` ->
`count()` / `setCount`), a name without such a prefix gets `get` (`value` -> `getValue()`),
and `caseStyle` picks camelCase, snake_case or PascalCase. With `cpp.accessor.boolGetterIsPrefix`
`bool` members get `isX()`. The setter parameter is the base name, or the name with a trailing
`_` when that would be the member's own name.

The new functions go after the last public member (next to an existing getter or setter if
there is one). If the class has no public members, a `public:` section is added after the last
member, indented like the class's other access labels. `cpp.accessor.getterDefinitionLocation`
and `setterDefinitionLocation` choose between an inline definition (default), a declaration
plus a definition below the class, or a definition in the matching source file (below the
class if there is no matching source file, or for class templates).

Differences from C-Mantic:

- Types are judged by syntax only (there is no clangd here; C-Mantic does the same with its
  default `resolveTypes: false`). The setter takes pointers and types spelled with a primitive
  keyword (`int`, `unsigned long`, `bool`, ...) by value and everything else by `const T &`,
  including enums and typedefs of primitives like `std::size_t`.
- The getter returns by value, like C-Mantic, but only top-level `const`/`volatile` are
  dropped from the return type, so `const char*` and `const T&` members give valid getters.
- No accessors for arrays, function pointers, members of anonymous structs/unions, or member
  templates (C-Mantic would produce code that doesn't compile).
- Also offered in source files, not only in headers.

## Generate Operators

On a class or struct head (its name, base list, ... but not its body):

- **Generate Equality Operators for "Foo"**: `operator==` and `operator!=`.
- **Generate Relational Operators for "Foo"**: `operator<`, `operator>`, `operator<=` and
  `operator>=` (the last three in terms of `<`).
- **Generate Stream Output Operator for "Foo"**: a `friend` `operator<<` for `std::ostream`.

```cpp
class Foo : public Base {
public:
    bool operator==(const Foo &other) const {
        return static_cast<const Base &>(*this) == static_cast<const Base &>(other)
            && a == other.a
            && b == other.b;
    }
    bool operator!=(const Foo &other) const { return !(*this == other); }

private:
    int a;
    std::string b;
};
```

Operators the class already declares (as members or friends) are skipped, and the missing
ones of a group are placed next to the existing ones; nothing is offered when all exist.
`cpp.friendComparisonOperators` generates `friend bool operator==(const Foo &lhs, const Foo
&rhs)` instead of member functions, and `cpp.useExplicitThisPointer` writes `this->a`.

Differences from C-Mantic:

- C-Mantic asks which base classes and members to use and where to define each operator.
  Zed has no pickers, so all base classes and non-static data members are used, in
  declaration order, and the operators are defined inline in the public section (a
  `public:` section is added if needed). Edit the result to taste.
- `operator<<` needs `<ostream>` (or `<iostream>`). C-Mantic adds `#include <ostream>`; this
  port doesn't touch includes, so add it yourself if it's missing.
- Base classes are cast with their full name (`static_cast<const ns::Base<int> &>`).
- With no operands, `==` returns `true` and `<` returns `false` (C-Mantic leaves the body empty).
- Not offered for unions or anonymous classes; operators declared outside the class (free
  functions) aren't detected.

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
          "bracedInitialization": false,
          "accessor": {
            "getterDefinitionLocation": "inline",
            "setterDefinitionLocation": "inline",
            "boolGetterIsPrefix": false
          },
          "useExplicitThisPointer": false,
          "friendComparisonOperators": false
        },
        "c": { "curlyBraceFormat": { "function": "newLine" } },
        "caseStyle": "camelCase",
        "revealNewDefinition": true,
        "alwaysMoveComments": true
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
| `cpp.accessor.getterDefinitionLocation`, `cpp.accessor.setterDefinitionLocation` | Where new getters/setters are defined: `"inline"` (in the class), `"belowClass"` (declaration in the class, definition below it), or `"matchingSourceFile"` (falls back to `"belowClass"` without a matching source file or for class templates). C-Mantic's values (`"Generate definition inline"`, ...) are accepted too. |
| `cpp.accessor.boolGetterIsPrefix` | Name getters of `bool` members `isX` instead of `getX`. |
| `cpp.useExplicitThisPointer` | Write `this->member` in generated member functions (getters, setters, operators). |
| `cpp.friendComparisonOperators` | Generate comparison operators as `friend` functions taking `lhs` and `rhs` instead of member functions. |
| `caseStyle` | Naming of generated getters and setters: `"camelCase"`, `"snake_case"` or `"PascalCase"`. |
| `revealNewDefinition` | After adding a definition to the matching source file, or adding all missing definitions, open the file with the cursor in the (first) new body. Also used for getters/setters defined in the matching source file, and to open the file a definition was moved to. |
| `alwaysMoveComments` | Move Definition moves leading comments with the definition even when a declaration is left behind. When `false`, they stay with the declaration. |

Indentation is detected from the file being edited, from the lines that follow a `{`, an access
specifier or a case label (so aligned initializer lists and parameter lists are ignored). If the
file has none, the other file of the refactoring or 4 spaces is used.

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
