#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(rust_2018_idioms)]
#![warn(unused_qualifications)]
#![warn(unused_crate_dependencies)]

use crate::relay::server::RelayServer;
use crate::udp::paper_interface::PaperInterface;
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use tokio::signal;
use tracing::{error, info};
use tracing_subscriber::FmtSubscriber;

mod config;
mod relay;
mod transport;
mod udp;
mod webrtc;

fn get_log_level() -> tracing::Level {
    let log_level_str = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "debug".to_string());

    match log_level_str.to_lowercase().as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => {
            eprintln!("Invalid log level '{}', defaulting to DEBUG", log_level_str);
            tracing::Level::DEBUG
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let subscriber = FmtSubscriber::builder()
        .with_max_level(get_log_level())
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let config = config::loader::load_config("config.toml")?;
    let addr: SocketAddr = config
        .udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let transport = PaperInterface::new(addr).await?;

    let http_addr: SocketAddr = config
        .http_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve HTTP host name")?;

    let mut server = RelayServer::new(transport, config);

    info!("relay server started");
    info!("UDP server listening on {}", addr);
    info!("HTTP server will listen on {}", http_addr);
    info!("To connect via UDP, use address: {}", addr);
    info!(
        "To connect via WebRTC, send HTTP POST to http://{}/signaling",
        http_addr
    );

    tokio::select! {
        res = server.0.run() => {
            if let Err(e) = res {
                error!("server error: {}", e);
            }
        }
        _ = signal::ctrl_c() => {
            info!("shutdown signal received");
        }
    }

    info!("shutting down server");
    server.0.cleanup().await;

    Ok(())
}
