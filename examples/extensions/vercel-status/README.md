# Vercel Status

A Codeyard extension in TypeScript for [Vercel](https://vercel.com) projects:

- the status bar shows the state of the latest deployment (Ready, Building, Error); a click refreshes it
- the **Vercel Deployments** view in the sidebar lists the recent deployments with their target, branch, commit and a
  link to each
- **Vercel: Refresh Deployments** asks again; it also refreshes every minute (`interval`) while Codeyard is in front
- **Vercel: Set Access Token** and **Vercel: Forget Access Token**

The project is the one `vercel link` wrote to `.vercel/project.json` (its team too), or the `project` and `team`
settings under Settings › Extensions › Vercel Status.

## The token

Create a token at [vercel.com/account/tokens](https://vercel.com/account/tokens) and run **Vercel: Set Access Token**.
The input box hides what you type. The extension checks the token with Vercel (`GET /v2/user`), then keeps it in your
keychain with `secrets`. It sends the token only to `api.vercel.com`, the one host in its `net` permission.

Why not OAuth: Codeyard's `[[oauth]]` sign-in is for public clients (PKCE, no client secret), and Vercel's OAuth apps
need a client secret. A token the user creates is the way for such services.

## Run it

```sh
npm install    # the SDK, its types and TypeScript
npm test       # unit tests against a mock Codeyard, with fixture responses: no network
npm run check  # type-check
cyx test       # the built extension in the real extension host (no network or token there)
cyx dev        # run it in Codeyard
cyx package    # write codeyard.vercel-status-0.1.0.cyx
```

Install the package from the Extensions view (**Install from File…**), which shows its permissions first.
`codeyard --install-extension` refuses it, because a code extension that asks for permissions needs that sheet.

## How it is built

- `ctx.setStatusItem` for the `[[status-bar]]` item, `ctx.view` for the `[[views]]` entry, `ctx.every` for the timer.
- The view is built with the SDK's `ui` functions, which write the form of
  [the views reference](../../../docs/extensions/reference/ui.md): a `table` with a `badge` per state and a `link` per deployment, an `empty-state` with a button while there is no token.
- `[activation] workspace-contains` starts it in a folder with a `vercel.json`. `.vercel/project.json` is listed too,
  but `vercel link` adds `.vercel` to `.gitignore`, which that check honours; such a folder starts the extension on its
  first command or when the view opens.
