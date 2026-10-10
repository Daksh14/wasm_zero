# RFC 0001: `#[wasm_zero(component)] mod` for components and WASI

- Status: Draft
- Created: 2026-10-10
- Tracks: README § "Roadmap: WASI components"

## Summary

Add a second ABI to wasm_zero, the component model's **canonical ABI**, next
to today's **core ABI** (`malloc`/`free` + `[len][archive]` frames). Every
item uses exactly one of the two, and you can tell which from the source:
anything inside a `#[wasm_zero(component)] mod` uses the canonical ABI,
and everything else uses the core ABI, exactly as today (see "Two ABIs"
below).

Inside a `#[wasm_zero(component)]` module, `#[wasm_zero]` marks the items
that cross the component boundary:

- a `#[wasm_zero] fn` **with a body** is an **export**, and
- a `#[wasm_zero] fn` **without a body** (`fn print(msg: &str);`) is an
  **import** of your own world.

Bindings can come from two places. The macro treats both the same way:

1. **Your own WIT world**, in `wit/`. The macro wraps `wit_bindgen::generate!`
   for it.
2. **A bindings crate that already exists**: `wasip3`, `wasip2`, or a crate you
   generated once for a draft proposal such as `wasi:nn`, `wasi:webgpu`, or
   `wasi:keyvalue`. The macro calls the crate's `Guest` trait and its
   `export!` macro.

The central rule is **forward, don't interpret**. The macro reads only names
and syntax: the function's name, whether it has a body, whether it is
`async`, and its receiver. It copies your signatures *verbatim* into the
generated trait impls and lets rustc check them. It never resolves,
validates, or converts one of your types on its own initiative. That is why a
`wasip3::http::types::Request`, a `wit_stream::StreamReader<u8>`, a `wasi:nn`
`Tensor`, or a WebGPU buffer can appear in a `#[wasm_zero] fn` signature
unchanged: wasm_zero never has to understand any of them.

```wit
// wit/host.wit
package example:host;

world host {
  import print: func(msg: string);

  export run: func();
}
```

```rust
use wasm_zero::wasm_zero;

#[wasm_zero(component)]
mod host {
    #[wasm_zero]
    fn print(msg: &str); // import: no body

    #[wasm_zero]
    pub fn run() {
        // export: has a body
        print("hello from the guest");
    }

    fn helper() {} // not annotated: an ordinary Rust function
}
```

The same mechanism with an HTTP handler from the `wasip3` crate:

```rust
#[wasm_zero(component, export = wasip3::exports::http::handler, via = wasip3::http::service::export)]
mod handler {
    use wasip3::http::types::{ErrorCode, Request, Response};

    #[wasm_zero]
    pub async fn handle(request: Request) -> Result<Response, ErrorCode> {
        /* your program: wasip3 types in, wasip3 types out */
    }
}
```

## Motivation

The usual `wit-bindgen` guest needs a lot of boilerplate for a one-function
world:

```rust
wit_bindgen::generate!({ world: "host", path: "wit" });

struct Component;

impl Guest for Component {
    fn run() {
        print("hello from the guest");
    }
}

export!(Component);
```

The marker struct and the trait impl are pure ceremony. The real implementation
is a set of free functions, and the trait is how wit-bindgen gets a
monomorphisation point. The boilerplate grows with every exported interface,
every resource (`type Foo = MyFoo;`), and every world that `include`s another.
It is the same when the bindings come from a crate. The `wasip3` HTTP example
also needs `struct Example;`, `impl wasip3::exports::http::handler::Guest for
Example`, and `wasip3::http::service::export!(Example);` around a single
`async fn`.

wasm_zero already uses a single attribute as the source of truth for an item:
`#[wasm_zero] fn foo` means "foo crosses the boundary" (see "How it works" in
the README). This RFC keeps that meaning for components, removes the
ceremony, and stays out of the types. The WASI ecosystem already maintains
bindings crates, and wasm_zero shouldn't compete with them or need an update
every time a proposal changes.

## Two ABIs

wasm_zero has two ABIs. This section is normative. Everything later in this
RFC is about the canonical ABI, unless it says otherwise.

### Which ABI an item uses

The ABI is decided by **where the item is written**. It never depends on the
compilation target or on the item's types:

