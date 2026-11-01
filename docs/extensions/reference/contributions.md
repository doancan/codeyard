# Contributions reference

<!-- Generated from crates/ext-manifest/src/raw.rs: run `UPDATE_DOCS=1 cargo test -p ext-manifest reference` after changing it. -->

What an extension adds to Codeyard. Each kind is an array of tables (`[[commands]]`) except `settings`. Ids are local: `refresh` in `you.deploy` becomes `you.deploy.refresh`. Contributions that run code need a `[code]` table ([Manifest](manifest.md#code)); the others work without any. The guides are the [developer guide](../developer-guide.md#contributions-that-run-your-code), [Commands](../commands.md) and [Views](../views.md).

- [`[[themes]]`](#themes): A colour theme: a Codeyard or VS Code colour theme JSON file.
- [`[[snippets]]`](#snippets): Snippets for one language, in the VS Code snippet format.
- [`[[keymap-preset]]`](#keymap-preset): A whole keymap the user can pick with `"keymap.preset": "<extension id>/<label>"`.
- [`[[keybindings]]`](#keybindings): A key binding for one of your commands, a dependency's command or a `core:` command. A key a built-in shortcut uses is left out.
- [`[[languages]]`](#languages): A language for files no built-in language claims.
- [`[[json-schemas]]`](#json-schemas): A JSON schema for the JSON files that match.
- [`[[yaml-schemas]]`](#yaml-schemas): A JSON schema for the YAML files that match.
- [`[settings.<key>]`](#settings): A setting, as `[settings.<key>]`. The user sets it as `ext.<extension id>.<key>` in settings.jsonc; code reads it by its key.
- [`[[language-servers]]`](#language-servers): A language server: a program that speaks the Language Server Protocol. It runs where the files are, so it works over SSH.
- [`[[commands]]`](#commands): A command in the palette. Needs `[code]`; the code registers its handler with the same id.
- [`[[views]]`](#views): A view drawn from built-in elements (reference/ui.md). Needs `[code]`.
- [`[[oauth]]`](#oauth): An OAuth 2 provider the extension signs in to, named in `[permissions] auth`.
- [`[[menus]]`](#menus): An entry in a context menu or a view's title bar that runs a command.
- [`[[status-bar]]`](#status-bar): A status bar item; the code sets its text with this id.
- [`[[formatters]]`](#formatters): Formats documents of these languages (`request.format`).
- [`[[code-actions]]`](#code-actions): Code actions for these languages, shown in Quick Fix (`request.code-actions`).
- [`[[diagnostics]]`](#diagnostics): A set of diagnostics the code pushes for files of these languages.
- [`[[decorations]]`](#decorations): A set of decorations (gutter, line, after the text) the code pushes for files of these languages.
- [`[[tasks]]`](#tasks): Tasks the code lists and runs (`request.provide-tasks`, `request.run-task`).
- [`[[hover]]`](#hover): Hover text for these languages (`request.hover`).
- [`[[completion]]`](#completion): Completion items for these languages (`request.completion`).
- [`[[code-lens]]`](#code-lens): Code lenses for the lines in view (`request.code-lens`).
- [`[[inlay-hints]]`](#inlay-hints): Inlay hints for the lines in view (`request.inlay-hints`).
- [`[[scm]]`](#scm): A source control provider for workspace folders that contain one of its markers.
- [`[[pull-requests]]`](#pull-requests): A pull (merge) request provider for the repository remotes on its hosts.

## themes

`[[themes]]`: A colour theme: a Codeyard or VS Code colour theme JSON file.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `label` | string | yes | The name the user picks it by. |
| `path` | string | yes | The file in the package. |

## snippets

`[[snippets]]`: Snippets for one language, in the VS Code snippet format.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `language` | string | yes | The language id the snippets apply to. |
| `path` | string | yes | The snippet file in the package. |

## keymap-preset

`[[keymap-preset]]`: A whole keymap the user can pick with `"keymap.preset": "<extension id>/<label>"`.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `label` | string | yes | The name the user picks it by. |
| `path` | string | yes | The file in the package. |

## keybindings

`[[keybindings]]`: A key binding for one of your commands, a dependency's command or a `core:` command. A key a built-in shortcut uses is left out.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `key` | string | yes | Key strokes such as `"cmd-shift-r"` or `"ctrl-k ctrl-r"`. |
| `command` | string | yes | The local id of one of your commands, a dependency's full id, or a `core:` command. |
| `when` | string | no | A `when` expression; the binding applies only while it holds. |

## languages

`[[languages]]`: A language for files no built-in language claims.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The language id, lowercase `a-z 0-9 -`. |
| `name` | string | no | The display name. |
| `extensions` | array of strings | no | File extensions without the dot (`"toml"`). A language needs these or `filenames`. |
| `filenames` | array of strings | no | Whole file names (`"Justfile"`). |
| `grammar` | string | no | A built-in grammar to highlight with, by its language id (`rust`, `toml`, …). |
| `line-comment` | string | no | The line comment token (`"#"`), for Toggle Comment. |
| `block-comment` | array of two strings | no | The start and end of a block comment (`["/*", "*/"]`). |

## json-schemas

`[[json-schemas]]`: A JSON schema for the JSON files that match.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `file-match` | array of strings | yes | Workspace globs of the files the schema applies to. |
| `path` | string | yes | The schema file in the package. |

## yaml-schemas

`[[yaml-schemas]]`: A JSON schema for the YAML files that match.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `file-match` | array of strings | yes | Workspace globs of the files the schema applies to. |
| `path` | string | yes | The schema file in the package. |

## settings

`[settings.<key>]`: A setting, as `[settings.<key>]`. The user sets it as `ext.<extension id>.<key>` in settings.jsonc; code reads it by its key.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `type` | string | yes | `string`, `number`, `boolean`, `array` or `object`. |
| `default` | any value | no | The value when the user has not set one; it must match `type`. |
| `description` | string | yes | What the setting does, shown in Settings. |
| `enum` | array of strings | no | The allowed values of a string setting. |

## language-servers

`[[language-servers]]`: A language server: a program that speaks the Language Server Protocol. It runs where the files are, so it works over SSH.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `command` | string | yes | The program, also listed in `[permissions] run`: a name on `PATH` or one inside the package. |
| `args` | array of strings | no | Arguments passed to the program. |
| `languages` | array of strings | yes | Language ids it serves. |

## commands

`[[commands]]`: A command in the palette. Needs `[code]`; the code registers its handler with the same id.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The local id: `refresh` becomes `<extension id>.refresh`. |
| `title` | string | yes | The palette title, such as `"Deploy: Refresh"`. |
| `category` | string | no | Groups the command in the palette. |
| `enablement` | string | no | A `when` expression; the command is greyed out while it does not hold. |

## views

`[[views]]`: A view drawn from built-in elements (reference/ui.md). Needs `[code]`.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The local id. |
| `title` | string | yes | The view's title. |
| `location` | string | yes | `sidebar`, `bottom` or `editor`. |
| `when` | string | no | A `when` expression; the view is offered only while it holds. |

## oauth

`[[oauth]]`: An OAuth 2 provider the extension signs in to, named in `[permissions] auth`.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The account id, named in `[permissions] auth`. |
| `name` | string | yes | The provider's name, shown to the user. |
| `authorize-url` | string | yes | The `https://` authorization endpoint. |
| `token-url` | string | yes | The `https://` token endpoint. |
| `client-id` | string | yes | The public client id. There is no client secret: PKCE replaces it. |
| `scopes` | array of strings | no | The scopes to ask for; `auth.session` may ask for fewer. |
| `hosts` | array of strings | yes | The hosts `auth.fetch` signs requests to; any other host is refused. |
| `account-url` | string | no | An `https://` URL that answers JSON with the account's name, shown on the extension's page. |
| `instances` | boolean | no | The user may add self-managed instances of this provider (#184). |

## menus

`[[menus]]`: An entry in a context menu or a view's title bar that runs a command.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `command` | string | yes | One of your commands, a dependency's full id, or a `core:` command that takes no argument. |
| `location` | string | yes | `editor-context`, `file-tree` or `view-title`. |
| `view` | string | no | For `view-title`: the view (local id). |
| `when` | string | no | A `when` expression; the entry is hidden while it does not hold. |

## status-bar

`[[status-bar]]`: A status bar item; the code sets its text with this id.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The local id the code sets the item's text with. An undeclared id is not shown. |
| `command` | string | no | The command a click runs, unless the item names another at run time. |

## formatters

`[[formatters]]`: Formats documents of these languages (`request.format`).

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `languages` | array of strings | yes | Language ids it serves. |

## code-actions

`[[code-actions]]`: Code actions for these languages, shown in Quick Fix (`request.code-actions`).

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `languages` | array of strings | yes | Language ids it serves. |

## diagnostics

`[[diagnostics]]`: A set of diagnostics the code pushes for files of these languages.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The local id the code pushes them under. |
| `languages` | array of strings | yes | Language ids it serves. |

## decorations

`[[decorations]]`: A set of decorations (gutter, line, after the text) the code pushes for files of these languages.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The local id the code pushes them under. |
| `languages` | array of strings | yes | Language ids it serves. |

## tasks

`[[tasks]]`: Tasks the code lists and runs (`request.provide-tasks`, `request.run-task`).

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The local id. |

## hover

`[[hover]]`: Hover text for these languages (`request.hover`).

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `languages` | array of strings | yes | Language ids it serves. |

## completion

`[[completion]]`: Completion items for these languages (`request.completion`).

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `languages` | array of strings | yes | Language ids it serves. |
| `trigger-characters` | array of strings | no | Punctuation characters that ask too, besides typing a word (`["."]`). |

## code-lens

`[[code-lens]]`: Code lenses for the lines in view (`request.code-lens`).

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `languages` | array of strings | yes | Language ids it serves. |

## inlay-hints

`[[inlay-hints]]`: Inlay hints for the lines in view (`request.inlay-hints`).

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `languages` | array of strings | yes | Language ids it serves. |

## scm

`[[scm]]`: A source control provider for workspace folders that contain one of its markers.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The local id; the code answers for it. |
| `label` | string | yes | The name shown in Source Control. |
| `markers` | array of strings | yes | Files or folders at the top of a workspace folder that it claims, such as `".hg"`. |

## pull-requests

`[[pull-requests]]`: A pull (merge) request provider for the repository remotes on its hosts.

| Field | Type | Required | What it does |
| --- | --- | --- | --- |
| `id` | string | yes | The local id; the code answers for it. |
| `label` | string | yes | Shown next to each request. |
| `hosts` | array of strings | yes | The hosts of the remotes it claims; `*.example.com` works as in `net`. |
| `auth` | string | no | An account from `[permissions] auth`, passed to the code as `remote.account`. |

## Key names

A `key` is one to four keystrokes separated by spaces (`ctrl-k ctrl-r`), each modifiers and one key joined by `-`. Modifiers: `cmd`, `ctrl`, `alt`, `shift`, `fn`, `super`, `win`, `secondary`. `secondary` is `cmd` on macOS and `ctrl` elsewhere. A key is one character (`a`, `1`, `/`; `ctrl--` is the minus key), `f1` to `f35`, or one of `enter`, `escape`, `tab`, `space`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`, `right`, `menu`. A key a built-in shortcut uses is not bound; `cyx check` warns about it.

## Language ids

The languages Codeyard gives files itself, for `language` and `languages` fields and `editor.language`: `c`, `cpp`, `csharp`, `css`, `go`, `html`, `java`, `javascript`, `javascriptreact`, `json`, `jsonc`, `kotlin`, `less`, `lua`, `markdown`, `php`, `plaintext`, `python`, `ruby`, `rust`, `scss`, `shellscript`, `sql`, `swift`, `toml`, `typescript`, `typescriptreact`, `yaml`. An extension's `[[languages]]` adds its own ids. `cyx check` warns about an id that is neither one of these nor one the extension adds (a dependency may add it).
