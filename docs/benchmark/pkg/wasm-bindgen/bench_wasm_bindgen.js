/* @ts-self-types="./bench_wasm_bindgen.d.ts" */

/**
 * @param {number} a
 * @param {number} b
 * @returns {number}
 */
function add2(a, b) {
    const ret = wasm.add(a, b);
    return ret;
}
export { add2 as add }

/**
 * @param {number} n
 */
export function call_doesnt_throw_n_times(n) {
    wasm.call_doesnt_throw_n_times(n);
}

/**
 * @param {number} n
 */
export function call_doesnt_throw_with_catch_n_times(n) {
    wasm.call_doesnt_throw_with_catch_n_times(n);
}

/**
 * @param {number} n
 * @param {any} element
 */
export function call_first_child_n_times(n, element) {
    try {
        wasm.call_first_child_n_times(n, addBorrowedObject(element));
    } finally {
        heap[stack_pointer++] = undefined;
    }
}

/**
 * @param {number} n
 * @param {any} element
 */
export function call_first_child_structural_n_times(n, element) {
    try {
        wasm.call_first_child_structural_n_times(n, addBorrowedObject(element));
    } finally {
        heap[stack_pointer++] = undefined;
    }
}

/**
 * @param {number} n
 * @param {any} foo
 */
export function call_foo_bar_n_times(n, foo) {
    try {
        wasm.call_foo_bar_n_times(n, addBorrowedObject(foo));
    } finally {
        heap[stack_pointer++] = undefined;
    }
}

/**
 * @param {number} n
 * @param {any} foo
 */
export function call_foo_bar_structural_n_times(n, foo) {
    try {
        wasm.call_foo_bar_structural_n_times(n, addBorrowedObject(foo));
    } finally {
        heap[stack_pointer++] = undefined;
    }
}

/**
 * @param {number} n
 * @param {number} a
 * @param {number} b
 */
export function call_js_add_n_times(n, a, b) {
    wasm.call_js_add_n_times(n, a, b);
}

/**
 * @param {number} n
 */
export function call_js_thunk_n_times(n) {
    wasm.call_js_thunk_n_times(n);
}

/**
 * @param {number} n
 * @param {any[]} elements
 */
export function call_node_first_child_n_times(n, elements) {
    const ptr0 = passArrayJsValueToWasm0(elements, wasm.__wbindgen_export2);
    const len0 = WASM_VECTOR_LEN;
    wasm.call_node_first_child_n_times(n, ptr0, len0);
}

/**
 * @param {number} n
 * @param {any[]} elements
 */
export function call_node_has_child_nodes_n_times(n, elements) {
    const ptr0 = passArrayJsValueToWasm0(elements, wasm.__wbindgen_export2);
    const len0 = WASM_VECTOR_LEN;
    wasm.call_node_has_child_nodes_n_times(n, ptr0, len0);
}

/**
 * @param {number} n
 * @param {any[]} elements
 */
export function call_node_node_type_n_times(n, elements) {
    const ptr0 = passArrayJsValueToWasm0(elements, wasm.__wbindgen_export2);
    const len0 = WASM_VECTOR_LEN;
    wasm.call_node_node_type_n_times(n, ptr0, len0);
}

/**
 * @param {Node} element
 */
export function count_node_types(element) {
    wasm.count_node_types(addHeapObject(element));
}

/**
 * @param {number} n
 * @returns {number}
 */
export function fibonacci(n) {
    const ret = wasm.fibonacci(n);
    return ret;
}

/**
 * @returns {number}
 */
export function fibonacci_high() {
    const ret = wasm.fibonacci_high();
    return ret;
}

/**
 * Matches `bench_wasm_zero::get_person`.
 * @returns {any}
 */
export function get_person() {
    const ret = wasm.get_person();
    return takeObject(ret);
}

/**
 * Matches `bench_wasm_zero::get_scores` (1024 u32s).
 * @returns {Uint32Array}
 */
