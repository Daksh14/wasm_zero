// wasm_bindgen vs wasm_zero benchmark harness.
//
// Adapted from benchmark/web/index.js to export an `initBenchmark({...})`
// entry point (instead of auto-running on import) so it can mount inside the
// shared tabbed docs page. Paths are relative to this file (docs/benchmark/).
//
// Note: the wasm_zero bindings here decode straight from wasm memory and the
// benchmark exports take only scalar args, so nothing imports rkyv-js — this
// harness ships fully self-contained, no network needed.
import initWb, * as wb from './pkg/wasm-bindgen/bench_wasm_bindgen.js';
import { initWasmZero } from './pkg/wasm-zero/bindings.js';

const WZ_WASM = new URL('./pkg/wasm-zero/bench_wasm_zero.wasm', import.meta.url);

// --- pure-JS baseline suite ------------------------------------------------

function jsFib(n) {
  let a = 1n, b = 1n;
  for (let i = 0; i < n; i++) {
    const t = b;
    b += a;
    a = t;
  }
  return a;
}

const jsSuite = {
  thunk: () => {},
  add: (a, b) => a + b,
  fibonacci: (n) => jsFib(n),
  get_person: () => ({
    name: 'Susize',
    age: 25,
    email: 'susize@example.com',
    scores: [95, 87, 92],
  }),
  get_scores: () => Uint32Array.from({ length: 1024 }, (_, i) => i),
};

// --- test definitions ------------------------------------------------------
// Each driver turns a suite's function into `(iters) => void`, calling it with
// the same arguments across every suite for a fair comparison.

// A sink to keep results live so the JIT can't elide the work — and, crucially,
// to *consume* fields. wasm_zero returns lazy zero-copy values, so a benchmark
// that discarded them would measure nothing; we read every field to force the
// same materialization the js / wasm_bindgen suites do up front.
let sink = 0;

const drivers = {
  thunk: (fn) => (n) => { for (let i = 0; i < n; i++) fn(); },
  add: (fn) => (n) => { for (let i = 0; i < n; i++) sink += fn(1, 2); },
  fibonacci: (fn) => (n) => { for (let i = 0; i < n; i++) sink += Number(fn(40)); },
  get_person: (fn) => (n) => {
    for (let i = 0; i < n; i++) {
      const p = fn();
      sink += p.age + p.name.length + (p.email ? p.email.length : 0);
      for (let j = 0; j < p.scores.length; j++) sink += p.scores[j];
    }
  },
  get_scores: (fn) => (n) => {
    for (let i = 0; i < n; i++) {
      const a = fn();
      for (let j = 0; j < a.length; j++) sink += a[j];
    }
  },
};
const TEST_IDS = Object.keys(drivers);

function buildSuite(name, fns) {
  const bms = {};
  for (const id of TEST_IDS) {
    if (typeof fns[id] === 'function') bms[id] = drivers[id](fns[id]);
  }
  return [name, bms];
}

// --- measurement -----------------------------------------------------------

const TARGET_MS = 40; // each timed run aims for roughly this long
const MAX_ITERS = 5e7; // hard cap so a slow suite can't run away

function timeOnce(bm, iters) {
  const t = performance.now();
  bm(iters);
  return performance.now() - t;
}

// Auto-scale iterations *per benchmark* to ~TARGET_MS, then report the best
// (lowest) nanoseconds-per-op over a few runs. Scaling per benchmark is what
// keeps a slow suite (wasm_zero) from inheriting a fast suite's huge iteration
// count and hanging the page.
function nsPerOp(bm) {
  let iters = 1;
  let ms = timeOnce(bm, iters);
  while (ms < TARGET_MS && iters < MAX_ITERS) {
    const factor = ms < 1 ? 8 : Math.max(2, (TARGET_MS / ms) * 1.5);
    iters = Math.min(MAX_ITERS, Math.ceil(iters * factor));
    ms = timeOnce(bm, iters);
  }
  let best = ms / iters;
  for (let i = 0; i < 3; i++) best = Math.min(best, timeOnce(bm, iters) / iters);
  return best * 1e6; // ms/op -> ns/op
}

const yieldToUi = () => new Promise((r) => setTimeout(r));

function fmtNs(ns) {
  if (ns >= 1000) return `${(ns / 1000).toFixed(2)} µs/op`;
  return `${ns.toFixed(1)} ns/op`;
}

async function run(suites, status, btn, tbody) {
  btn.disabled = true;
  tbody.innerHTML = '';

  // Warm up JITs / wasm with a few cheap iterations.
  for (const [, bms] of suites)
    for (const id of TEST_IDS) if (bms[id]) bms[id](20);

  for (const id of TEST_IDS) {
    status.textContent = `running ${id}…`;
    await yieldToUi();

    let base = null;
    const results = [];
    for (let s = 0; s < suites.length; s++) {
      const bm = suites[s][1][id];
      if (!bm) {
        results.push(null);
        continue;
      }
      await yieldToUi(); // let the "running …" label paint
      try {
        const ns = nsPerOp(bm);
        if (s === 0) base = ns;
        results.push(ns);
      } catch (e) {
        console.error(`${id} / ${suites[s][0]}`, e);
        results.push('err');
      }
    }

    const row = document.createElement('tr');
    row.innerHTML =
      `<td>${id}</td>` +
      results
        .map((ns, s) => {
          if (ns === null) return '<td>—</td>';
          if (ns === 'err') return '<td>error</td>';
          const ratio = base && s !== 0 ? ` (${(ns / base).toFixed(1)}×)` : '';
          return `<td>${fmtNs(ns)}${ratio}</td>`;
        })
        .join('');
    tbody.appendChild(row);
  }

  status.textContent = 'done.';
  btn.disabled = false;
  if (!Number.isFinite(sink)) console.log(sink); // keep `sink` live
}

// --- entry point -----------------------------------------------------------

// wasm-bindgen's `extern` imports resolve against these globals (matches the
// upstream benchmark). Defined here so the demo is self-contained.
function defineWbGlobals() {
  window.thunk = window.thunk ?? function () {};
  window.doesnt_throw = window.doesnt_throw ?? function () {};
  window.add = window.add ?? function (a, b) { return a + b; };
  window.Foo = window.Foo ?? class { bar() {} };
}

export async function initBenchmark({ runBtn, status, tbody }) {
  defineWbGlobals();
  status.textContent = 'loading wasm…';
  await initWb();
  const wz = await initWasmZero(WZ_WASM);

  // Sanity check decoding before benchmarking (surfaces rkyv-js problems early).
  console.log('wasm_zero.get_person() →', wz.get_person());
  console.log('wasm_zero.fibonacci(40) →', wz.fibonacci(40));
  console.log('wasm_zero.add(1, 2) →', wz.add(1, 2));

  const suites = [
    buildSuite('js', jsSuite),
    buildSuite('wasm_bindgen', wb),
    buildSuite('wasm_zero', wz),
  ];

  runBtn.disabled = false;
  runBtn.onclick = () =>
    run(suites, status, runBtn, tbody).catch((e) => {
      status.textContent = 'error: ' + e;
      console.error(e);
      runBtn.disabled = false;
    });
  status.textContent = 'ready — click “Run benchmarks”.';
}
