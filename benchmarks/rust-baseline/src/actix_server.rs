//! Pure Rust Actix-web server — same tech stack as Robyn.
//! Equivalent endpoints to examples/hello.py for fair comparison.

use actix_web::{get, web, App, HttpResponse, HttpServer};
use serde_json::json;

#[get("/")]
async fn index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain")
        .body("Hello from Actix-web (pure Rust)!")
}

#[get("/hello/{name}")]
async fn greet(path: web::Path<String>) -> HttpResponse {
    let name = path.into_inner();
    HttpResponse::Ok()
        .content_type("application/json")
        .json(json!({"message": format!("Hello, {}!", name)}))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    println!("\n  Actix-web (pure Rust) listening on http://127.0.0.1:8003\n");

    HttpServer::new(|| App::new().service(index).service(greet))
        .bind("127.0.0.1:8003")?
        .run()
        .await
}
