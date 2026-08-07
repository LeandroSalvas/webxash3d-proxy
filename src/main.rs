//! WebRTC to UDP proxy for CS 1.6 / Half-Life game servers.
//!
//! This proxy enables browser clients to connect to traditional game servers
//! by bridging WebRTC data channels to UDP sockets.

mod assets;
mod bridge;
mod config;
mod signaling;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, Response, StatusCode};
use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use clap::Parser;
use serde::Serialize;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use config::Config;

/// Application state shared across handlers
#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    spectator_counter: Arc<AtomicUsize>,
}

/// Client configuration response
#[derive(Serialize)]
struct ClientConfig {
    arguments: Vec<String>,
    console: Vec<String>,
    game_dir: String,
    spectator_name: String,
    libraries: ClientLibraries,
    dynamic_libraries: Vec<String>,
    files_map: std::collections::HashMap<String, String>,
    proxy_host: String,
    proxy_port: u16,
}

#[derive(Serialize)]
struct ClientLibraries {
    client: String,
    server: String,
    extras: String,
    menu: String,
    filesystem: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments
    let config = Config::parse();

    // Setup logging
    let filter = if config.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    info!("Starting webxash3d-proxy");
    info!("Game server: {}", config.server);
    info!("Listen address: {}", config.listen_addr());
    info!("Game directory: {}", config.game_dir);

    if let Some(ref ip) = config.public_ip {
        info!("Public IP for ICE: {}", ip);
    }

    if let Some(ref package_zip) = config.package_zip {
        info!("Package ZIP: {}", package_zip);
    } else {
        warn!("No --package-zip specified, valve.zip will not be available");
    }

    if config.use_embedded_assets() {
        info!("Serving embedded assets");
    } else if let Some(ref static_dir) = config.static_dir {
        info!("Development mode: serving static files from {}", static_dir);
    }

    let state = AppState {
        config: Arc::new(config.clone()),
        spectator_counter: Arc::new(AtomicUsize::new(0)),
    };

    // Build router with API routes
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/websocket", get(ws_handler))
        .route("/health", get(health_handler))
        .route("/config", get(config_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state.clone());

    // Add static file serving
    let app = if let Some(ref static_dir) = config.static_dir {
        // Development mode: serve from filesystem
        app.fallback_service(ServeDir::new(static_dir).append_index_html_on_directories(true))
    } else {
        // Production mode: serve embedded assets + package_zip for valve.zip
        app.fallback(move |request: Request<Body>| {
            let state = state.clone();
            let path = request.uri().path().to_string();
            async move { serve_static(path, state).await }
        })
    };

    // Start server
    let listener = tokio::net::TcpListener::bind(config.listen_addr()).await?;
    info!("Server listening on http://{}", config.listen_addr());

    axum::serve(listener, app).await?;

    Ok(())
}

/// Serve static files from embedded assets or `package_zip`
async fn serve_static(path: String, state: AppState) -> Response<Body> {
    // Normalize path - remove leading slash
    let path = path.trim_start_matches('/');

    // Handle valve.zip specially - serve from package_zip path
    if path == "valve.zip" {
        return serve_package_zip(&state).await;
    }

    // Serve from embedded assets
    assets::serve_embedded(path)
}

/// Serve valve.zip from the `package_zip` path
async fn serve_package_zip(state: &AppState) -> Response<Body> {
    let Some(ref package_path) = state.config.package_zip else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("valve.zip not configured (use --package-zip)"))
            .expect("building response should not fail");
    };

    // Read the file
    let mut file = match File::open(package_path).await {
        Ok(f) => f,
        Err(e) => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from(format!("Failed to open valve.zip: {e}")))
                .expect("building response should not fail");
        }
    };

    // Get file size for Content-Length
    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(e) => {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Failed to read file metadata: {e}")))
                .expect("building response should not fail");
        }
    };

    // Read file contents
    #[allow(clippy::cast_possible_truncation)]
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    if let Err(e) = file.read_to_end(&mut contents).await {
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(format!("Failed to read valve.zip: {e}")))
            .expect("building response should not fail");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_LENGTH, contents.len())
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"valve.zip\"",
        )
        .body(Body::from(contents))
        .expect("building response should not fail")
}

/// WebSocket upgrade handler
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let client_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    ws.on_upgrade(move |socket| handle_socket(socket, state, client_id))
}

/// Handle upgraded WebSocket connection
async fn handle_socket(socket: WebSocket, state: AppState, client_id: String) {
    signaling::handle_websocket(socket, state.config, client_id).await;
}

/// Health check endpoint
async fn health_handler() -> &'static str {
    "OK"
}

/// Client configuration endpoint
/// Returns configuration needed by the `Xash3D` WASM client
async fn config_handler(State(state): State<AppState>) -> Json<ClientConfig> {
    let game_dir = &state.config.game_dir;

    // Build files_map to translate .so requests to .wasm files
    let mut files_map = std::collections::HashMap::new();

    // Map server library requests (engine requests .so, we serve .wasm)
    files_map.insert(
        "dlls/cs_emscripten_wasm32.so".to_string(),
        format!("/{game_dir}/dlls/cs_emscripten_wasm32.wasm"),
    );
    files_map.insert(
        "dlls/hl_emscripten_wasm32.so".to_string(),
        format!("/{game_dir}/dlls/cs_emscripten_wasm32.wasm"),
    );
    files_map.insert(
        "/rwdir/filesystem_stdio.wasm".to_string(),
        "/filesystem_stdio.wasm".to_string(),
    );

    // Use public_ip if provided, otherwise use host
    let proxy_host = state
        .config
        .public_ip
        .clone()
        .unwrap_or_else(|| state.config.host.clone());

    let spectator_num = state.spectator_counter.fetch_add(1, Ordering::SeqCst) + 1;

    Json(ClientConfig {
        arguments: vec![
            "-windowed".to_string(),
            "-game".to_string(),
            game_dir.clone(),
        ],
        console: state.config.get_console_commands(),
        game_dir: game_dir.clone(),
        spectator_name: format!("spectator{spectator_num}"),
        libraries: ClientLibraries {
            // These paths are relative to the static directory
            // The client will load them from these URLs
            client: format!("/{game_dir}/cl_dlls/client_emscripten_wasm32.wasm"),
            server: format!("/{game_dir}/dlls/cs_emscripten_wasm32.wasm"),
            extras: format!("/{game_dir}/extras.pk3"),
            menu: format!("/{game_dir}/cl_dlls/menu_emscripten_wasm32.wasm"),
            filesystem: "/filesystem_stdio.wasm".to_string(),
        },
        dynamic_libraries: vec![
            "dlls/cs_emscripten_wasm32.so".to_string(),
            "/rwdir/filesystem_stdio.wasm".to_string(),
        ],
        files_map,
        proxy_host,
        proxy_port: state.config.port,
    })
}