export function get_scores() {
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        wasm.get_scores(retptr);
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        var v1 = getArrayU32FromWasm0(r0, r1).slice();
        wasm.__wbindgen_export3(r0, r1 * 4, 4);
        return v1;
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
    }
}

function thunk2() {
    wasm.thunk();
}
export { thunk2 as thunk }
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_rethrow_c4d99b4b53265290: function(arg0) {
            throw takeObject(arg0);
        },
        __wbg___wbindgen_throw_1506f2235d1bdba0: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_add_e6de262af5f48b1e: function(arg0, arg1) {
            const ret = add(arg0, arg1);
            return ret;
        },
        __wbg_bar_24ee589ae9ccccc0: function(arg0) {
            getObject(arg0).bar();
        },
        __wbg_bar_3370c92b88765132: function(arg0) {
            getObject(arg0).bar();
        },
        __wbg_doesnt_throw_9e2d2ea4afc54d7e: function() { return handleError(function () {
            doesnt_throw();
        }, arguments); },
        __wbg_doesnt_throw_c732200188d7e93b: function() {
            doesnt_throw();
        },
        __wbg_firstChild_67934ad130fd76ea: function(arg0) {
            const ret = getObject(arg0).firstChild;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_firstChild_b074ab013a0373a1: function(arg0) {
            const ret = getObject(arg0).firstChild;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_firstChild_d8bd92470fd7b5e7: function(arg0) {
            const ret = getObject(arg0).firstChild;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_hasChildNodes_78ec3cba596b8fa4: function(arg0) {
            const ret = getObject(arg0).hasChildNodes();
            return ret;
        },
        __wbg_new_ce1ab61c1c2b300d: function() {
            const ret = new Object();
            return addHeapObject(ret);
        },
        __wbg_new_d90091b82fdf5b91: function() {
            const ret = new Array();
            return addHeapObject(ret);
        },
        __wbg_nextSibling_f356004d1bf9863d: function(arg0) {
            const ret = getObject(arg0).nextSibling;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_nodeType_6fe8a61baba73939: function(arg0) {
            const ret = getObject(arg0).nodeType;
            return ret;
        },
        __wbg_set_6be42768c690e380: function(arg0, arg1, arg2) {
            getObject(arg0)[takeObject(arg1)] = takeObject(arg2);
        },
        __wbg_set_dca99999bba88a9a: function(arg0, arg1, arg2) {
            getObject(arg0)[arg1 >>> 0] = takeObject(arg2);
        },
        __wbg_thunk_c987dffa936d1c1a: function() {
            thunk();
        },
        __wbindgen_cast_0000000000000001: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return addHeapObject(ret);
        },
        __wbindgen_object_clone_ref: function(arg0) {
            const ret = getObject(arg0);
            return addHeapObject(ret);
        },
        __wbindgen_object_drop_ref: function(arg0) {
            takeObject(arg0);
        },
    };
    return {
        __proto__: null,
        "./bench_wasm_bindgen_bg.js": import0,
    };
}

function addHeapObject(obj) {
    if (heap_next === heap.length) heap.push(heap.length + 1);
    const idx = heap_next;
    heap_next = heap[idx];

    heap[idx] = obj;
    return idx;
}

function addBorrowedObject(obj) {
    if (stack_pointer == 1) throw new Error('out of js stack');
    heap[--stack_pointer] = obj;
    return stack_pointer;
}

function dropObject(idx) {
    if (idx < 1028) return;
    heap[idx] = heap_next;
    heap_next = idx;
}

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function getObject(idx) { return heap[idx]; }

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        wasm.__wbindgen_export(addHeapObject(e));
    }
}

let heap = new Array(1024).fill(undefined);
heap.push(undefined, null, true, false);

let heap_next = heap.length;

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    const mem = getDataViewMemory0();
    for (let i = 0; i < array.length; i++) {
        mem.setUint32(ptr + 4 * i, addHeapObject(array[i]), true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
}

let stack_pointer = 1024;

function takeObject(idx) {
    const ret = getObject(idx);
    dropObject(idx);
    return ret;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('bench_wasm_bindgen_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
