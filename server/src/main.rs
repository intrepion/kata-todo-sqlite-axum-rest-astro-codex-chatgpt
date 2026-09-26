mod api;
mod model;
mod store;

use directories::ProjectDirs;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let database_path = database_path()?;
    let store = store::Store::open(&database_path)?;
    let web_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/dist");
    let app = api::router(store, web_dir);
    let port = std::env::var("TODO_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, database = %database_path.display(), "To Do List server is ready");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn database_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = std::env::var("TODO_DB_PATH") {
        return Ok(PathBuf::from(path));
    }
    let project_dirs = ProjectDirs::from("com", "Codex", "TodoList")
        .ok_or("Could not determine an application data directory; set TODO_DB_PATH.")?;
    Ok(project_dirs.data_local_dir().join("todo-list.sqlite3"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        let _ = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install signal handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {}, }
}
