//! Pure Rust Axum server — same tech stack as Pyre (Tokio + Hyper).
//! Equivalent endpoints to examples/hello.py for fair comparison.

use axum::{extract::Path, routing::get, Json, Router};
use serde_json::{json, Value};
use std::net::SocketAddr;

async fn index() -> &'static str {
    "Hello from Axum (pure Rust)!"
}

async fn greet(Path(name): Path<String>) -> Json<Value> {
    Json(json!({"message": format!("Hello, {}!", name)}))
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(index))
        .route("/hello/{name}", get(greet));

    let addr = SocketAddr::from(([127, 0, 0, 1], 8002));
    println!("\n  Axum (pure Rust) listening on http://{addr}\n");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
