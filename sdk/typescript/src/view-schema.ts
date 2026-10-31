// Generated from crates/ext-ui (schema.rs) by `cargo test -p ext-ui`; do not edit. After a change to the view format,
// `CODEYARD_UPDATE_VIEW_SCHEMA=1 cargo test -p ext-ui` rewrites it. The format is docs/extensions/reference/ui.md.

import type { Tone } from "./wit.ts";

/** A spacing step, from 0 (none) to 4 (widest). */
export type Space = 0 | 1 | 2 | 3 | 4;

/** How many rows of a list, table or tree the user can select. */
export type Selection = "none" | "one" | "many";

/** The properties of each element (docs/extensions/reference/ui.md). */
export interface ElementProps {
  column: { gap?: Space; pad?: Space };
  row: { gap?: Space; pad?: Space };
  text: { text: string; tone?: Tone };
  heading: { text: string; level: 1 | 2 | 3 };
  icon: { icon: string; tone?: Tone; label?: string | null };
  image: { alt: string; source: { bytes: string } | { url: string } };
  button: { label: string; icon?: string | null; tone?: Tone; disabled?: boolean };
  "text-input": { label: string; placeholder?: string; value?: string; multiline?: boolean; disabled?: boolean };
  checkbox: { label: string; checked?: boolean; disabled?: boolean };
  select: { label: string; options: { key: string; label: string }[]; selected?: string | null; disabled?: boolean };
  list: { label: string; selection?: Selection };
  tree: { label: string; description?: string | null; icon?: string | null; more?: string | null; selection?: Selection };
  table: { label: string; columns: string[]; selection?: Selection };
  badge: { text: string; tone?: Tone };
  progress: { label: string; percent?: number | null };
  markdown: { source: string };
  code: { source: string; language?: string | null };
  "key-value": { pairs: { name: string; value: string }[] };
  banner: { text: string; tone?: Tone };
  link: { label: string; url?: string | null };
  divider: Record<string, never>;
  "empty-state": { title: string; text: string; icon?: string | null };
}

/** The kinds of element. */
export type Kind = keyof ElementProps;

/** An element: `{ <kind>: { properties } }`, or `"divider"`. */
export type Element = "divider" | { [K in Exclude<Kind, "divider">]: { [P in K]: ElementProps[K] } }[Exclude<Kind, "divider">];

/**
 * One node of a view's tree, as Codeyard reads it from `render`: the element, its key (needed on interactive elements
 * and rows, unique among siblings) and its children (on column, row, list, tree, table and empty-state).
 */
export interface ViewNode {
  key?: string;
  element: Element;
  children?: ViewNode[];
}

/** The format as data, for the mock host's check (testing/view-check.ts). */
export interface ViewSchema {
  elements: { kind: Kind; children: boolean; rows: boolean; interactive: boolean; properties: { name: string; type: string; required: boolean; nullable: boolean; what: string }[] }[];
  tones: string[];
  selections: string[];
  events: Record<string, string[]>;
  limits: Record<string, number>;
}

