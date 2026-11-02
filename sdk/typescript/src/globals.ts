// Imported first by the component's entry module, before the extension's own modules run: the global `fetch` is the
// SDK's, mapped to the host's `net`, and the timers the engine cannot run say so.

import { installGlobals, refuseTimers } from "./guest.ts";

installGlobals();
refuseTimers();
