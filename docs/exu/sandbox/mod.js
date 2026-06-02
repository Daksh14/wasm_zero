// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.
//
// Copyright (c) DUSK NETWORK. All rights reserved.

import { NullTarget } from "../proxies.js";
import { WorkerPool } from "./pool.js";

// One process-wide pool of reusable workers, created lazily.
let sharedPool;
const pool = () => (sharedPool ??= new WorkerPool());

// Per-helper stack size (the wasm linker's default region is also 1 MiB).
const STACK_SIZE = 1 << 20;

/**
 * A per-task sandbox over pooled workers.
 *
 * A fresh wasm instance is created on a borrowed coordinator worker; when the
 * task ends the instance + memory are dropped (the sandbox boundary) and the
 * worker is returned to the pool rather than terminated. Calling
 * {@link Sandbox#initThreadPool} additionally borrows helper workers and wires
 * them into a rayon thread pool sharing the coordinator's memory.
 */
export class Sandbox {
  #module;
  #importsUrl;
  #signal;

  #coordinator;
  #helpers = [];
  #memory = null;
  #globals = {};
  #memoryPort;
  #ready;
  #threaded = false;
  #aborted = null;

  constructor({ module, importsUrl, signal }) {
    this.#module = module;
    this.#importsUrl = importsUrl;
    this.#signal = signal;

    const channel = new MessageChannel();
    this.#memoryPort = channel.port1;

    this.#ready = (async () => {
      if (signal?.aborted) throw signal.reason;

      [this.#coordinator] = await pool().acquire(1);

      const { memory, globals } = await this.#coordinator.request(
        { cmd: "instantiate", module, importsUrl },
        [channel.port2],
      );

      this.#memory = memory;
      this.#globals = globals;

      signal?.addEventListener("abort", () => this.#abort(signal.reason));
    })();
  }

  get ready() {
    return this.#ready;
  }

  get memory() {
    return this.#ready.then(() => this.#memory);
  }

  get globals() {
    return this.#ready.then(() => this.#globals);
  }

  /**
   * Async proxy over the coordinator's exports. Each access returns a function
   * that forwards `{ cmd: "call" }` to the coordinator worker.
   *
   * @type {WebAssembly.Exports}
   */
  get exports() {
    return new Proxy(NullTarget, {
      get:
        (_, member) =>
        (...args) =>
          this.#ready.then(() =>
            this.#coordinator.request({ cmd: "call", member, args }),
          ),
    });
  }

  /**
   * Bootstrap a rayon thread pool with up to `requested` threads (clamped to
   * the worker pool's capacity). The coordinator counts as one thread, so
   * `requested - 1` helper workers are borrowed, each instantiated over the
   * coordinator's shared memory and handed a private stack + TLS before running
   * its rayon loop. After this resolves, the parallel exports run across all
   * threads.
   *
   * @param {number} requested - desired thread count (e.g. hardwareConcurrency)
   * @returns {Promise<number>} the actual thread count
   */
  initThreadPool = async (requested) => {
    await this.#ready;

    if (self.crossOriginIsolated === false) {
      throw new Error(
        "initThreadPool requires a cross-origin-isolated context (COOP/COEP headers)",
      );
    }
    if (!(this.#memory instanceof WebAssembly.Memory)) {
      throw new Error("module has no shared memory to thread over");
    }

    const call = (member, args = []) =>
      this.#coordinator.request({ cmd: "call", member, args });

    // `requested` is the desired rayon worker-thread count. Each rayon worker
    // thread is a helper Worker; the coordinator is a *separate* worker that
    // issues the compute call and blocks (parked, not burning a core) inside
    // install(). Clamp so coordinator + helpers fit the pool.
    const helperCount = Math.max(
      1,
      Math.min((requested | 0) || 1, pool().size - 1),
    );

    // Single-threaded: skip the pool entirely — the compute runs inline on the
    // coordinator (Rust's `with_pool` runs directly when no pool exists). This
    // is the honest 1-thread baseline.
    if (helperCount <= 1) {
      return 1;
    }

    const tlsSize = await this.#coordinator.request({
      cmd: "getGlobal",
      name: "__tls_size",
    });
    const tlsAlign = await this.#coordinator.request({
      cmd: "getGlobal",
      name: "__tls_align",
    });

    // Allocate a stack (helpers) and/or TLS block from the shared heap; returns
    // packed (stackTop<<32 | tlsBase). The coordinator keeps its default stack,
    // so it asks for stack_size 0 and uses only the TLS base.
    const allocThread = async (stackSize) => {
      const packed = await call("alloc_thread_stack", [
        stackSize,
        tlsSize,
        tlsAlign,
      ]);
      const big = typeof packed === "bigint" ? packed : BigInt(packed);
      return { stackTop: Number(big >> 32n), tlsBase: Number(big & 0xffffffffn) };
    };

    // 0. The coordinator runs rayon's "main" thread, so it needs its OWN TLS
    //    too (the module never inits TLS for any instance). Keep its stack.
    const coord = await allocThread(0);
    await this.#coordinator.request({
      cmd: "bootstrapThread",
      tlsBase: coord.tlsBase,
    });

    // 1. Make the ThreadBuilder channel; rayon will spawn exactly `helperCount`
    //    worker threads. Returns the receiver pointer the helpers block on.
    const receiverPtr = await call("init_thread_pool", [helperCount]);
    this.#threaded = true;

    this.#helpers = await pool().acquire(helperCount);

    // Every helper instantiates over the SAME shared memory (a Memory whose
    // buffer is a SharedArrayBuffer aliases across the structured clone).
    await Promise.all(
      this.#helpers.map((helper) =>
        helper.request({
          cmd: "instantiate",
          module: this.#module,
          importsUrl: this.#importsUrl,
          memory: this.#memory,
        }),
      ),
    );

    // 2. Give each helper a private stack + TLS, then have it block in recv().
    for (let i = 0; i < helperCount; i++) {
      const { stackTop, tlsBase } = await allocThread(STACK_SIZE);

      await this.#helpers[i].request({
        cmd: "bootstrapThread",
        stackTop,
        tlsBase,
      });
      // Resolves on `{ started }`; the helper then blocks in recv() waiting for
      // its ThreadBuilder (delivered by build_thread_pool below).
      await this.#helpers[i].request({ cmd: "runHelperEntry", receiverPtr });
    }

    // 3. Build the pool now that every helper is waiting on the channel.
    await call("build_thread_pool");

    return helperCount;
  };

  /**
   * Copies memory between the WebAssembly and JavaScript contexts.
   *
   * @param {number|Uint8Array|null} dest
   * @param {number|Uint8Array} source
   * @param {number} [count]
   * @returns {Promise<void|Uint8Array>}
   */
  memcpy = async (dest, source, count) => {
    if (dest === null && typeof source === "number") {
      return await this.#sendMemory({ get: { source, count } });
    } else if (typeof dest === "number" && source instanceof Uint8Array) {
      return await this.#sendMemory({ set: { dest, source, count } }, [
        source.buffer,
      ]);
    } else if (typeof dest === "number" && typeof source === "number") {
      await this.#sendMemory({ set: { source, dest, count } });
    } else if (dest instanceof Uint8Array && source instanceof Uint8Array) {
      dest.set(source);
    } else {
      throw new TypeError("Invalid arguments.");
    }
  };

  #sendMemory(message, transfer = []) {
    return new Promise((resolve, reject) => {
      if (this.#aborted) {
        reject(this.#aborted);
        return;
      }
      this.#memoryPort.onmessage = ({ data }) => {
        data instanceof Error ? reject(data) : resolve(data);
        this.#memoryPort.onmessage = null;
      };
      this.#memoryPort.postMessage(message, transfer);
    });
  }

  // An aborted task may have wedged its workers (e.g. an infinite loop), so we
  // can't reclaim them — terminate and forget them, and reject in-flight calls.
  #abort(reason) {
    this.#aborted = reason;
    this.#memoryPort?.close();
    for (const worker of [this.#coordinator, ...this.#helpers]) {
      if (!worker) continue;
      worker.terminate(reason);
      pool().forget(worker);
    }
    this.#coordinator = null;
    this.#helpers = [];
  }

  /**
   * End the task: drop the wasm instance(s) + memory (the sandbox boundary) and
   * return the workers to the pool. Never terminates workers (except on abort).
   */
  terminate = async () => {
    if (this.#aborted) return;
    try {
      if (this.#threaded) {
        // Dropping the rayon pool unblocks the helpers' run() loops.
        await this.#coordinator.request({
          cmd: "call",
          member: "shutdown_thread_pool",
          args: [],
        });
        await Promise.all(
          this.#helpers.map((helper) => helper.request({ cmd: "drop" })),
        );
      }
      await this.#coordinator.request({ cmd: "drop" });
    } catch {
      // If graceful teardown fails, fall through to releasing what we can.
    } finally {
      this.#memoryPort?.close();
      const workers = [this.#coordinator, ...this.#helpers].filter(Boolean);
      if (workers.length) pool().release(workers);
      this.#coordinator = null;
      this.#helpers = [];
    }
  };
}
