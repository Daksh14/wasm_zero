// Imports for wasm_zero_rayon. The module imports a single thing — a *shared*
// linear memory as `env.memory`.
//
// We export a memory *descriptor* (not a constructed Memory) so exu can mint a
// FRESH shared memory for every task — even when a pooled worker is reused —
// which keeps the sandbox boundary at the memory, not the worker. The sizes
// match the module's declared `(memory 17 16384 shared)` limits.
export const memoryDescriptor = {
  initial: 17,
  maximum: 16384,
  shared: true,
};

export default {
  env: {},
};