| Where the `#[wasm_zero]` item is                         | ABI                         |
| -------------------------------------------------------- | --------------------------- |
| a `fn` that is **not** inside a `#[wasm_zero(component)] mod` | **core ABI** (today's behaviour, unchanged) |
| anything inside a `#[wasm_zero(component …)] mod`, at any nesting depth | **canonical ABI** |
| a `struct` (`#[wasm_zero] #[derive(Archive)]`)           | neither: it is data. It gets a reader and can be used by both ABIs |

Rules:

1. **The boundary is explicit.** A bare `#[wasm_zero]` on a `mod` is a
   `compile_error!` ("say `#[wasm_zero(component)]`"). The keyword is only
   needed on the outermost module. Nested modules (`import = …`,
   `export = …`) inherit it.
2. **One item, one ABI.** Inside a component module, `#[wasm_zero]` can't ask
   for the core ABI, and `#[wasm_zero(core)]` there is an error. To expose
   the same logic both ways, write it once as a plain function and call it
   from two annotated wrappers (see "Exposing one function through both ABIs").
3. **No automatic bridging.** A core-ABI shim never becomes a component
   export, and a component export never gets a `__wasm_zero_*` shim.
   Putting an existing core module inside a component is a separate,
   explicit build step (see "Open questions").

### What each ABI means

|                       | Core ABI                                                      | Canonical ABI                                                |
| --------------------- | ------------------------------------------------------------- | ------------------------------------------------------------ |
| Declared by           | `#[wasm_zero] fn` outside component modules                   | items inside `#[wasm_zero(component …)] mod`                 |
| Artifact              | core wasm module                                              | component (built as `wasm32-wasip2`)                         |
| Typical host          | browser / exu / Node, through the generated `bindings.js`     | Wasmtime, Spin, jco, `wasmtime serve`, …                     |
| Exported symbols      | `__wasm_zero_<fn>` + the allocator (`malloc`/`free`)          | WIT names, lifted by `canon lift`. wit-bindgen supplies `cabi_realloc` |
| Imports               | none                                                          | WIT imports (own world, or bindings crates)                  |
| Scalars               | direct wasm params and returns                                | direct (canonical flattening)                                |
| Non-scalar values     | **always** rkyv: `[len: u32][pad to 16][archive]` in guest memory | whatever the WIT type maps to (pass-through). rkyv **only** with `#[archive]` / `#[wasm_zero(archive)]` |
| Who allocates         | the host calls the guest's `malloc`. Results go into a frame at `out_ptr` | the canonical ABI: the host calls `cabi_realloc` for args, and the guest returns pointers for results |
| Allowed types         | scalars, or owned rkyv-archivable types                       | anything the `Guest` trait signature accepts                 |
| Errors                | `ErrorCode` return value                                      | whatever the WIT says (`result<…>`). An invalid archive traps unless taken as `Result<Archived<T>, _>` |
| How the host reads results | in place, in guest memory (0 copies)                     | the host copies the value out (1 copy), then reads the archive in place |
| Bindings              | `wasm_zero_build` → `bindings.{js,ts}`                        | the host's component bindings, plus `wasm_zero_build` typed views for `#[archive]` values |

**The archive bytes are identical in both ABIs. Only the framing differs.**
With the core ABI, the archive sits in guest memory behind a 16-byte header
that holds the length. With the canonical ABI, an archive is a `list<u8>`, the
length travels in the canonical `(ptr, len)` pair, and there is no header.
So the generated `decode_Person` reads either: from a frame in wasm memory,
or from the `Uint8Array` a component host returns.

### Exposing one function through both ABIs

```rust
#[wasm_zero]
#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct Person { name: String, age: u32 }

fn load_person() -> Person { /* … */ } // plain Rust, used by both ABIs

#[wasm_zero]                           // core ABI: __wasm_zero_get_person(out_ptr)
pub fn get_person() -> Person {
    load_person()
}

#[wasm_zero(component)]                // canonical ABI: export get-person: func() -> list<u8>
mod api {
    #[wasm_zero(archive)]
    pub fn get_person() -> super::Person {
        super::load_person()
    }
}
```

Build it as a core module (`wasm32-unknown-unknown`) for the browser and as a
component (`wasm32-wasip2`) for component hosts. Each build emits both kinds
of items, so gate the side you don't need:

- **Core-ABI items in a component build** are dead weight. wit-component
  ignores core exports that aren't part of the world (it logs "unknown
  export"), so they are never reachable. Gate them with
  `#[cfg(not(target_env = "p2"))]`.
- **The core ABI's allocator on WASI targets.** On `target_os = "wasi"`, Rust's
  std links wasi-libc, which already defines `malloc` and `free`. There, the
  core ABI exports its allocator as `__wasm_zero_malloc` /
  `__wasm_zero_free` instead. It records the names in the `__wasm_zero`
  metadata section, so the generated bindings use whichever names the module
  has. On other targets the names stay `malloc` / `free`, so nothing changes
  for existing users.
- **Component modules in a core build** need wit-bindgen and WIT imports
  that a browser can't provide. Gate them with
  `#[cfg(target_env = "p2")]` (or `"p3"` once that target exists).

## Guide-level explanation

### Forward, don't interpret

Every export produces a forwarder in a trait impl. That forwarder is your
signature copied token for token, with a body that calls your function:

```rust
// you wrote
#[wasm_zero]
pub async fn handle(request: Request) -> Result<Response, ErrorCode> { /* … */ }

// the macro emits, in the same module (so `Request` etc. resolve identically)
impl wasip3::exports::http::handler::Guest for __WasmZeroComponent {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        self::handle(request).await
    }
}
```

The macro uses only syntax:

| The macro looks at           | Used for                                                  |
| ---------------------------- | --------------------------------------------------------- |
| the function name            | the trait method name, matched by rustc                   |
| body vs no body              | export vs import                                          |
| `async`                      | copied into the forwarder, which adds `.await`            |
| the receiver (`&self`, none) | resource methods vs free and static functions             |
| parameter patterns           | turned into argument names for the call                   |
| `#[archive]` / `archive`     | the explicit opt-in conversion (see "Archived payloads")  |

Parameter and return **types** are never inspected. If a type is wrong, the
error comes from rustc (see "Diagnostics"). Each forwarder is emitted *in the
same module* as the function it forwards to, so your `use` items resolve in
the forwarder exactly as they do in your function.

### The world module

```rust
    component,                      // required: this module and everything in it use the canonical ABI
#[wasm_zero(
    world = "host",                 // own WIT world. Default: the module ident, kebab-cased
    path = "wit",                   // default: "wit" (relative to CARGO_MANIFEST_DIR)
    with("wasi:http/types@0.3.0" = wasip3::http::types), // passed to wit-bindgen verbatim
    runtime = wasip3::wit_bindgen,  // which wit-bindgen runtime the generated code uses
    extends(super::base),           // optional, see "Extending worlds"
    root = true,                    // default: true. Whether this module emits the export macro call
)]
mod host { /* … */ }
```

`component` is required: it is what selects the canonical ABI (see "Two
ABIs"). With no other arguments, `#[wasm_zero(component)] mod host {}`
resolves to world `host` in `./wit`. A module that only uses a bindings crate (`export = path`,
see below) doesn't need a world or a `wit/` directory.

An outer attribute on an inline `mod` (`mod host { … }`) is stable Rust. An
attribute on an out-of-line `mod host;` is not stable (`proc_macro_hygiene`),
so the world module has to be inline. Implementations can still live in other
files; see "Delegating to other paths".

The outer attribute expands **first** and receives the module's tokens
unexpanded, including the inner `#[wasm_zero]` attributes. It interprets those
inner attributes and removes them, so they never expand separately. The macro
recognizes any attribute path whose last segment is `wasm_zero`. A
`#[wasm_zero] fn` *outside* any component module is never seen by this
macro. It expands on its own into a core-ABI shim, as it does today.

### Exports

Mark the function with `#[wasm_zero]` and give it a body. The trait method
name comes from the Rust name. Use `name = "…"` when your Rust name differs
from the trait method:

```rust
#[wasm_zero]
pub fn run() { /* … */ }                       // export run

#[wasm_zero(name = "get-person")]
pub fn fetch_person() -> Vec<u8> { /* … */ }   // export get-person (trait method `get_person`)
```

Exported functions stay normal Rust functions. They can be called from other
code and unit-tested on the host target, because the export macro call is
`cfg`'d to `target_arch = "wasm32"`.

### Imports

#### From your own world: bodyless declarations

```rust
#[wasm_zero]
fn print(msg: &str);           // world-level import `print`

#[wasm_zero(name = "print")]
pub(crate) fn log(msg: &str);  // same import, under a different Rust name and visibility
```

The macro replaces each declaration with a wrapper that has your signature,
copied verbatim, and a body that calls the generated binding:

```rust
#[inline(always)]
fn print(msg: &str) {
    self::wit::print(msg)
}
```

A bodyless `fn` at module level isn't legal Rust after expansion. It is fine
as macro *input*, though: it parses as an item, and rustc only reports "free
function without a body" in AST validation, which runs after the macro has
replaced it.

**Declaring imports is opt-in, not required.** The full wit-bindgen output
lives in a generated child module called `wit`. Any import you didn't declare
can still be reached as `wit::print(..)`, and the generated WIT types live
there too. Declaring an import gives you a name and visibility of your
choosing, makes signature drift fail at the declaration, and allows the
`#[archive]` opt-in.

An imported interface from your own world is a nested module. Every bodyless
`fn` inside it is an import of that interface:

```rust
#[wasm_zero(import = "example:host/logging")]
mod logging {
    pub fn log(level: u8, msg: &str);
}
```

#### From a bindings crate: plain `use`

Imports that a crate already provides need no macro support. They are just
Rust:

```rust
#[wasm_zero(component)]
mod app {
    use wasip3::clocks::monotonic_clock;
    use wasip3::random::random::get_random_u64;

    #[wasm_zero]
    pub fn roll() -> u64 {
        let _t = monotonic_clock::now();
        get_random_u64() % 6 + 1
    }
}
```

The crate's bindings record the WIT import in their own `component-type`
custom section, and `wit-component` merges it into the final component like
any other.

### Exports from a bindings crate

`export =` takes either form:

| `export =`                          | Means                                                                      |
| ----------------------------------- | -------------------------------------------------------------------------- |
| `"ns:pkg/iface@v"` (string literal) | an interface exported by **your own world**. Its `Guest` trait is in `wit::exports::…` |
| `some::path` (Rust path)            | a **prebuilt bindings module** containing a `Guest` trait (`wasip3::exports::http::handler`) |

With a path, `via = some::export` names the crate's export macro. The macro
calls `some::export!(__WasmZeroComponent)` instead of the `wit::export!` it
would generate for an own world. (The `wasip3` export macros take a plain
identifier, which is why the component struct is always a local ident.)

```rust
#[wasm_zero(component, export = wasip3::exports::cli::run, via = wasip3::cli::command::export)]
mod cli {
    #[wasm_zero]
    pub async fn run() -> Result<(), ()> {
        Ok(())
    }
}
```

Expansion:

```rust
mod cli {
    pub async fn run() -> Result<(), ()> {
        Ok(())
    }

    #[doc(hidden)]
    pub struct __WasmZeroComponent;

    impl wasip3::exports::cli::run::Guest for __WasmZeroComponent {
        async fn run() -> Result<(), ()> {
            self::run().await
        }
    }

    #[doc(hidden)]
    pub const __WASM_ZERO_EXPORTS: bool = true;

    #[cfg(target_arch = "wasm32")]
    wasip3::cli::command::export!(__WasmZeroComponent);
}
```

A crate-backed export module can sit at crate level, as shown, or inside an
own-world module next to own-world exports. In the nested case, the inner
module emits `use super::__WasmZeroComponent;`, and its impls and its `via`
call target the outer module's struct.

### Expansion (own world)

For the `host` example at the top:

```rust
mod host {
    /// Everything wit-bindgen generates for world `host`.
    #[doc(hidden)]
    pub mod wit {
        ::wasm_zero::__private::wit_bindgen::generate!({
            world: "host",
            path: "wit", // made absolute against CARGO_MANIFEST_DIR by the macro
            runtime_path: "::wasm_zero::__private::wit_bindgen::rt", // or from `runtime = …`
            pub_export_macro: true,
            // with: { … }, copied from `with(…)` unchanged
        });
    }

    #[inline(always)]
    fn print(msg: &str) {
        self::wit::print(msg)
    }

    pub fn run() {
        print("hello from the guest");
    }

    fn helper() {}

    #[doc(hidden)]
    pub struct __WasmZeroComponent;

    impl self::wit::Guest for __WasmZeroComponent {
        #[inline(always)]
        fn run() {
            self::run()
        }
    }

    #[doc(hidden)]
    pub const __WASM_ZERO_EXPORTS: bool = true;

    #[cfg(target_arch = "wasm32")]
    self::wit::export!(__WasmZeroComponent with_types_in self::wit);
}
```

Notes:

- `self::run()` inside the impl refers to the module-level `fn run`. An
  associated function needs `Self::` to call itself, so there is no recursion.
- The forwarders and the import wrappers are built with `quote_spanned!` at
  your declaration, so rustc's errors point at your code.
- **No implicit conversions.** wit-bindgen gives an exported `string` param as
  `String` and an imported one as `&str`. You write what the trait or the
  binding expects. The macro doesn't insert `&` or `.into()`.

### Interfaces and resources

```wit
interface greeter {
  resource counter {
    constructor(start: u32);
    incr: func() -> u32;
  }
  greet: func(name: string) -> string;
}

world host {
  export greeter;
}
```

```rust
#[wasm_zero(component)]
mod host {
    #[wasm_zero(export = "example:host/greeter")]
    pub mod greeter {
        use core::cell::Cell;

        #[wasm_zero]
        pub fn greet(name: String) -> String {
            format!("hello, {name}")
        }

        #[wasm_zero(resource)]
        pub struct Counter(Cell<u32>);

        #[wasm_zero]
        impl Counter {
            pub fn new(start: u32) -> Self {   // constructor
                Counter(Cell::new(start))
            }

            pub fn incr(&self) -> u32 {        // method (it has a receiver)
                let n = self.0.get() + 1;
                self.0.set(n);
                n
            }
        }
    }
}
```

The macro emits the following inside `mod greeter`, so that types resolve
there:

```rust
impl super::wit::exports::example::host::greeter::Guest for super::__WasmZeroComponent {
    type Counter = Counter;

    fn greet(name: String) -> String {
        self::greet(name)
    }
}

impl super::wit::exports::example::host::greeter::GuestCounter for Counter {
    fn new(start: u32) -> Self {
        Counter::new(start)
    }

    fn incr(&self) -> u32 {
        Counter::incr(self)
    }
}
```

`#[wasm_zero(resource)]` is spelled out because a bare `#[wasm_zero] struct`
already means "rkyv-archived struct with a generated reader".
wit-bindgen gives resource methods `&self`, so mutable state needs interior
mutability. If you write `&mut self`, rustc rejects the copied signature
(see "Diagnostics").

### Direction rules at a glance

| Item inside a world module                                          | Meaning                                    |
| ------------------------------------------------------------------- | ------------------------------------------ |
| `#[wasm_zero] fn f(..) { body }`                                    | export `f`                                 |
| `#[wasm_zero] async fn f(..) { body }`                              | async export (WASI 0.3 `async func`)       |
| `#[wasm_zero] fn f(..);`                                            | import `f` from your own world             |
| `#[wasm_zero(import = "ns:pkg/iface@v")] mod m { fn f(..); }`       | import interface from your own world       |
| `#[wasm_zero(export = "ns:pkg/iface@v")] mod m { … }`               | export interface from your own world       |
| `#[wasm_zero(export = path, via = path)] mod m { … }`               | export through a bindings crate            |
| `#[wasm_zero(resource)] struct T` + `#[wasm_zero] impl T`           | exported resource                          |
| `#[wasm_zero] struct T` (with `#[derive(Archive)]`)                 | archived payload type (existing meaning)   |
| `use some_crate::…;`                                                | crate imports: plain Rust, untouched       |
| anything without `#[wasm_zero]`                                     | plain Rust, untouched                      |

### Extending worlds

WIT composes worlds with `include`:

```wit
world app {
  include host;
  export greet: func(name: string) -> string;
}
```

You can implement every export in place, or reuse an existing world module
with `extends`:

```rust
#[wasm_zero(component, root = false)] // a library piece: doesn't emit `export!`
mod host {
    #[wasm_zero]
    fn print(msg: &str);

    #[wasm_zero]
    pub fn run() {
        print("hello from host");
    }
}

#[wasm_zero(component, extends(super::host))]
mod app {
    #[wasm_zero]
    pub fn greet(name: String) -> String {
        format!("hello {name}")
    }
    // no `run` here, so it is delegated to `super::host::run`
}
```

A delegated export has no local function whose signature the macro could
copy. For this case only, the macro derives the forwarder's signature from
the WIT. It parses the world with `wit-parser`, sees that `run` comes from
`include host`, and prints the Rust signature with wit-bindgen's own type
mapping. It names types through `self::wit::…`, or through the `with(…)`
path when the interface is mapped. That is WIT-to-Rust printing, the same
work `generate!` does. Your types are still never inspected, and if the
result disagrees with `host::run`, rustc reports the call. The same
applies to `pub use` exports (below). Both are limited to own worlds.

```rust
const _: () = assert!(
    !super::host::__WASM_ZERO_EXPORTS,
    "`super::host` is used in `extends(..)`; mark it `#[wasm_zero(component, root = false)]`",
);
```

This turns a missing `root = false` into a clear compile error instead
of a duplicate-symbol error from the linker.

### Delegating to other paths

```rust
mod impls; // regular out-of-line module

#[wasm_zero(component)]
mod host {
    #[wasm_zero]
    pub use super::impls::run; // satisfies `export run`
}
```

For an own world, the forwarder's signature is derived from the WIT, as for
`extends`. A crate-backed module has no WIT to derive from, so `pub use` is a
`compile_error!` there. The error asks for a one-line wrapper `fn` instead.

### Archived payloads (explicit opt-in)

This section is about the canonical ABI only. With the core ABI, every
non-scalar value is already an archive and no opt-in exists.

A WIT `list<u8>` can carry an rkyv archive. Following the rule above, wasm_zero
doesn't convert to or from a `list<u8>` because it *sees* a struct type. You
opt in with `#[archive]` on the parameter, or `#[wasm_zero(archive)]` on the
function for the return value. The macro then writes `Vec<u8>` in that position
of the trait signature (or `&[u8]` for an import parameter). It is the one
type the macro ever writes, chosen by your annotation and not by inference:

```rust
#[wasm_zero]
#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct Person {
    name: String,
    age: u32,
}

#[wasm_zero(component)]
mod people {
    use super::Person;
    use wasm_zero::Archived;

    #[wasm_zero]
    fn store(#[archive] person: &Person);           // import: store(person: list<u8>)

    #[wasm_zero(archive)]
    pub fn get_person() -> Person {                 // export: get-person() -> list<u8>
        Person { name: "Ada".into(), age: 36 }
    }

    #[wasm_zero]
    pub fn put_person(#[archive] p: Archived<Person>) { // export: put-person(p: list<u8>)
        store(&p.deserialize());
    }
}

// generated, roughly:
// fn store(person: &Person) {
//     self::wit::store(&::wasm_zero::__private::to_archive(person))
// }
// fn get_person() -> Vec<u8> {
//     ::wasm_zero::__private::to_archive(&self::get_person())
// }
// fn put_person(p: Vec<u8>) {
//     self::put_person(::wasm_zero::FromArchiveBytes::from_archive_bytes(p))
// }
```

The conversion goes through `to_archive` and the `FromArchiveBytes` trait,
which rustc resolves. The macro still never looks at `Person`.
`FromArchiveBytes` is implemented for `Archived<T>` (an owned,
bytecheck-validated buffer that traps when validation fails) and for
`Result<Archived<T>, ArchiveError>` (when you want to handle the failure
yourself). Parameter attributes are stable Rust syntax, and the outer macro
removes `#[archive]` before rustc sees it.

For each opt-in, the macro also writes a `__wasm_zero` metadata record. That
lets `wasm_zero_build` generate typed views for the host side (jco), which
read the bytes in place.

### Diagnostics

Because the macro copies signatures instead of checking them, type errors
come from rustc. They are reported at your span:

| Mistake                                              | Error you get                                                    |
| ---------------------------------------------------- | ---------------------------------------------------------------- |
| wrong parameter or return type                       | E0053 "method `handle` has an incompatible type for trait", with expected and found types |
| `fn` where the trait has `async fn`                  | "method `handle` should be async because the method from the trait is async" |
| `&mut self` on a resource method                     | E0053 "types differ in mutability"                               |
| export name the trait doesn't have                   | E0407 "method `hnadle` is not a member of trait `Guest`"         |
| export missing                                       | E0046 "not all trait items implemented, missing: `handle`", at the module |
| the same WIT type from your `wit` and from a crate   | E0308 "expected `wasip3::…::Request`, found `wit::…::Request`". Fix: add `with(…)` |
| mismatched import declaration                        | E0308 inside the generated wrapper, at your declaration          |

The macro adds its own `compile_error!` only for problems that are purely
syntactic: a bodyless `fn` in an export module, `#[archive]` on a
receiver, `pub use` in a crate-backed module, `export = path` without
`via`, and the "is it an import or an export" rules. Name-level checks against
the WIT for own worlds (for example, "world `host` exports no `rnu`; did you
mean `run`?") are a later improvement. They only use names, so they don't break
the rule.

## WASI and other proposals

None of the following needs code in wasm_zero that is specific to a proposal.
Each subsection shows how the general mechanisms apply.

### Choosing the binding source

| Proposal status                                  | Bindings come from                         | Imports                     | Exports                                  |
| ------------------------------------------------ | ------------------------------------------ | --------------------------- | ---------------------------------------- |
| Phase 3, WASI 0.2 (clocks, random, filesystem, sockets, cli, http) | `wasip2` crate              | `use wasip2::…`             | `export = wasip2::exports::…`, `via = …::export` |
| Phase 3, WASI 0.3 (same set, async)              | `wasip3` crate                             | `use wasip3::…`             | `export = wasip3::exports::…`, `via = …::export` |
| Phase 2 and earlier drafts (keyvalue, nn, webgpu, runtime-config, messaging, i2c, …) | your WIT: vendor it into `wit/deps` (`wkg wit fetch`), or a community crate if one exists | `wit::…`, or bodyless declarations | `export = "wasi:…"` in your world |
| Legacy witx bindings (the `wasi-nn` 0.6 crate)   | core-module imports (not components)       | call it from a core `#[wasm_zero] fn` | n/a                            |

Two practical rules:

1. **Share types with `with`.** If your own world `include`s or `use`s an
   interface that a crate already binds (for example `wasi:http/types`),
   map it: `with("wasi:http/types@0.3.0" = wasip3::http::types)`.
   Otherwise your `wit` module gets its own copy of `Request`, and passing it
   to `wasip3` code fails with E0308. wasm_zero passes `with` through to
   wit-bindgen without changes and doesn't add mappings of its own.
2. **Use one async runtime.** WASI 0.3 bindings rely on wit-bindgen's async
   runtime (`wit_future`, `wit_stream`, the task executor). Point your own
   world at the same runtime with `runtime = wasip3::wit_bindgen`, so that
   futures and streams from your `wit` and from `wasip3` belong to the same
   executor. If the wit-bindgen versions in the dependency graph are
   semver-compatible, Cargo unifies them anyway. `runtime =` is for cases
   where they aren't.

`wasip3` is `#![no_std]` with a default `std` feature. Use it with
`default-features = false` to keep a wasm_zero crate on `alloc` only.
Components are built with the `wasm32-wasip2` target until a `wasm32-wasip3`
target exists, as the `wasip3` crate documents.

### HTTP (`wasip3`)

In WASI 0.3 the handler is `wasi:http/handler.handle: async func(request) ->
result<response, error-code>`, and request and response bodies are
component-model `stream<u8>`s instead of `wasi:io` resources:

```rust
#[wasm_zero(component, export = wasip3::exports::http::handler, via = wasip3::http::service::export)]
mod handler {
    use super::Person;
    use wasip3::http::types::{ErrorCode, Fields, Request, Response};
    use wasip3::{wit_bindgen, wit_future, wit_stream};

    #[wasm_zero]
    pub async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let body = wasm_zero::to_archive(&Person { name: "Ada".into(), age: 36 });

        let (mut tx, rx) = wit_stream::new();
        let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
        let (response, _sent) = Response::new(Fields::new(), Some(rx), trailers_rx);
        drop(trailers_tx);

        wit_bindgen::spawn_local(async move {
            let rest = tx.write_all(body).await;
            debug_assert!(rest.is_empty());
        });
        Ok(response)
    }
}
```

wasm_zero contributes only the removed ceremony. The body is ordinary
`wasip3` code, and the archive is just bytes you choose to write. The browser
side can read that response with the generated `decode_Person`, using the
same reader as the core-module path.

To use it next to your own world, put the crate-backed module inside your
world module. Keep the HTTP export *out* of your WIT: the `wasip3` macro
already exports it, and if your world exported it as well, `wit::export!`
would also require an implementation of `wit::exports::wasi::http::handler`.

```rust
#[wasm_zero(component, world = "api", runtime = wasip3::wit_bindgen)] // world api { import example:api/audit; }
mod api {
    #[wasm_zero(import = "example:api/audit")]
    mod audit {
        pub fn record(path: &str);
    }

    #[wasm_zero(export = wasip3::exports::http::handler, via = wasip3::http::service::export)]
    mod handler {
        use wasip3::http::types::{ErrorCode, Request, Response};

        #[wasm_zero]
        pub async fn handle(request: Request) -> Result<Response, ErrorCode> {
            super::audit::record(&request.get_path_with_query().unwrap_or_default());
            todo!()
        }
    }
}
```

If your WIT *does* use `wasi:http` types (for example an `audit` function that
takes a `method`), add `with("wasi:http/types@0.3.0" = wasip3::http::types)`.

On WASI 0.2 the shape is the same with the `wasip2` crate: a synchronous
`handle(request: IncomingRequest, response_out: ResponseOutparam)` and
`wasi:io` streams. wasm_zero doesn't care which version you use.

### Streams and `wasi:io`

WASI 0.2 streams and pollables (`wasi:io/streams`, `wasi:io/poll`) come from
the `wasip2` crate. WASI 0.3 drops `wasi:io` in favour of the component
model's built-in `stream<T>` / `future<T>`, which `wasip3` exposes as
`wit_stream` / `wit_future`. Both kinds pass through `#[wasm_zero]`
signatures unchanged, like any other type.

Archive framing over a stream (the `[len: u32 LE][bytes]` frame the
core-module ABI already writes at `out_ptr`) would be a small helper library
on top of these types. It isn't part of the macro. It is worth noting for
later: in WASI 0.3, `stream.read` writes into a buffer that the *guest*
provides. A framing reader can therefore read the 4-byte length, allocate
one 16-byte-aligned frame, and have the archive written straight into it. For
streamed data, that is the guest-controlled placement that lazy lowering
(component-model#383) would give to plain parameters.

### `wasi:nn` (machine learning)

[`wasi:nn@0.2.0-rc-2024-10-28`](https://github.com/WebAssembly/wasi-nn) is a
phase 2, imports-only world (`ml`) with `tensor`, `graph`, `inference`, and
`errors` interfaces. Tensor data is a plain `list<u8>`.

**As a component.** No pre-generated crate is assumed. Vendor the WIT into
`wit/deps` and include it in your world. Its bindings then appear under your
`wit` module:

```wit
// wit/classifier.wit
package example:classifier;

world classifier {
  include wasi:nn/ml@0.2.0-rc-2024-10-28;
  export classify: func(image: list<u8>) -> result<list<f32>, string>;
}
```

```rust
#[wasm_zero(component, world = "classifier")]
mod classifier {
    use super::Image; // #[wasm_zero] #[derive(Archive)] struct Image { pixels: Vec<u8>, .. }
    use self::wit::wasi::nn::{graph, tensor::{Tensor, TensorType}};
    use wasm_zero::Archived;

    #[wasm_zero]
    pub fn classify(#[archive] image: Archived<Image>) -> Result<Vec<f32>, String> {
        let g = graph::load_by_name("mobilenet").map_err(|e| e.data())?;
        let ctx = g.init_execution_context().map_err(|e| e.data())?;
        let input = Tensor::new(&[1, 3, 224, 224], TensorType::Fp32, image.pixels.as_slice());
        let outputs = ctx.compute(vec![("input".into(), input)]).map_err(|e| e.data())?;
        let bytes = outputs[0].1.data();
        Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect())
    }
}
```

The `wasi:nn` types are wit-bindgen's own. The exact Rust signatures (borrowed
vs owned lists, how the enum cases are spelled) are whatever the generated
`wit` module says, and rustc enforces them. `image.pixels.as_slice()` reads
straight from the validated archive, so the pixel bytes are never copied
before they reach the tensor.

**As a core module.** The `wasi-nn` 0.6 crate binds the older witx ABI
(`wasi_ephemeral_nn` core imports), not the component model. Those are plain
core imports, so you can call it from a regular crate-level `#[wasm_zero] fn`
on the core-module path. That needs a host that provides `wasi_ephemeral_nn`,
such as Wasmtime with wasi-nn enabled. Browsers don't provide it.

### `wasi:webgpu`

[`wasi:webgpu@0.3.0-rc.2`](https://github.com/WebAssembly/wasi-webgpu) is a
phase 2, single-interface (`webgpu`) mirror of the WebGPU API: `get-gpu`,
`async` `request-adapter` / `request-device`, and explicit copy operations
such as `queue.write-buffer-with-copy(buffer, offset, data: list<u8>, …)` and
`buffer.get-mapped-range-get-with-copy`. It uses `async func`, so it is a
WASI 0.3-era API. Vendor its WIT (or use a crate from the wasi-gfx project if
one fits) and include it in your world. Exports that drive it are simply
`async`:

```rust
#[wasm_zero(component, world = "renderer", runtime = wasip3::wit_bindgen)]
// world renderer { import wasi:webgpu/webgpu@0.3.0-rc.2; export draw: async func(mesh: list<u8>); }
mod renderer {
    use super::Mesh; // #[wasm_zero] #[derive(Archive)] struct Mesh { vertices: Vec<f32>, .. }
    use self::wit::wasi::webgpu::webgpu as gpu;
    use wasm_zero::Archived;

    #[wasm_zero]
    pub async fn draw(#[archive] mesh: Archived<Mesh>) {
        let adapter = gpu::get_gpu().request_adapter(None).await.expect("no adapter");
        let device = adapter.request_device(None).await.expect("no device");

        let v = mesh.vertices.as_slice(); // &[f32_le], contiguous little-endian f32s
        // SAFETY: f32_le is 4 bytes, no padding, and the slice borrows `mesh`.
        let vertex_bytes: &[u8] = unsafe { core::slice::from_raw_parts(v.as_ptr().cast(), v.len() * 4) };
        // create a buffer, then device.queue().write_buffer_with_copy(&buf, 0, vertex_bytes, None, None) …
    }
}
```

An archived `Vec<f32>` is a contiguous little-endian `f32` array, which is
what a vertex buffer wants. The data goes from the archive to the GPU upload
with no deserialization. (The byte view is your code, not the macro's.)

Running `wasi:webgpu` in a browser needs a host shim from the component to
`navigator.gpu`, for example through jco. That is outside wasm_zero. The
browser-native path stays the core-module ABI plus JS.

### `wasi:keyvalue`

[`wasi:keyvalue@0.2.0-draft2`](https://github.com/WebAssembly/wasi-keyvalue)
(phase 2) stores values as `list<u8>`: `store` (`open`,
`bucket.get/set/delete/exists/list-keys`), `atomics` (`increment`,
`cas`/`swap`), `batch` (`get-many`/`set-many`/`delete-many`), and an
exported `watcher` (`on-set`/`on-delete`) in world `watch-service`. Those
opaque bytes are a good fit for archives, but storing them remains your code:

```rust
#[wasm_zero(component, world = "indexer")] // world indexer { include wasi:keyvalue/watch-service@0.2.0-draft2; }
mod indexer {
    use super::Person;
    use self::wit::wasi::keyvalue::store::{self, Bucket};
    use wasm_zero::{ArchiveError, Archived};

    pub fn save(id: u32, p: &Person) -> Result<(), store::Error> {
        store::open("people")?.set(&id.to_string(), &wasm_zero::to_archive(p))
    }

    #[wasm_zero(export = "wasi:keyvalue/watcher@0.2.0-draft2")]
    mod watcher {
        use super::*;

        #[wasm_zero]
        pub fn on_set(_bucket: Bucket, key: String, #[archive] value: Result<Archived<Person>, ArchiveError>) {
            match value {
                Ok(person) => { /* index person.name, read in place */ }
                Err(_) => { /* not a Person archive (e.g. an older layout); skip `key` */ }
            }
        }

        #[wasm_zero]
        pub fn on_delete(_bucket: Bucket, _key: String) {}
    }
}
```

rkyv archives don't describe their own layout, and values in a store outlive
the code that wrote them. If you store archives, version the key or the value
yourself. Taking `Result<Archived<T>, ArchiveError>`, as above, is how a
watcher survives old layouts.

### Future proposals

The [WASI proposals list](https://github.com/WebAssembly/WASI/blob/main/docs/Proposals.md)
keeps growing: runtime-config, messaging, i2c, blob-store, sql, tls, otel,
and others. Every one of them is WIT, sometimes with a crate. The recipe
doesn't change:

1. Get the WIT: `wkg wit fetch` into `wit/deps`, or a crate that ships it.
2. Imports: `use` the crate, or `include` / `import` the interface in your
   world and use `wit::…` (optionally with bodyless declarations).
3. Exports: `export = crate::path` + `via = …` for crate bindings, or
   `export = "ns:pkg/iface@v"` for your world.
4. Shared types: `with(…)`. Async: one `runtime = …`.
5. Archives: `#[archive]` / `#[wasm_zero(archive)]` wherever a `list<u8>` is
   meant to carry one.

wasm_zero doesn't keep a list of proposals and has no feature flag for any of
them. A new proposal, or a breaking change in a draft, needs a WIT or crate
update in your project and nothing in wasm_zero.

## Reference-level explanation

### Macro pipeline

1. Parse `attr`. Without `component`, emit the "say `#[wasm_zero(component)]`"
   error. Otherwise parse `{ world?, path, with: TokenStream, runtime: Path?,
   extends: Vec<Path>, root: bool, export?: (Lit | Path), via?: Path }`.
2. If there is a `world`, resolve `path` against `CARGO_MANIFEST_DIR`, load
   it with `wit-parser` (to get names, the `include` structure, and the
   signatures for `extends` / `pub use` forwarding), and emit
   `include_bytes!` for each WIT file so that editing it triggers a rebuild.
   Without a world, skip this step: crate-backed modules need no WIT.
3. Walk the module's items, recursing into nested `mod`s. Collect the items
   that carry an inner `#[wasm_zero]` attribute and classify them by syntax
   only (see the direction table). Remove the inner attributes and
   `#[archive]` markers.
4. For each module that exports something, emit forwarder impls **in that
   module**, built by copying signatures. Positions marked `#[archive]` or
   `archive` are replaced with `Vec<u8>` / `&[u8]`, plus the conversion
   call.
5. Emit `pub mod wit { generate!(…) }` (own worlds only), the import wrappers,
   the user items, `__WasmZeroComponent`, `__WASM_ZERO_EXPORTS`, and the
   `cfg`'d export macro call: `wit::export!` or `<via>!`.
6. Emit `__wasm_zero` metadata for each `#[archive]` opt-in. This extends the
   existing record format with `kind = "wit-fn"` and a direction field.

### `no_std`

wit-bindgen's generated code needs `alloc`, not `std`. `wasm_zero` depends on
`wit-bindgen` with `default-features = false` and the `macros` and `realloc`
features enabled. Bindings crates have their own feature flags; `wasip3`
works without `std`.

## Drawbacks

- Copying signatures verbatim means you have to write exactly what the trait
  expects (`String` and not `&str` for exported strings, and so on). The
  error messages are rustc's, which are precise but don't mention WIT.
- A bodyless `fn` is unusual Rust. It is legal macro input, but
  rust-analyzer may show a "missing body" diagnostic until it expands the
  macro.
- The same `#[wasm_zero] fn` text means a different ABI depending on whether it
  is inside a `#[wasm_zero(component)]` module. The required `component`
  keyword on the boundary and the "Two ABIs" rules keep this visible, but a
  function moved into or out of a component module silently changes ABI.
  rustc usually catches this, because the core ABI requires rkyv types and the
  canonical ABI requires the trait's types.
- Each own world, and each bindings crate, contributes its own
  `component-type` section, and `wit-component` merges them. That is how
  `wasip3` + `generate!` crates already work, but a world that combines
  several of them needs a test.

## Alternatives

- **Understand WIT types and adapt automatically** (`&str` ↔ `String`,
  `list<u8>` ↔ archive inferred from the struct). This is more convenient, but
  the macro would then need to understand every crate's types and every
  version of every proposal. That is the coupling this RFC avoids. The
  `#[archive]` opt-in keeps the useful case and makes it explicit.
- **Built-in bindings and sugar** (a `wasm_zero::wasi` re-export with default
  `with` mappings, `wasm_zero::http` request types and routes, typed keyvalue
  buckets). These would fit better as optional helper crates built on top of
  this RFC. The macro itself stays neutral about proposals.
- **Implicit name matching** (any `pub fn run` binds `export run`). A typo
  would silently produce an unexported helper. Explicit `#[wasm_zero]` matches
  the core-module model.
- **Generating WIT from Rust** (roadmap item 1). Annotated own-world modules
  carry enough information to emit WIT later. Crate-backed modules don't
  need it.

## Open questions

1. Should imports also be spellable as `extern "wit" { fn print(msg: &str); }`?
   It is friendlier to rust-analyzer before expansion, and also valid macro
   input.
2. `cabi_realloc`: until lazy lowering
   ([component-model#383](https://github.com/WebAssembly/component-model/issues/383))
   lands, a `list<u8>` param arrives at the alignment `cabi_realloc` was asked
   for, which is 1, so `Archived<T>` copies into an aligned buffer once. Should
   wasm_zero provide its own 16-byte-aligned `cabi_realloc`? That would
   conflict with wit-bindgen's `realloc` feature, and with every bindings
   crate that enables it.
3. Can `via` be inferred? The mapping from `wasip3::exports::X::Y` to
   `wasip3::X::<world>::export` isn't regular enough to guess. A trait in the
   bindings crates that names its export macro would need upstream support.
4. Should name-level WIT checks (missing or misspelled exports in own worlds)
   come before or after the first implementation?
5. **Bringing a prebuilt core module into a component.** A
   `wasm_zero_build componentize app.wasm` step could read the `__wasm_zero`
   section and generate WIT plus a small adapter core module. The result is
   one component containing two core modules: the unchanged wasm_zero
   module, which owns the memory and allocator, and the adapter, which
   imports them. Scalar shims could be lifted directly. Buffer shims would go
   through the adapter, with `cabi_realloc` over `malloc` aligned to 16 bytes,
   and a post-return `free`. This is a build-tool feature, so it doesn't
   affect rule 3 of "Two ABIs". Should it be RFC 0002?
6. Should a local `#[wasm_zero] fn` be allowed to override an
   `extends`-delegated export silently, or should it require
   `#[wasm_zero(override)]`?
