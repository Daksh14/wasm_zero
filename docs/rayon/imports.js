// Imports for wasm_zero_rayon_demo — a single shared `env.memory`.
//
// Exported as a *descriptor* (not a constructed Memory) so exu mints a fresh
// shared memory per task even on a reused worker (sandbox boundary = memory).
// 16384 pages = 1 GiB max, matching the module's `(memory 17 16384 shared)` —
// plenty for a large RGBA canvas buffer.
export const memoryDescriptor = {
  initial: 32,
  maximum: 16384,
  shared: true,
};

export default {
  env: {},
};
