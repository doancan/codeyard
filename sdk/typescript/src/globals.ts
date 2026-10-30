// Imported first by the component's entry module, before the extension's own modules run: the global `fetch` is the
// SDK's, mapped to the host's `net`.

import { installGlobals } from "./guest.ts";

installGlobals();
