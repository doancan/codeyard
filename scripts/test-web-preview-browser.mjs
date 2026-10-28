// Optional browser acceptance driver. No npm packages; use Node with global WebSocket.
// Start the ignored web_preview_browser_acceptance Rust test first, then this driver.
// BROWSER_EXECUTABLE can select a Chromium executable; Windows defaults to Edge.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
const target = path.resolve('target');
const done = path.join(target, 'web-preview-browser-done.json');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(fn, label, timeout = 20000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const result = await fn().catch(() => false);
    if (result) return result;
    await delay(150);
  }
  throw new Error('Timed out: ' + label);
}
const profile = await fs.mkdtemp(path.join(os.tmpdir(), 'codeyard-preview-browser-'));
let browser, ws;
try {
  const fixture = await until(async () => JSON.parse(await fs.readFile(path.join(target, 'web-preview-browser.json'), 'utf8')), 'Rust preview fixture', 90000);
  const executable = process.env.BROWSER_EXECUTABLE || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
  browser = spawn(executable, ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--remote-debugging-port=0', '--user-data-dir=' + profile, 'about:blank'], {windowsHide: true, stdio: 'ignore'});
  let spawnError;
  browser.on('error', error => { spawnError = error; });
  const port = await until(async () => {
    if (spawnError) throw spawnError;
    return (await fs.readFile(path.join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0];
  }, 'browser debugger');
  const page = await (await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(fixture.url)}`, {method: 'PUT'})).json();
  ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  let id = 0;
  const pending = new Map();
  ws.onmessage = event => {
    const message = JSON.parse(event.data);
    const reply = pending.get(message.id);
    if (reply) { pending.delete(message.id); message.error ? reply.reject(new Error(JSON.stringify(message.error))) : reply.resolve(message.result); }
  };
  function call(method, params = {}) {
    return new Promise((resolve, reject) => { const key = ++id; pending.set(key, {resolve, reject}); ws.send(JSON.stringify({id: key, method, params})); });
  }
  async function evaluate(expression) {
    const result = await call('Runtime.evaluate', {expression, returnByValue: true});
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  }
  await until(() => evaluate("document.querySelector('h1')?.textContent === 'Before' && document.documentElement.dataset.script === 'before'"), 'HTML, root-relative JS and cookie authentication');
  await until(() => evaluate("document.querySelector('script[src=\"/__codeyard_live.js\"]')?.dataset.interval === '500'"), 'configured live reload interval');
  await until(() => evaluate("getComputedStyle(document.querySelector('h1')).color === 'rgb(255, 0, 0)'"), 'initial CSS');
  await evaluate('window.previewSentinel = 42');
  await delay(1200); // Let the agent install its watches and the browser take its initial revision.
  await fs.writeFile(path.join(fixture.root, 'style.css'), 'h1 { color: rgb(0, 0, 255); }');
  await until(() => evaluate("getComputedStyle(document.querySelector('h1')).color === 'rgb(0, 0, 255)'"), 'live CSS update');
  if (await evaluate('window.previewSentinel') !== 42) throw new Error('CSS change reloaded the document');
  await fs.writeFile(path.join(fixture.root, 'app.js'), "document.documentElement.dataset.script = 'after';");
  await until(() => evaluate("document.documentElement.dataset.script === 'after'"), 'JavaScript reload');
  await fs.writeFile(path.join(fixture.root, 'index.html'), '<!doctype html><link rel=stylesheet href=/style.css><h1>After</h1><script src=/app.js></script>');
  await until(() => evaluate("document.querySelector('h1')?.textContent === 'After'"), 'HTML reload');
  const result = {passed: true, checks: ['HTML rendering', 'authenticated root-relative assets', 'configured live reload interval', 'CSS update without navigation', 'JavaScript reload', 'HTML reload', 'external file changes through agent watcher']};
  await fs.writeFile(done, JSON.stringify(result));
  console.log(JSON.stringify(result, null, 2));
} catch (error) {
  await fs.writeFile(done, JSON.stringify({passed: false, error: String(error)}));
  console.error(error);
  process.exitCode = 1;
} finally {
  ws?.close();
  if (browser && browser.exitCode === null) {
    browser.kill();
    await Promise.race([new Promise(resolve => browser.once('exit', resolve)), delay(5000)]);
  }
  // Only the exact throwaway directory returned by mkdtemp is removed.
  if (path.dirname(path.resolve(profile)) !== path.resolve(os.tmpdir()) || !path.basename(profile).startsWith('codeyard-preview-browser-')) {
    throw new Error('Unexpected browser profile cleanup path');
  }
  await fs.rm(profile, {recursive: true, force: true, maxRetries: 10, retryDelay: 200}).catch(error => console.error('Temporary browser profile cleanup:', String(error)));
}
