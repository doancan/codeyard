// `fetch` on top of the host's `net` (and, signed for an account, `auth`): the request leaves from the machine Codeyard
// runs on, and the extension never opens a socket. The Promise resolves when the response head arrives; the body
// streams in as the host delivers it. Installed as the global `fetch`, so npm packages that call `fetch` (Octokit,
// ky, the OpenAI client, …) work unchanged.

import { CodeyardError, fromWit } from "./errors.ts";
import { host, optional, wait } from "./runtime.ts";
import type { HttpRequest } from "./wit.ts";

/** `fetch`'s options, plus Codeyard's own: `auth` signs the request for an account (`auth = ["github"]`). */
export interface FetchInit {
  method?: string;
  headers?: HeadersInit;
  body?: BodyInit | null;
  signal?: AbortSignal | null;
  /** Sign the request for this `auth` provider; the extension never sees the token. */
  auth?: string;
  /** Give up after this many milliseconds. */
  timeoutMs?: number;
  [other: string]: unknown;
}

type HeadersInit = FetchHeaders | Iterable<readonly [string, string]> | Record<string, string | readonly string[] | undefined> | { forEach(f: (value: string, name: string) => void): void };
type BodyInit = string | Uint8Array | ArrayBuffer | ArrayBufferView | URLSearchParams;
type FetchInput = string | URL | { url: string; method?: string; headers?: HeadersInit };

