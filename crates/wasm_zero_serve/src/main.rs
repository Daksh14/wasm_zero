use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use axum::http::HeaderValue;
use axum::middleware::map_response;
use axum::response::Response;
use tower_http::services::ServeDir;

const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Stamp every response with the cross-origin isolation headers. A page is only
/// cross-origin isolated — and thus only allowed to back a WebAssembly.Memory
/// with a SharedArrayBuffer — when it's served with both of these. The threaded
/// `wasm_zero_rayon` demo needs it so its Web Workers can share linear memory;
/// the other (single-threaded) pages are unaffected since all their assets are
/// same-origin.
async fn cross_origin_isolation(mut res: Response) -> Response {
    let headers = res.headers_mut();
    headers.insert(
        "Cross-Origin-Opener-Policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "Cross-Origin-Embedder-Policy",
        HeaderValue::from_static("require-corp"),
    );
    res
}

#[tokio::main]
async fn main() {
    let root = PathBuf::from(WORKSPACE_ROOT)
        .canonicalize()
        .expect("workspace root must exist");

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    let app = Router::new()
        .fallback_service(ServeDir::new(&root))
        .layer(map_response(cross_origin_isolation));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");

    println!("serving {} on http://{addr}", root.display());
    println!();
    println!("test pages:");
    println!("  http://{addr}/crates/wasm_zero_test/index.html");
    println!("  http://{addr}/crates/wasm_zero_test_nostd/index.html");
    println!("  http://{addr}/crates/wasm_zero_test_canvas/index.html  (GPU-style compute → canvas)");
    println!("  http://{addr}/crates/wasm_zero_rayon/index.html  (rayon threads, shared memory)");
    println!("  http://{addr}/crates/wasm_zero_rayon_demo/index.html  (parallel Mandelbrot)");
    println!("  http://{addr}/benchmark/web/index.html  (wasm_bindgen vs wasm_zero)");
    println!();
    println!("(all responses carry COOP/COEP so the rayon page is cross-origin isolated)");
    println!();
    println!("build the wasm first if you haven't:");
    println!("  cargo build --target wasm32-unknown-unknown -p wasm_zero_test");
    println!("  cargo build --target wasm32-unknown-unknown -p wasm_zero_test_nostd");
    println!("  cargo build --release --target wasm32-unknown-unknown -p wasm_zero_test_canvas");
    // Build the rayon crate from *inside* its dir: cargo discovers
    // .cargo/config.toml from the cwd, and only the crate-local one carries the
    // shared-memory link flags (build from the workspace root and you get a
    // non-shared module).
    println!("  (cd crates/wasm_zero_rayon && cargo build --release)");
    println!("  (cd crates/wasm_zero_rayon_demo && cargo build --release)");
    println!("  ./benchmark/build.sh");

    axum::serve(listener, app).await.expect("server crashed");
}
