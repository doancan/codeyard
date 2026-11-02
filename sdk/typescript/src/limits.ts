// The size limit of one answer, shared by the guest (it fails an answer over it before moving it out of the engine) and
// the mock host. Codeyard's own rule is `result_too_large` (crates/ext-manifest/src/calls.rs); test/rule-cases.json
// holds cases judged by it.

/** The most bytes one answer (a command's result, a view's tree) may carry. */
export const CALL_BYTES = 8 * 1024 * 1024;

/** The error for an answer of `bytes`, if it is over the call limit. */
export function resultTooLarge(bytes: number): string | undefined {
  const MIB = 1 << 20;
  if (bytes <= CALL_BYTES) return undefined;
  const tenths = Math.max(Math.floor((bytes * 10 + MIB / 2) / MIB), Math.floor((CALL_BYTES * 10) / MIB) + 1);
  const size = tenths % 10 === 0 ? `${tenths / 10}` : `${Math.floor(tenths / 10)}.${tenths % 10}`;
  return `the result is ${size} MiB; at most ${CALL_BYTES / MIB} MiB`;
}

