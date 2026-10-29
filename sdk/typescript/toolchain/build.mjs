// node build.mjs <job.json>: bundle a TypeScript or JavaScript extension with esbuild, then build its WebAssembly
// component with ComponentizeJS. `cyx` writes this file, its package.json and lock file into a cache folder, installs
// them with `npm ci`, and runs it; authors never call it. The job:
//
//   entry       the extension's main module ([code] entry), absolute
//   dir         the extension's folder
//   sdk         the folder of @codeyard/extension's sources that cyx ships (the SDK always matches the API cyx builds for)
//   wit         a WIT folder whose `extension` world imports only the interfaces the manifest grants
//   interfaces  those interfaces, by WIT name
//   bundle      where to write the bundle (its file name is what stack traces show)
//   out         where to write extension.wasm
//   map         where to write the bundle's source map (sources relative to the extension's folder)
//
// Errors go to stderr in cyx's format (`error: …` with a `= fix:` line) and exit with 1.

import { builtinModules } from "node:module";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

const job = JSON.parse(await readFile(process.argv[2], "utf8"));
const builtins = new Set(builtinModules.flatMap((m) => [m, `node:${m}`]));
const camel = (s) => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
const slash = (p) => p.split(path.sep).join("/");

function fail(lines) {
  process.stderr.write(lines.join("\n") + "\n");
  process.exit(1);
}

// ---- bundle -----------------------------------------------------------------------------------------------------

const { build } = await import("esbuild");

// The entry: the SDK's globals first (so the extension's modules see the SDK's `fetch`), then the extension, then the
// granted interfaces, wired into the guest exports.
const imports = job.interfaces.map((name, i) => `import * as i${i} from "codeyard:api/${name}@0.1.0";`);
const table = job.interfaces.map((name, i) => `${camel(name)}: i${i}`).join(", ");
const entry = [
  `import "@codeyard/extension/globals";`,
  `import * as extension from ${JSON.stringify(slash(job.entry))};`,
  `import { start } from "@codeyard/extension/guest";`,
  ...imports,
  `export const guest = start(extension, { ${table} });`,
].join("\n");

const sdkFiles = { "": "index.ts", "/guest": "guest.ts", "/globals": "globals.ts" };
const codeyard = {
  name: "codeyard",
  setup(b) {
    // The host's interfaces stay imports of the component.
    b.onResolve({ filter: /^codeyard:/ }, (a) => ({ path: a.path, external: true, sideEffects: false }));
    // The SDK is the copy cyx ships, whatever version node_modules has: its glue must match the API cyx builds for.
    b.onResolve({ filter: /^@codeyard\/extension(\/.*)?$/ }, (a) => {
      const sub = a.path.slice("@codeyard/extension".length);
      if (sub === "/testing") {
        return { errors: [{ text: "@codeyard/extension/testing is the mock host for `npm test`; the extension's own code cannot import it" }] };
      }
      const file = sdkFiles[sub];
      if (!file) return { errors: [{ text: `@codeyard/extension has no module "${sub.slice(1)}"` }] };
      return { path: path.join(job.sdk, file) };
    });
  },
};

let result;
try {
  result = await build({
    stdin: { contents: entry, resolveDir: job.dir, sourcefile: "codeyard-entry.js", loader: "js" },
    absWorkingDir: job.dir,
    bundle: true,
    format: "esm",
    // Packages' browser builds use `fetch` and no Node built-ins.
    platform: "browser",
    target: "es2022",
    outfile: job.bundle,
    write: false,
    sourcemap: "external",
    sourcesContent: false,
    keepNames: true,
    legalComments: "none",
    logLevel: "silent",
    define: { "process.env.NODE_ENV": '"production"' },
    plugins: [codeyard],
  });
} catch (e) {
  if (!e.errors) throw e;
  fail(e.errors.flatMap(describe).concat([`\n${e.errors.length} problem${e.errors.length === 1 ? "" : "s"} found`]));
}

function describe(m) {
  const where = m.location ? `  --> ${slash(m.location.file)}:${m.location.line}:${m.location.column + 1}` : null;
  const unresolved = /^Could not resolve "([^"]+)"/.exec(m.text);
  if (unresolved && builtins.has(unresolved[1])) {
    const name = unresolved[1];
    const from = m.location && m.location.file.includes("node_modules/") ? ` (the package ${pkgOf(m.location.file)} needs it)` : "";
    return [
      `error: "${name}" is a Node.js built-in module, which Codeyard extensions do not have${from}`,
      ...(where ? [where] : []),
      "   = fix: use ctx.readFile / ctx.writeFile for workspace files, fetch for HTTP and ctx.run for programs; pick npm packages that work in browsers",
    ];
  }
  if (unresolved && !unresolved[1].startsWith(".")) {
    return [`error: ${m.text}`, ...(where ? [where] : []), `   = fix: npm install (is "${unresolved[1]}" in package.json?)`];
  }
  return [`error: ${m.text}`, ...(where ? [where] : []), ...(m.notes ?? []).map((n) => `   = note: ${n.text}`)];
}

function pkgOf(file) {
  const parts = slash(file).split("node_modules/").pop().split("/");
  return parts[0].startsWith("@") ? `${parts[0]}/${parts[1]}` : parts[0];
}

const js = result.outputFiles.find((f) => f.path.endsWith(".js"));
const map = result.outputFiles.find((f) => f.path.endsWith(".map"));
await mkdir(path.dirname(job.bundle), { recursive: true });
await writeFile(job.bundle, js.contents);

// Sources relative to the extension's folder; the SDK's as @codeyard/extension/….
const sourceMap = JSON.parse(map.text);
sourceMap.sources = sourceMap.sources.map((s) => {
  const abs = path.resolve(path.dirname(job.bundle), s);
  const inSdk = path.relative(job.sdk, abs);
  if (!inSdk.startsWith("..") && !path.isAbsolute(inSdk)) return `@codeyard/extension/src/${slash(inSdk)}`;
  const inExt = path.relative(job.dir, abs);
  return slash(inExt.startsWith("..") ? abs : inExt);
});
sourceMap.file = path.basename(job.bundle);
await writeFile(job.map, JSON.stringify(sourceMap));

// ---- componentize -----------------------------------------------------------------------------------------------

const { componentize } = await import("@bytecodealliance/componentize-js");
// ComponentizeJS reads the WIT through preview2-shim, which preopens nothing by default.
const { _setPreopens } = await import("@bytecodealliance/preview2-shim/filesystem");
_setPreopens({ "/": "/" });

let component;
try {
  // Relative to the working folder (cyx runs this in the folder that holds both), as the spike built them on every
  // platform: preview2-shim maps the preopened root to the drive of the working folder on Windows.
  ({ component } = await componentize({
    sourcePath: path.relative(process.cwd(), job.bundle),
    witPath: path.relative(process.cwd(), job.wit),
    worldName: "extension",
    // No wasi:http or sockets: `fetch` goes through the host's `net` (the import check refuses the others).
    disableFeatures: ["http", "fetch-event"],
  }));
} catch (e) {
  fail([`error: ComponentizeJS could not build the component: ${e.message ?? e}`, "   = fix: run the extension's code under Node (npm test) to find the error, or report it with the message above"]);
}
await writeFile(job.out, component);
