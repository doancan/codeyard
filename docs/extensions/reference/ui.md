# Views reference

<!-- Checked against crates/ext-ui by `cargo test -p ext-ui the_reference_page_matches_the_code`: every element has a
section here and every `json view` and `json event` example is accepted as written. -->

A view is a tree of 22 elements that Codeyard draws itself, with the user's theme, keyboard and screen reader. There
are no webviews and no colour or font control: you pick tones and spacing steps, and themes and high contrast work
without any effort from you. The guide is in [Views](../views.md); this page is the exact format.

The `ui` functions of both SDKs write this format ([Views](../views.md#build-it-with-the-sdk)); this page is what they
produce and what a hand-written tree must look like. The mock hosts and `cyx test` check a tree with the same code as
Codeyard (crates/ext-ui).

## How a view is drawn

1. You declare the view in the manifest (`[[views]]` with `id`, `title` and `location = "sidebar" | "bottom" | "editor"`).
2. When the user opens it, Codeyard starts your extension (reason `view:<id>`) and sends `request.render(<id>)`.
3. You answer with `response.view(json)`: the tree below, as one JSON string.
4. When your data changes, call `ui.invalidate(<id>)`. Codeyard sends `render` again, compares the new tree with the
   old one by `key` and keeps what the user typed, ticked, selected, expanded and scrolled.
5. What the user does arrives as `request.ui-event(<id>, event)`, with the event as JSON (see [Events](#events)).

Changing the colour theme or the interface scale redraws the view without calling your extension.

## The tree

Each node is a JSON object:

| Field | | |
|---|---|---|
| `element` | required | The element: `{"<kind>": {properties}}`, or just `"divider"`. |
| `key` | see below | Names the element among its siblings. Any text without control characters, at most 128 characters. |
| `children` | optional | Only on `column`, `row`, `list`, `tree`, `table` and `empty-state`. |

Interactive elements (`button`, `text-input`, `checkbox`, `select`, `list`, `tree`, `table`, `link`) and every row of
a list, table or tree need a `key`. Keys are unique among siblings. The keys from the root down to an element (skipping
elements without one) are its *path*; events name elements by their path.

Unknown elements and unknown properties are refused, so a typo is an error you see, not a silent gap. Properties
marked *optional* may be left out.

```json view
{
  "element": {"column": {"gap": 2, "pad": 2}},
  "children": [
    {"element": {"heading": {"text": "Deployments", "level": 2}}},
    {"key": "refresh", "element": {"button": {"label": "Refresh", "icon": "refresh"}}},
    {"key": "deploys", "element": {"list": {"label": "Deployments", "selection": "one"}}, "children": [
      {"key": "d1", "element": {"text": {"text": "web: ready", "tone": "success"}}},
      {"key": "d2", "element": {"text": {"text": "api: building", "tone": "warning"}}}
    ]}
  ]
}
```

### Tones and spacing

- **Tone**: `neutral` (default), `accent`, `success`, `warning`, `danger`, `muted`. The theme picks the colour.
- **Spacing step** (`gap`, `pad`): `0` (none) to `4` (widest). Codeyard maps steps to its spacing scale.
- **Selection** (lists, tables, trees): `none` (default), `one`, `many`.
- **Icons** are product icon names (`refresh`, `git-branch`, `play`). An unknown name draws a neutral placeholder.

## Elements

### `column`

Children top to bottom. Properties: `gap` (optional), `pad` (optional).

### `row`

Children left to right. Properties: `gap` (optional), `pad` (optional). A row inside a `table` is one table row: it
needs a key and one child per column.

### `text`

Properties: `text`, `tone` (optional). At most 4096 characters.

### `heading`

Properties: `text`, `level` (1 to 3).

### `icon`

Properties: `icon`, `tone` (optional), `label` (optional). Without a label the icon is decorative and screen readers
skip it.

### `image`

Properties: `source`, `alt`. `source` is `{"bytes": "<base64>"}` (PNG, JPEG, GIF, WebP or SVG, at most 1 MB) or
`{"url": "https://…"}`. An empty `alt` marks the image as decorative.

URL images are fetched by Codeyard through your `net` permission: the host must be in `net` and the folder trusted.
The response may be at most 1 MB and must be one of the formats above. Images are fetched only while the view is on
screen and kept by URL until it closes, at most 64 URLs per view. Until an image arrives, and when it is refused or
fails, the `alt` text is shown; a refusal is listed in the Inspector.

### `button`

Properties: `label`, `icon` (optional), `tone` (optional; `accent` marks the primary action), `disabled` (optional).
Sends `click`.

### `text-input`

Properties: `label`, `placeholder` (optional), `value` (optional), `multiline` (optional), `disabled` (optional).
`value` is the starting text; after that Codeyard keeps what the user types across renders. Send a different `value`
to replace it. Sends `change` when the user pauses typing, and `submit` on Enter in a single-line input.

### `checkbox`

Properties: `label`, `checked` (optional), `disabled` (optional). Sends `change`.

### `select`

Properties: `label`, `options` (a list of `{"key": …, "label": …}`, at most 256), `selected` (optional, an option's
key), `disabled` (optional). Sends `change`.

### `list`

Properties: `label`, `selection` (optional). Each child is a row with a key. At most 10,000 rows; only the rows on
screen are drawn. Sends `select` when the selection changes.

### `tree`

Properties: `label`, `description` (optional), `icon` (optional), `more` (optional page token), `selection`
(optional, only on the outermost tree). The outermost `tree` is the tree; its children are its items, which are
`tree` elements too.

Trees load one page at a time. Send the first page of a level and a `more` token; when the user expands an item that
has no children yet and has `more`, or reaches the "Load more" row of a level, Codeyard sends `expand` with that
token, and you call `ui.invalidate` once the next page is in your tree. An item with no children and no `more` is a
leaf.

```json view
{
  "key": "projects",
  "element": {"tree": {"label": "Projects", "selection": "one", "more": "page-2"}},
  "children": [
    {"key": "web", "element": {"tree": {"label": "web", "description": "3 builds", "more": ""}}},
    {"key": "docs", "element": {"tree": {"label": "docs"}}}
  ]
}
```

### `table`

Properties: `label`, `columns` (headers, 1 to 32), `selection` (optional). Each child is a `row` with a key and one
child per column. At most 10,000 rows; only the rows on screen are drawn.

```json view
{
  "key": "builds",
  "element": {"table": {"label": "Builds", "columns": ["Id", "State"]}},
  "children": [
    {"key": "b1", "element": {"row": {}}, "children": [
      {"element": {"text": {"text": "b1"}}},
      {"element": {"badge": {"text": "ok", "tone": "success"}}}
    ]}
  ]
}
```

### `badge`

Properties: `text`, `tone` (optional).

### `progress`

Properties: `label`, `percent` (optional, 0 to 100; left out when the end is unknown).

### `markdown`

Properties: `source` (CommonMark, at most 64 KB). Drawn natively. Remote images are never loaded, and a link opens
only when the user clicks it and confirms the address.

### `code`

Properties: `source` (at most 64 KB), `language` (optional).

### `key-value`

Properties: `pairs`, a list of `{"name": …, "value": …}` (at most 256).

### `banner`

Properties: `text`, `tone` (optional). A `warning` or `danger` banner is announced by screen readers when it appears.

### `link`

Properties: `label`, `url` (optional, `https:` only). With a `url`, Codeyard shows the full address and opens it in
the browser after the user agrees. Without one, you get a `link` event.

### `divider`

No properties: write `"element": "divider"`.

### `empty-state`

Properties: `title`, `text`, `icon` (optional). Its children are its actions: buttons and links only.

## Events

`request.ui-event` carries one event as JSON. `path` is the element's path (see [The tree](#the-tree)).

```json event
{"click": {"path": ["refresh"]}}
```

```json event
{"change": {"path": ["settings", "name"], "value": {"text": "web"}}}
```

`value` is `{"text": …}` for a text input, `{"checked": true}` for a checkbox and `{"choice": "<option key>"}` for a
select.

```json event
{"submit": {"path": ["search"], "text": "deploy"}}
```

```json event
{"select": {"path": ["deploys"], "rows": ["d1"]}}
```

`rows` lists every selected row (for a tree: item keys).

```json event
{"expand": {"path": ["projects"], "item": "web", "page": ""}}
```

`item` is the item whose next page is wanted, or `null` for the top level; `page` is that level's `more` token.

```json event
{"link": {"path": ["help"]}}
```

## Limits

A view is refused, and the reason shown in place of the view and in your log, when it breaks a rule: at most 8 MB of
JSON, 50,000 elements, 32 levels deep, 10,000 rows per list, table or tree page, labels of 256 characters and body text
of 4096 characters.

## Keyboard and screen readers

You write no accessibility code. Every element is announced with a role and label taken from its kind and text: a
`list` row as a list item labelled with its text, a `table` row as a row of cells, a `tree` item as a tree item, a
`warning` or `danger` banner as an alert. Tab moves between interactive elements. In a list, table or tree the arrow
keys move between rows, Space selects, Right and Left expand and collapse a tree item, and Enter presses a button or
opens a link.

## What extensions cannot draw over

A view stays inside its place: the sidebar, a panel tab or an editor tab, under a header that names the extension's
publisher. Install consent, workspace trust and sign-in sheets are drawn by Codeyard in a layer views cannot reach, and
an extension cannot open, cover or replace them.
