// Enough TOML for `codeyard-extension.toml`: tables, arrays of tables, dotted keys, strings (basic, literal and
// multi-line), numbers, booleans, arrays and inline tables. The mock host reads `[permissions]` and `[settings.*]`.

type Value = string | number | boolean | Value[] | Table;
export interface Table {
  [key: string]: Value;
}

export function parseToml(text: string): Table {
  const root: Table = {};
  let current = root;
  let i = 0;
  let line = 1;

  const fail = (msg: string): never => {
    throw new Error(`codeyard-extension.toml:${line}: ${msg}`);
  };
  const peek = () => text[i];
  const skipSpace = () => {
    while (text[i] === " " || text[i] === "\t") i++;
  };
  const skipComment = () => {
    if (text[i] === "#") while (i < text.length && text[i] !== "\n") i++;
  };
  // Spaces, newlines and comments, inside arrays.
  const skipAll = () => {
    for (;;) {
      skipSpace();
      skipComment();
      if (text[i] === "\n" || text[i] === "\r") {
        if (text[i] === "\n") line++;
        i++;
      } else return;
    }
  };
  const endOfLine = () => {
    skipSpace();
    skipComment();
    if (text[i] === "\r") i++;
    if (i < text.length && text[i] !== "\n") fail(`unexpected '${text[i]}'`);
    i++;
    line++;
  };

  const bareKey = () => {
    const start = i;
    while (i < text.length && /[A-Za-z0-9_-]/.test(text[i] as string)) i++;
    if (start === i) fail("expected a key");
    return text.slice(start, i);
  };
  const key = (): string[] => {
    const parts: string[] = [];
    for (;;) {
      skipSpace();
      parts.push(peek() === '"' || peek() === "'" ? (str() as string) : bareKey());
      skipSpace();
      if (peek() !== ".") return parts;
      i++;
    }
  };

  const str = (): string => {
    const quote = text[i] as string;
    const multi = text.startsWith(quote.repeat(3), i);
    i += multi ? 3 : 1;
    if (multi && text[i] === "\n") {
      i++;
      line++;
    } else if (multi && text.startsWith("\r\n", i)) {
      i += 2;
      line++;
    }
    let out = "";
    for (;;) {
      if (i >= text.length) fail("unterminated string");
      if (multi ? text.startsWith(quote.repeat(3), i) : text[i] === quote) {
        i += multi ? 3 : 1;
        return out;
      }
      const c = text[i++] as string;
      if (c === "\n") {
        if (!multi) fail("newline in a string");
        line++;
      }
      if (c === "\\" && quote === '"') {
        const e = text[i++] as string;
        const simple: Record<string, string> = { n: "\n", t: "\t", r: "\r", '"': '"', "\\": "\\", b: "\b", f: "\f" };
        if (e in simple) out += simple[e];
        else if (e === "u" || e === "U") {
          const n = e === "u" ? 4 : 8;
          out += String.fromCodePoint(parseInt(text.slice(i, i + n), 16));
          i += n;
        } else if (multi && (e === "\n" || e === " " || e === "\r")) {
          while (/[\s]/.test(text[i] ?? "")) {
            if (text[i] === "\n") line++;
            i++;
          }
        } else fail(`unknown escape \\${e}`);
      } else out += c;
    }
  };

  const value = (): Value => {
    const c = peek();
    if (c === '"' || c === "'") return str();
    if (c === "[") {
      i++;
      const arr: Value[] = [];
      for (;;) {
        skipAll();
        if (peek() === "]") {
          i++;
          return arr;
        }
        arr.push(value());
        skipAll();
        if (peek() === ",") i++;
        else if (peek() !== "]") fail("expected , or ] in an array");
      }
    }
    if (c === "{") {
      i++;
      const t: Table = {};
      skipSpace();
      if (peek() === "}") {
        i++;
        return t;
      }
      for (;;) {
        const k = key();
        if (peek() !== "=") fail("expected =");
        i++;
        skipSpace();
        set(t, k, value());
        skipSpace();
        if (peek() === "}") {
          i++;
          return t;
        }
        if (peek() !== ",") fail("expected , or } in an inline table");
        i++;
      }
    }
    const m = /^[^\s,\]}#]+/.exec(text.slice(i));
    if (!m) return fail("expected a value");
    i += m[0].length;
    const word = m[0];
    if (word === "true") return true;
    if (word === "false") return false;
    const n = Number(word.replace(/_/g, ""));
    if (Number.isNaN(n)) fail(`'${word}' is not a value`);
    return n;
  };

  const table = (t: Table, k: string, array: boolean): Table => {
    let next = t[k];
    if (array) {
      if (next === undefined) next = t[k] = [];
      if (!Array.isArray(next)) fail(`${k} is not an array of tables`);
      const fresh: Table = {};
      (next as Value[]).push(fresh);
      return fresh;
    }
    if (next === undefined) next = t[k] = {};
    if (Array.isArray(next)) next = next[next.length - 1] as Value;
    if (typeof next !== "object") fail(`${k} is not a table`);
    return next as Table;
  };
  const set = (t: Table, k: string[], v: Value) => {
    let at = t;
    for (const part of k.slice(0, -1)) at = table(at, part, false);
    at[k[k.length - 1] as string] = v;
  };

  while (i < text.length) {
    skipSpace();
    const c = peek();
    if (c === "#" || c === "\n" || c === "\r") {
      endOfLine();
      continue;
    }
    if (c === "[") {
      const array = text[i + 1] === "[";
      i += array ? 2 : 1;
      const k = key();
      if (!text.startsWith(array ? "]]" : "]", i)) fail("expected ] after a table name");
      i += array ? 2 : 1;
      current = root;
      k.forEach((part, n) => {
        current = table(current, part, array && n === k.length - 1);
      });
      endOfLine();
      continue;
    }
    const k = key();
    if (peek() !== "=") fail("expected =");
    i++;
    skipSpace();
    set(current, k, value());
    endOfLine();
  }
  return root;
}
