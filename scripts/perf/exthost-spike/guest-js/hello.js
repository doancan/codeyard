import { log } from "codeyard:spike/host@0.1.0";

export function activate() {
  log("activated");
}

export function call(input) {
  return `hello ${input}`;
}
