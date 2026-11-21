use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::{from_fn, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Extension, Router,
};
use std::path::PathBuf;
use temps_plugin_sdk::prelude::{AuthenticatedCaller, PluginRole};

#[path = "../src/board.rs"]
mod board;

const ADDRESS: &str = "127.0.0.1:18763";

async fn loopback_only(request: Request, next: Next) -> Response {
    let headers = request.headers();
    if headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        != Some(ADDRESS)
        || headers
            .get(header::ORIGIN)
            .is_some_and(|value| value != "http://127.0.0.1:18763")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

#[tokio::main]
async fn main() -> Result<(), board::Error> {
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("expected test database path")?,
    );
    let caller = AuthenticatedCaller::without_platform_delegation(
        1,
        "preview@localhost",
        PluginRole::User,
        ["projects:read", "projects:write"],
    );
    let app = Router::new()
        .route(
            "/",
            get(|| async { Redirect::temporary("/api/x/agent-board/ui/") }),
        )
        .nest(
            "/api/x/agent-board",
            board::router_with(
                board::BoardStore::open(path, false)?,
                post(
                    |state, Extension(caller): Extension<AuthenticatedCaller>, request| {
                        board::mcp(state, caller, axum::http::HeaderMap::new(), request)
                    },
                ),
            ),
        )
        .layer(Extension(caller))
        .layer(from_fn(loopback_only));
    axum::serve(tokio::net::TcpListener::bind(ADDRESS).await?, app).await?;
    Ok(())
}
