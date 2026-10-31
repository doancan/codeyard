# Views

A view is a panel your extension fills: in the sidebar, as a tab of the bottom panel, or as a page in the editor area.
You describe it as a tree of 22 built-in elements and Codeyard draws it, with the user's theme, fonts, keyboard and
screen reader. There are no webviews, so you cannot run HTML or scripts in a view; in exchange a view looks and behaves
like the rest of Codeyard and stays fast with thousands of rows.

The exact format of every element and event is in the [views reference](reference/ui.md).

## Declare it

```toml
[[views]]
id       = "deployments"
title    = "Deployments"
location = "sidebar"          # sidebar | bottom | editor
```

- `sidebar` adds a button to the activity bar.
- `bottom` adds a tab to the bottom panel, next to Problems and Terminal.
- `editor` opens as a page tab in the editor area.

Every view is also in the command palette as **Open View: Deployments (you)**. A `[[menus]]` entry with
`location = "view-title"` and `view = "deployments"` puts your command under the **…** button in the view's title bar;
it runs with the view's full id (`"you.ext.deployments"`) as its argument, and `when` and `enablement` apply as in
other menus. Opening a view starts your extension
(the activation reason is `view:deployments`), so a view costs nothing until someone looks at it.

### Only when it fits

```toml
[[views]]
id       = "crates"
title    = "Crates"
location = "sidebar"
when     = "editor.language == 'rust'"
```

With `when`, the view is offered (activity bar button, panel tab, **Open View**) only while the condition holds, and an
open view closes when it turns false. The keys describe what the window shows:

| Key | True when |
| --- | --- |
| `editor` | a file is open in an editor tab |
| `editor.language` | the language id of that file (`rust`, `toml`, …) |
| `terminal` | the bottom panel shows the terminal |
| `file-tree`, `scm`, `search` | that view is in a side bar |
| `panel` | the bottom panel is open |
| `workspace.trusted` | every folder of the window is trusted |

`ext.<key>` is always false in this version. A sidebar view replaces the view that was in the side bar, so
`when = "file-tree"` on a sidebar view closes it as soon as it opens.

## Draw it

When the view opens, your extension gets `render("deployments")` and answers with `response.view`: the tree as JSON.

```json
{
  "element": {"column": {"gap": 2}},
  "children": [
    {"element": {"heading": {"text": "Deployments", "level": 2}}},
    {"key": "refresh", "element": {"button": {"label": "Refresh", "icon": "refresh-cw"}}},
    {"key": "list", "element": {"list": {"label": "Deployments", "selection": "one"}}, "children": [
      {"key": "web", "element": {"text": {"text": "web: ready", "tone": "success"}}},
      {"key": "api", "element": {"text": {"text": "api: building", "tone": "warning"}}}
    ]}
  ]
}
```

You pick tones (`neutral`, `accent`, `success`, `warning`, `danger`, `muted`) and spacing steps (0 to 4), never colours
or sizes. When the user changes the colour theme, turns on high contrast or zooms the interface, Codeyard draws your
view again by itself; your extension is not called.

### Build it with the SDK

Both SDKs have `ui` functions that write exactly this form, one per element. What an element needs comes first (an
interactive element's key, then its label), the optional properties after; containers take their children.

```rust
use codeyard_extension::prelude::*;

ctx.view("deployments", async |_ctx| {
    Ok(ui::column()
        .gap(2)
        .child(ui::heading("Deployments", 2))
        .child(ui::button("refresh", "Refresh").icon("refresh-cw"))
        .child(ui::list("list", "Deployments").selection(ui::Selection::One).children([
            ui::text("web: ready").tone(Tone::Success).key("web"),
            ui::text("api: building").tone(Tone::Warning).key("api"),
        ])))
});
```

```ts
import { ui } from "@codeyard/extension";

ctx.view("deployments", () =>
  ui.column(
    [
      ui.heading("Deployments", 2),
      ui.button("refresh", "Refresh", { icon: "refresh-cw" }),
      ui.list("list", "Deployments", [ui.text("web: ready", { tone: "success", key: "web" }), ui.text("api: building", { tone: "warning", key: "api" })], {
        selection: "one",
      }),
    ],
    { gap: 2 },
  ),
);
```

Both draw the tree above. You may also return the JSON yourself, in the form of the reference: in TypeScript the
`ViewNode` type checks it element by element. Codeyard refuses anything else, such as a `{ "type": "column", … }`
shorthand, and shows the reason in place of the view. The mock hosts' `render` and `cyx test` check a tree with the
same rules ([Testing](testing.md)), so a tree that passes there is drawn.

## Keep it current

When your data changes, call `ui.invalidate("deployments")`. Codeyard asks for the tree again and compares it with the
one on screen by `key`. Give every interactive element and every row a key that stays the same across renders: then
what the user typed into an input, ticked, chose, selected, expanded and how far they scrolled stays as it was. Send a
different `value`, `checked` or `selected` only when you mean to replace what the user did.

Several `invalidate` calls in a row cost one render.

## Hear the user

What the user does arrives as `ui-event("deployments", event)` with the event as JSON, for example
`{"click": {"path": ["refresh"]}}`. The path is the keys from the top of the tree down to the element. Buttons send
`click`, inputs, checkboxes and selects send `change` (a text input when the user pauses typing), Enter in a
single-line input sends `submit`, lists, tables and trees send `select`, a link without a URL sends `link`, and a tree
asks for its next page with `expand`.

A `ui-event` counts as a user action, like a command: your extension may open a link in the browser in answer.

## Long lists and trees

A `list` or `table` takes up to 10,000 rows, and Codeyard draws only those on screen, so scrolling stays smooth. A
`tree` loads one page at a time: send the first page with a `more` token, and when the user opens an item that has no
children yet (or reaches "Load more"), you get `expand` with that token. Add the next page to your tree and call
`ui.invalidate`.

## What views cannot do

- Draw outside their place, over other parts of Codeyard, or over the install, trust and sign-in sheets. Each view
  names your publisher id at the top, so it cannot pass for Codeyard or for another extension.
- Take the keyboard while one of Codeyard's sheets or pickers waits for the user.
- Load remote images in `markdown`. Links in Markdown and `link` elements open only after the user sees and confirms
  the full address.
- Load an `image` from a host that is not in your `net` permission, or from any host in a folder the user has not
  trusted. Such an image shows its `alt` text, and the refusal is listed under **Recent denials** in the Inspector.

## Images from the web

An `image` with `{"url": "https://…"}` is fetched by Codeyard, not by your extension, through your `net` permission:

```toml
[permissions]
net = ["avatars.example.com"]
```

The rules are those of `net.fetch`: https only, the host must be in `net`, the address it resolves to must be public,
and redirects are checked again. The image may be at most 1 MB and must be PNG, JPEG, GIF, WebP or SVG. Codeyard
fetches only while the view is on screen, two images at a time, and keeps each image by URL until the view closes, so
drawing the view again does not fetch it again. A view fetches at most 64 different URLs; later ones show their `alt`
text. A failed image shows its `alt` text and a line in your log.

## Accessibility

You write no accessibility code. Every element is announced with a role and a label from its kind and text, Tab moves
between interactive elements, and the arrow keys move through lists, tables and trees (Right and Left open and close
tree items, Space selects). Give images an `alt`, icons that carry meaning a `label`, and inputs a `label`, and the view
works with a screen reader.