export const VIEW_SCHEMA: ViewSchema = {
  "elements": [
    {
      "children": true,
      "interactive": false,
      "kind": "column",
      "properties": [
        {
          "name": "gap",
          "nullable": false,
          "required": false,
          "type": "space",
          "what": "gap"
        },
        {
          "name": "pad",
          "nullable": false,
          "required": false,
          "type": "space",
          "what": "pad"
        }
      ],
      "rows": false
    },
    {
      "children": true,
      "interactive": false,
      "kind": "row",
      "properties": [
        {
          "name": "gap",
          "nullable": false,
          "required": false,
          "type": "space",
          "what": "gap"
        },
        {
          "name": "pad",
          "nullable": false,
          "required": false,
          "type": "space",
          "what": "pad"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "text",
      "properties": [
        {
          "name": "text",
          "nullable": false,
          "required": true,
          "type": "body",
          "what": "the text"
        },
        {
          "name": "tone",
          "nullable": false,
          "required": false,
          "type": "tone",
          "what": "the tone"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "heading",
      "properties": [
        {
          "name": "text",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the text"
        },
        {
          "name": "level",
          "nullable": false,
          "required": true,
          "type": "level",
          "what": "the level"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "icon",
      "properties": [
        {
          "name": "icon",
          "nullable": false,
          "required": true,
          "type": "icon",
          "what": "the icon name"
        },
        {
          "name": "tone",
          "nullable": false,
          "required": false,
          "type": "tone",
          "what": "the tone"
        },
        {
          "name": "label",
          "nullable": true,
          "required": false,
          "type": "label",
          "what": "the label"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "image",
      "properties": [
        {
          "name": "alt",
          "nullable": false,
          "required": true,
          "type": "alt",
          "what": "the alt text"
        },
        {
          "name": "source",
          "nullable": false,
          "required": true,
          "type": "image",
          "what": "the image"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": true,
      "kind": "button",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "icon",
          "nullable": true,
          "required": false,
          "type": "icon",
          "what": "the icon name"
        },
        {
          "name": "tone",
          "nullable": false,
          "required": false,
          "type": "tone",
          "what": "the tone"
        },
        {
          "name": "disabled",
          "nullable": false,
          "required": false,
          "type": "bool",
          "what": "disabled"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": true,
      "kind": "text-input",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "placeholder",
          "nullable": false,
          "required": false,
          "type": "body",
          "what": "the placeholder"
        },
        {
          "name": "value",
          "nullable": false,
          "required": false,
          "type": "body",
          "what": "the value"
        },
        {
          "name": "multiline",
          "nullable": false,
          "required": false,
          "type": "bool",
          "what": "multiline"
        },
        {
          "name": "disabled",
          "nullable": false,
          "required": false,
          "type": "bool",
          "what": "disabled"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": true,
      "kind": "checkbox",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "checked",
          "nullable": false,
          "required": false,
          "type": "bool",
          "what": "checked"
        },
        {
          "name": "disabled",
          "nullable": false,
          "required": false,
          "type": "bool",
          "what": "disabled"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": true,
      "kind": "select",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "options",
          "nullable": false,
          "required": true,
          "type": "choices",
          "what": "the options"
        },
        {
          "name": "selected",
          "nullable": true,
          "required": false,
          "type": "option-key",
          "what": "the selected option"
        },
        {
          "name": "disabled",
          "nullable": false,
          "required": false,
          "type": "bool",
          "what": "disabled"
        }
      ],
      "rows": false
    },
    {
      "children": true,
      "interactive": true,
      "kind": "list",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "selection",
          "nullable": false,
          "required": false,
          "type": "selection",
          "what": "the selection"
        }
      ],
      "rows": true
    },
    {
      "children": true,
      "interactive": true,
      "kind": "tree",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "description",
          "nullable": true,
          "required": false,
          "type": "label",
          "what": "the description"
        },
        {
          "name": "icon",
          "nullable": true,
          "required": false,
          "type": "icon",
          "what": "the icon name"
        },
        {
          "name": "more",
          "nullable": true,
          "required": false,
          "type": "page",
          "what": "the page token"
        },
        {
          "name": "selection",
          "nullable": false,
          "required": false,
          "type": "selection",
          "what": "the selection"
        }
      ],
      "rows": true
    },
    {
      "children": true,
      "interactive": true,
      "kind": "table",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "columns",
          "nullable": false,
          "required": true,
          "type": "columns",
          "what": "a column header"
        },
        {
          "name": "selection",
          "nullable": false,
          "required": false,
          "type": "selection",
          "what": "the selection"
        }
      ],
      "rows": true
    },
    {
      "children": false,
      "interactive": false,
      "kind": "badge",
      "properties": [
        {
          "name": "text",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the text"
        },
        {
          "name": "tone",
          "nullable": false,
          "required": false,
          "type": "tone",
          "what": "the tone"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "progress",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "percent",
          "nullable": true,
          "required": false,
          "type": "percent",
          "what": "the percent"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "markdown",
      "properties": [
        {
          "name": "source",
          "nullable": false,
          "required": true,
          "type": "document",
          "what": "the markdown"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "code",
      "properties": [
        {
          "name": "source",
          "nullable": false,
          "required": true,
          "type": "document",
          "what": "the code"
        },
        {
          "name": "language",
          "nullable": true,
          "required": false,
          "type": "label",
          "what": "the language"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "key-value",
      "properties": [
        {
          "name": "pairs",
          "nullable": false,
          "required": true,
          "type": "pairs",
          "what": "the pairs"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "banner",
      "properties": [
        {
          "name": "text",
          "nullable": false,
          "required": true,
          "type": "body",
          "what": "the text"
        },
        {
          "name": "tone",
          "nullable": false,
          "required": false,
          "type": "tone",
          "what": "the tone"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": true,
      "kind": "link",
      "properties": [
        {
          "name": "label",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the label"
        },
        {
          "name": "url",
          "nullable": true,
          "required": false,
          "type": "url",
          "what": "the URL"
        }
      ],
      "rows": false
    },
    {
      "children": false,
      "interactive": false,
      "kind": "divider",
      "properties": [],
      "rows": false
    },
    {
      "children": true,
      "interactive": false,
      "kind": "empty-state",
      "properties": [
        {
          "name": "title",
          "nullable": false,
          "required": true,
          "type": "label",
          "what": "the title"
        },
        {
          "name": "text",
          "nullable": false,
          "required": true,
          "type": "body",
          "what": "the text"
        },
        {
          "name": "icon",
          "nullable": true,
          "required": false,
          "type": "icon",
          "what": "the icon name"
        }
      ],
      "rows": false
    }
  ],
  "events": {
    "change": [
      "path",
      "value"
    ],
    "click": [
      "path"
    ],
    "expand": [
      "path",
      "item",
      "page"
    ],
    "link": [
      "path"
    ],
    "select": [
      "path",
      "rows"
    ],
    "submit": [
      "path",
      "text"
    ]
  },
  "limits": {
    "choices": 256,
    "columns": 32,
    "depth": 32,
    "documentBytes": 65536,
    "iconLen": 40,
    "imageBytes": 1048576,
    "keyLen": 128,
    "labelLen": 256,
    "nodes": 50000,
    "problems": 100,
    "rowsPerPage": 10000,
    "spaceMax": 4,
    "textLen": 4096,
    "urlLen": 2048,
    "viewBytes": 8388608
  },
  "selections": [
    "none",
    "one",
    "many"
  ],
  "tones": [
    "neutral",
    "accent",
    "success",
    "warning",
    "danger",
    "muted"
  ]
};
