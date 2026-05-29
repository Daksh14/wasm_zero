use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use tower_http::services::ServeDir;

const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

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

    let app = Router::new().fallback_service(ServeDir::new(&root));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");

    println!("serving {} on http://{addr}", root.display());
    println!();
    println!("test pages:");
    println!("  http://{addr}/crates/wasm_zero_test/index.html");
    println!("  http://{addr}/crates/wasm_zero_test_nostd/index.html");
    println!("  http://{addr}/crates/wasm_zero_test_canvas/index.html  (GPU-style compute → canvas)");
    println!("  http://{addr}/benchmark/web/index.html  (wasm_bindgen vs wasm_zero)");
    println!();
    println!("build the wasm first if you haven't:");
    println!("  cargo build --target wasm32-unknown-unknown -p wasm_zero_test");
    println!("  cargo build --target wasm32-unknown-unknown -p wasm_zero_test_nostd");
    println!("  cargo build --release --target wasm32-unknown-unknown -p wasm_zero_test_canvas");
    println!("  ./benchmark/build.sh");

    axum::serve(listener, app).await.expect("server crashed");
}
