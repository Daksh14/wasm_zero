// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.
//
// Copyright (c) DUSK NETWORK. All rights reserved.

// The worker is a reusable *command server*: it stays alive across tasks and
// handles id-tagged commands (see `RpcWorker`). A worker plays one of two roles
// per task — the coordinator (runs the wasm "main" thread and the exported
// calls) or a helper (a rayon worker thread sharing the coordinator's memory).
// All wasm runs here, never on the page's main thread, because rayon blocks on
// `atomics.wait`, which is illegal on the main thread.
export default function () {
  const Internals = {
    instance: null,
    imports: null,
    memory: null,
    memoryPort: null,
  };

  const getGlobals = (module) =>
    WebAssembly.Module.exports(module)
      .filter(({ kind, name }) => kind === "global" && !name.startsWith("__"))
      .reduce(
        (acc, item) => (
          (acc[item.name] = Internals.instance.exports[item.name].value), acc
        ),
        {},
      );

  // Instantiate the module. Memory resolution, in priority order:
  //   1. a `memory` injected by the sandbox (helpers share the coordinator's),
  //   2. a fresh Memory minted from the imports module's `memoryDescriptor`
  //      (so a reused worker still gets clean memory per task — the sandbox
  //      boundary), or
  //   3. whatever the imports module's `default.env.memory` already holds.
  async function instantiate({ module, importsUrl, memory }, port) {
    let importsModule;
    let imports;

    if (typeof importsUrl === "string") {
      importsModule = await import(importsUrl);
      imports = importsModule.default;
    }

    if (!(memory instanceof WebAssembly.Memory) && importsModule?.memoryDescriptor) {
      memory = new WebAssembly.Memory(importsModule.memoryDescriptor);
    }

    if (memory instanceof WebAssembly.Memory) {
      imports = {
        ...(imports ?? {}),
        env: { ...((imports ?? {}).env ?? {}), memory },
      };
    }

    Internals.instance = imports
      ? new WebAssembly.Instance(module, imports)
      : new WebAssembly.Instance(module);
    Internals.imports = imports;
    Internals.memory =
      (memory instanceof WebAssembly.Memory ? memory : null) ??
      imports?.env?.memory ??
      Internals.instance.exports?.memory ??
      null;

    if (importsModule && typeof importsModule.oninit === "function") {
      await importsModule.oninit(Internals.instance);
    }

    if (port) {
      Internals.memoryPort = port;
      Internals.memoryPort.onmessage = handleMemoryRequest;
    }

    // Only hand the memory to the main thread when cross-origin isolated (i.e.
    // a SharedArrayBuffer); otherwise direct access is forbidden.
    const exposeMemory =
      self.crossOriginIsolated !== false ? Internals.memory : null;

    return { memory: exposeMemory, globals: getGlobals(module) };
  }

  function handleMemoryRequest({ data: { get, set } }) {
    const memory = Internals.memory;

    if (!(memory instanceof WebAssembly.Memory)) {
      throw new ReferenceError("WebAssembly.Memory is not defined");
    } else if (set) {
      const { dest, source, count } = set;
      const length = count ?? source.byteLength ?? source.length;

      new Uint8Array(memory.buffer, dest, length).set(source);
      Internals.memoryPort.postMessage(source, [source.buffer]);
    } else if (get) {
      const { source, count } = get;
      const length = count ?? source.byteLength ?? source.length;
      Internals.memoryPort.postMessage(
        new Uint8Array(memory.buffer.slice(source, source + length)),
      );
    } else {
      throw new TypeError("Invalid memory request");
    }
  }

  addEventListener("message", async ({ data, ports }) => {
    const { id, cmd } = data;

    try {
      switch (cmd) {
        case "instantiate": {
          const result = await instantiate(data, ports?.[0]);
          postMessage({ id, result });
          return;
        }
        case "getGlobal": {
          postMessage({ id, result: Internals.instance.exports[data.name].value });
          return;
        }
        case "call": {
          const fn = Internals.instance.exports[data.member];
          if (typeof fn !== "function") {
            throw new TypeError(`${data.member} is not a function`);
          }
          postMessage({ id, result: fn(...data.args) });
          return;
        }
        case "bootstrapThread": {
          // Give this instance its OWN thread-locals (and, for helpers, its own
          // stack). wasm globals are per-instance and the module never calls
          // __wasm_init_tls itself, so EVERY instance — coordinator included —
          // must do this before running rayon, or thread-locals alias and the
          // helpers' stacks clobber each other.
          const exports = Internals.instance.exports;
          if (typeof data.stackTop === "number") {
            exports.__stack_pointer.value = data.stackTop;
          }
          exports.__wasm_init_tls(data.tlsBase);
          postMessage({ id, result: { ready: true } });
          return;
        }
        case "runHelperEntry": {
          // Signal liveness FIRST (the call below blocks in recv() until the
          // pool is built, then runs the rayon loop until teardown).
          postMessage({ id, result: { started: true } });
          Internals.instance.exports.wasm_thread_entry(data.receiverPtr);
          return;
        }
        case "drop": {
          // Drop the wasm instance + memory (the sandbox boundary) while the
          // worker itself stays alive for reuse.
          if (Internals.memoryPort) Internals.memoryPort.onmessage = null;
          Internals.instance = null;
          Internals.imports = null;
          Internals.memory = null;
          Internals.memoryPort = null;
          postMessage({ id, result: { dropped: true } });
          return;
        }
        default:
          throw new TypeError(`unknown command: ${cmd}`);
      }
    } catch (error) {
      // Structured clone preserves standard Error subtypes (e.g. TypeError).
      postMessage({ id, error });
    }
  });
}
