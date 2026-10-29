// node build.mjs <out.wasm> [source.js] [--aot]: componentize a source (default hello.js). Run from this folder.
import { componentize } from "@bytecodealliance/componentize-js";
import { writeFile } from "node:fs/promises";
import { _setPreopens } from "@bytecodealliance/preview2-shim/filesystem";

// preview2-shim no longer preopens anything by default; the splicer reads the WIT through it.
_setPreopens({ "/": "/" });

const { component } = await componentize({
  sourcePath: process.argv[3] ?? "hello.js",
  witPath: "../wit",
  worldName: "ext",
  // The extension gets no http or stdio of its own; the real SDK maps fetch onto the host's `net`.
  disableFeatures: ["http", "fetch-event"],
  enableAot: process.argv.includes("--aot"),
});
await writeFile(process.argv[2], component);
