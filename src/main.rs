use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use poem::Server;
use poem::listener::TcpListener;
use tracing_subscriber::EnvFilter;

use error_menu::app::AppState;
use error_menu::database::Database;
use error_menu::forge::reader::ForgeReader;
use error_menu::http::oauth::GithubAuth;

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_DATABASE_URL: &str = "sqlite:error-menu.db";
const DEFAULT_DATA_ROOT: &str = ".tmp/analysis";

fn environment(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let database_url = environment("DATABASE_URL", DEFAULT_DATABASE_URL);
    let data_root = PathBuf::from(environment("DATA_ROOT", DEFAULT_DATA_ROOT));
    let address = match std::env::var("BIND_ADDRESS") {
        Ok(address) => address.parse::<SocketAddr>().map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid BIND_ADDRESS: {error}"),
            )
        })?,
        Err(std::env::VarError::NotPresent) => {
            SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT))
        }
        Err(error) => return Err(std::io::Error::other(error)),
    };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let database = Database::open(&database_url, 0)
        .await
        .map_err(std::io::Error::other)?;
    let forge = ForgeReader::new().map_err(std::io::Error::other)?;
    let state = Arc::new(AppState::new(database, forge, &data_root));
    let github_auth =
        GithubAuth::from_environment(Arc::clone(&state)).map_err(std::io::Error::other)?;
    tracing::info!(%address, "error.menu is listening");

    tokio::spawn(error_menu::worker::queue::serve(
        Arc::clone(&state),
        format!("app-{}", std::process::id()),
    ));

    Server::new(TcpListener::bind(address))
        .run(error_menu::http::routes(state, Some(github_auth)))
        .await
}