/** The host part of a URL, for the `net = [...]` line a refusal suggests. */
export function hostOf(url: string): string {
  const rest = url.includes("://") ? url.slice(url.indexOf("://") + 3) : url;
  const authority = rest.split(/[/?#]/)[0] ?? "";
  const h = authority.slice(authority.lastIndexOf("@") + 1);
  return (h.split(":")[0] ?? h).toLowerCase();
}

/** Response and request headers. Names are case-insensitive and kept lowercase. */
export class FetchHeaders {
  #map = new Map<string, string[]>();

  constructor(init?: HeadersInit) {
    if (!init) return;
    if (init instanceof FetchHeaders) {
      for (const [k, v] of init) this.append(k, v);
    } else if (Symbol.iterator in init) {
      for (const [k, v] of init as Iterable<readonly [string, string]>) this.append(k, v);
    } else if (typeof (init as { forEach?: unknown }).forEach === "function") {
      (init as { forEach(f: (value: string, name: string) => void): void }).forEach((v, k) => this.append(k, v));
    } else {
      for (const [k, v] of Object.entries(init as Record<string, string | readonly string[] | undefined>)) {
        if (v === undefined) continue;
        for (const one of typeof v === "string" ? [v] : v) this.append(k, String(one));
      }
    }
  }

  append(name: string, value: string): void {
    const k = name.toLowerCase();
    const list = this.#map.get(k);
    if (list) list.push(String(value));
    else this.#map.set(k, [String(value)]);
  }

  set(name: string, value: string): void {
    this.#map.set(name.toLowerCase(), [String(value)]);
  }

  get(name: string): string | null {
    const list = this.#map.get(name.toLowerCase());
    return list ? list.join(", ") : null;
  }

  has(name: string): boolean {
    return this.#map.has(name.toLowerCase());
  }

  delete(name: string): void {
    this.#map.delete(name.toLowerCase());
  }

  getSetCookie(): string[] {
    return [...(this.#map.get("set-cookie") ?? [])];
  }

  forEach(f: (value: string, name: string, headers: FetchHeaders) => void): void {
    for (const [k, v] of this) f(v, k, this);
  }

  *entries(): IterableIterator<[string, string]> {
    for (const k of [...this.#map.keys()].sort()) yield [k, this.get(k) as string];
  }

  *keys(): IterableIterator<string> {
    for (const [k] of this.entries()) yield k;
  }

  *values(): IterableIterator<string> {
    for (const [, v] of this.entries()) yield v;
  }

  [Symbol.iterator](): IterableIterator<[string, string]> {
    return this.entries();
  }

  /** As the host takes them: one pair per value. */
  pairs(): [string, string][] {
    const out: [string, string][] = [];
    for (const [k, list] of this.#map) for (const v of list) out.push([k, v]);
    return out;
  }
}

/** Body chunks as they arrive, read once. */
class BodyQueue {
  #chunks: Uint8Array[] = [];
  #done = false;
  #error: unknown = undefined;
  #wake: (() => void) | undefined;

  push(chunk: Uint8Array): void {
    this.#chunks.push(chunk);
    this.#notify();
  }

  end(): void {
    this.#done = true;
    this.#notify();
  }

  fail(e: unknown): void {
    this.#error = e;
    this.#done = true;
    this.#notify();
  }

  #notify(): void {
    const w = this.#wake;
    this.#wake = undefined;
    w?.();
  }

  async read(): Promise<{ done: true; value?: undefined } | { done: false; value: Uint8Array }> {
    for (;;) {
      const chunk = this.#chunks.shift();
      if (chunk) return { done: false, value: chunk };
      if (this.#error !== undefined) throw this.#error;
      if (this.#done) return { done: true };
      await new Promise<void>((r) => {
        this.#wake = r;
      });
    }
  }
}

function concat(chunks: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

/** A response whose body streams in. The usual `Response` surface: `ok`, `status`, `headers`, `json()`, `text()`, `body`. */
export class FetchResponse {
  readonly status: number;
  readonly statusText: string = "";
  readonly headers: FetchHeaders;
  readonly url: string;
  readonly redirected = false;
  readonly type = "basic";
  #queue: BodyQueue;
  #used = false;
  #stream: unknown = undefined;

  constructor(status: number, headers: [string, string][], url: string, queue: BodyQueue) {
    this.status = status;
    this.headers = new FetchHeaders(headers);
    this.url = url;
    this.#queue = queue;
  }

  get ok(): boolean {
    return this.status >= 200 && this.status < 300;
  }

  get bodyUsed(): boolean {
    return this.#used;
  }

  /** The body as a `ReadableStream` of byte chunks, read as they arrive. */
  get body(): ReadableStream<Uint8Array> | null {
    if (this.#stream === undefined) {
      if (typeof ReadableStream === "undefined") return null;
      this.#used = true;
      const q = this.#queue;
      this.#stream = new ReadableStream<Uint8Array>({
        async pull(controller) {
          const r = await q.read();
          if (r.done) controller.close();
          else controller.enqueue(r.value);
        },
      });
    }
    return this.#stream as ReadableStream<Uint8Array>;
  }

  /** The body's chunks as they arrive: `for await (const chunk of response.chunks())`. */
  async *chunks(): AsyncGenerator<Uint8Array> {
    this.#take();
    for (;;) {
      const r = await this.#queue.read();
      if (r.done) return;
      yield r.value;
    }
  }

  async bytes(): Promise<Uint8Array> {
    const all: Uint8Array[] = [];
    for await (const c of this.chunks()) all.push(c);
    return concat(all);
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    const b = await this.bytes();
    return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer;
  }

  async text(): Promise<string> {
    return new TextDecoder().decode(await this.bytes());
  }

  async json(): Promise<any> {
    return JSON.parse(await this.text());
  }

  async blob(): Promise<Blob> {
    if (typeof Blob === "undefined") throw new TypeError("Blob is not available; use bytes() or arrayBuffer()");
    return new Blob([(await this.bytes()) as Uint8Array<ArrayBuffer>], { type: this.headers.get("content-type") ?? "" });
  }

  /** A copy that reads the same body. Only before the body is read. */
  clone(): FetchResponse {
    this.#take();
    const [a, b] = [new BodyQueue(), new BodyQueue()];
    const source = this.#queue;
    (async () => {
      try {
        for (;;) {
          const r = await source.read();
          if (r.done) break;
          a.push(r.value);
          b.push(r.value);
        }
        a.end();
        b.end();
      } catch (e) {
        a.fail(e);
        b.fail(e);
      }
    })();
    this.#queue = a;
    this.#used = false;
    return new FetchResponse(this.status, this.headers.pairs(), this.url, b);
  }

  #take(): void {
    if (this.#used) throw new TypeError("the body was already read");
    this.#used = true;
  }
}

function bodyBytes(body: BodyInit | null | undefined, headers: FetchHeaders): Uint8Array {
  if (body === null || body === undefined) return new Uint8Array(0);
  if (typeof body === "string") return new TextEncoder().encode(body);
  if (body instanceof Uint8Array) return body;
  if (body instanceof ArrayBuffer) return new Uint8Array(body);
  if (ArrayBuffer.isView(body)) return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
  if (typeof URLSearchParams !== "undefined" && body instanceof URLSearchParams) {
    if (!headers.has("content-type")) headers.set("content-type", "application/x-www-form-urlencoded;charset=UTF-8");
    return new TextEncoder().encode(body.toString());
  }
  throw new TypeError("fetch: the body must be a string, bytes or URLSearchParams in a Codeyard extension");
}

/** `fetch` mapped to the host's `net` (or `auth` with `init.auth`). Needs `net = ["<host>"]` in the manifest. */
export function fetch(input: FetchInput, init: FetchInit = {}): Promise<FetchResponse> {
  let url: string;
  let method = init.method;
  let headers: FetchHeaders;
  if (typeof input === "string") {
    url = input;
    headers = new FetchHeaders(init.headers);
  } else if (typeof (input as { url?: unknown }).url === "string" && !(typeof URL !== "undefined" && input instanceof URL)) {
    const r = input as { url: string; method?: string; headers?: HeadersInit };
    url = r.url;
    method ??= r.method;
    headers = new FetchHeaders(init.headers ?? r.headers);
  } else {
    url = String(input);
    headers = new FetchHeaders(init.headers);
  }
  const provider = init.auth;
  const line = provider ? () => `auth = ["${provider}"]` : () => `net = ["${hostOf(url)}"]`;
  let request: HttpRequest;
  try {
    const body = bodyBytes(init.body, headers);
    request = { method: (method ?? "GET").toUpperCase(), url, headers: headers.pairs(), body, timeoutMs: init.timeoutMs };
  } catch (e) {
    return Promise.reject(e);
  }
  const signal = init.signal ?? undefined;
  if (signal?.aborted) return Promise.reject(signal.reason ?? new CodeyardError("cancelled", "cancelled"));

  return new Promise((resolve, reject) => {
    let ticket: bigint;
    try {
      ticket = provider ? optional("auth", line).fetch(provider, request) : optional("net", line).fetch(request);
    } catch (e) {
      reject(e);
      return;
    }
    const queue = new BodyQueue();
    let response: FetchResponse | undefined;
    const aborted = () => {
      host().tickets.cancel(ticket);
    };
    signal?.addEventListener?.("abort", aborted);
    wait(ticket, (e) => {
      if (e.tag === "http-head" && !response) {
        response = new FetchResponse(e.val.status, e.val.headers, url, queue);
        resolve(response);
      } else if (e.tag === "data") {
        queue.push(e.val);
      }
    }).then((c) => {
      signal?.removeEventListener?.("abort", aborted);
      if (c.tag === "failed") {
        const err = signal?.aborted ? (signal.reason ?? fromWit(c.val, line)) : fromWit(c.val, line);
        if (response) queue.fail(err);
        else reject(err);
        return;
      }
      if (c.tag === "bytes") queue.push(c.val);
      if (!response) {
        response = new FetchResponse(200, [], url, queue);
        resolve(response);
      }
      queue.end();
    });
  });
}
