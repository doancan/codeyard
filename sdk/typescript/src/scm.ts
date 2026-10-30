// Source control providers (`[[scm]]`): the state Source Control shows for a folder the provider claims, as authors
// write it (optional fields may be left out), and the arguments its buttons pass to their commands.

import { CodeyardError } from "./errors.ts";
import type { Uri } from "./types.ts";
import type { FileState, WitScmAction, WitScmState } from "./wit.ts";

export type { FileState };

/** A button that runs one of the extension's own commands (local id) with {@link ScmArgs}. */
export interface ScmAction {
  command: string;
  /** The tooltip and the screen reader's name. */
  title: string;
  /** An icon name (reference/ui.md): `plus`, `minus`, `undo-2`, `check`. */
  icon?: string;
}

export interface ScmResource {
  uri: Uri;
  state: FileState;
  /** Shown after the file name, such as the old name of a renamed file. */
  detail?: string;
}

export interface ScmGroup {
  /** Local to the provider; keeps the group open or closed between refreshes. */
  id: string;
  label: string;
  resources: ScmResource[];
  /** Buttons on each file; they get that file's uri. */
  resourceActions?: ScmAction[];
  /** Buttons on the header; they get every uri of the group. */
  groupActions?: ScmAction[];
  hideWhenEmpty?: boolean;
}

/** The input box above the groups. */
export interface ScmInput {
  placeholder?: string;
  /** Replaces the box's text when set. */
  value?: string;
  /** The button under the box (and Ctrl+Enter); none hides the box. */
  accept?: ScmAction;
}

/** What Source Control shows for one folder. */
export interface ScmState {
  /** Where the folder stands: a branch, a change id. */
  head?: string;
  input?: ScmInput;
  groups: ScmGroup[];
}

/** The arguments a source control button passes to its command. */
export interface ScmArgs {
  /** The provider's local id. */
  provider: string;
  /** The folder. */
  root: Uri;
  /** The file the button is on, or every file of the group. */
  uris: Uri[];
  /** The input box's text. */
  message: string;
}

/** Read a command's arguments; throws `invalid` when the command was not started by a source control button. */
export function scmArgs(args: unknown): ScmArgs {
  const a = args as Partial<ScmArgs> | null;
  if (!a || typeof a.provider !== "string" || typeof a.root !== "string") {
    throw CodeyardError.invalid("not the arguments of a source control button");
  }
  return { provider: a.provider, root: a.root, uris: Array.isArray(a.uris) ? a.uris : [], message: typeof a.message === "string" ? a.message : "" };
}

function action(a: ScmAction): WitScmAction {
  return { command: a.command, title: a.title, icon: a.icon };
}

export function toWitScmState(s: ScmState): WitScmState {
  return {
    head: s.head,
    input: { placeholder: s.input?.placeholder ?? "", value: s.input?.value, accept: s.input?.accept && action(s.input.accept) },
    groups: (s.groups ?? []).map((g) => ({
      id: g.id,
      label: g.label,
      resources: g.resources.map((r) => ({ uri: r.uri, state: r.state, detail: r.detail })),
      resourceActions: (g.resourceActions ?? []).map(action),
      groupActions: (g.groupActions ?? []).map(action),
      hideWhenEmpty: g.hideWhenEmpty ?? false,
    })),
  };
}
