// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.
//
// Copyright (c) DUSK NETWORK. All rights reserved.

import worker from "./worker.js";

// One Blob URL for the worker code, shared by every spawned worker.
export const workerUrl = URL.createObjectURL(
  new Blob([`(${worker})()`], { type: "application/javascript" }),
);

/**
 * A long-lived Worker wrapped with id-correlated request/response messaging.
 *
 * The original sandbox overwrote `worker.onmessage` per call (one-shot). That
 * breaks for *reused* workers that serve many requests and for the threaded
 * bootstrap, where a helper replies `{ started }` and then blocks. Here every
 * request carries a monotonic id and the persistent `onmessage` resolves the
 * matching pending promise.
 */
export class RpcWorker {
  #worker;
  #pending = new Map();
  #seq = 0;

  constructor(url = workerUrl) {
    this.#worker = new Worker(url, { type: "module" });
    this.#worker.onmessage = ({ data }) => {
      const { id, result, error } = data ?? {};
      const entry = this.#pending.get(id);
      if (!entry) return;
      this.#pending.delete(id);
      error ? entry.reject(error) : entry.resolve(result);
    };
    this.#worker.onerror = (event) => {
      this.#rejectAll(
        event?.error ?? new Error(event?.message ?? "worker error"),
      );
    };
  }

  /**
   * Send a command and resolve with the worker's reply.
   *
   * @param {Object} command - `{ cmd, ...payload }`
   * @param {Transferable[]} [transfer] - objects to transfer (e.g. a port)
   * @returns {Promise<any>}
   */
  request(command, transfer = []) {
    const id = this.#seq++;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      this.#worker.postMessage({ id, ...command }, transfer);
    });
  }

  #rejectAll(reason) {
    for (const { reject } of this.#pending.values()) reject(reason);
    this.#pending.clear();
  }

  /** Terminate the underlying worker, rejecting any in-flight requests. */
  terminate(reason = new Error("worker terminated")) {
    this.#rejectAll(reason);
    this.#worker.terminate();
  }
}

/**
 * A persistent pool of reusable workers.
 *
 * Per the HTML spec, workers are expensive and meant to be long-lived; the
 * original sandbox terminated one per call. Here workers are pooled and reused
 * across tasks — the *sandbox boundary is the wasm instance + memory* (dropped
 * per task), not the worker. The pool is capped (default
 * `hardwareConcurrency - 1`, leaving a core for the main thread); requests
 * beyond the cap queue until a worker is released.
 */
export class WorkerPool {
  #url;
  #idle = [];
  #waiters = [];
  #created = 0;

  constructor(url = workerUrl, size) {
    this.#url = url;
    // One worker per core, plus one for the blocking coordinator (which parks
    // in install() rather than burning a core), so a full-width pool can run
    // `hardwareConcurrency` busy rayon threads.
    this.size =
      size ?? Math.max(2, ((globalThis.navigator?.hardwareConcurrency ?? 4) | 0) + 1);
  }

  #take() {
    if (this.#idle.length) return Promise.resolve(this.#idle.pop());
    if (this.#created < this.size) {
      this.#created++;
      return Promise.resolve(new RpcWorker(this.#url));
    }
    return new Promise((resolve) => this.#waiters.push(resolve));
  }

  /** Acquire `count` workers, waiting if the pool is saturated. */
  async acquire(count = 1) {
    const out = [];
    for (let i = 0; i < count; i++) out.push(await this.#take());
    return out;
  }

  /** Return workers to the pool for reuse (never terminates them). */
  release(workers) {
    for (const worker of workers) {
      const waiter = this.#waiters.shift();
      if (waiter) waiter(worker);
      else this.#idle.push(worker);
    }
  }

  /**
   * Drop a worker that can't be reused (e.g. wedged by an aborted task and
   * already terminated). Frees its slot, spawning a replacement if anyone is
   * waiting.
   */
  forget(worker) {
    this.#created = Math.max(0, this.#created - 1);
    const waiter = this.#waiters.shift();
    if (waiter) {
      this.#created++;
      waiter(new RpcWorker(this.#url));
    }
  }
}
