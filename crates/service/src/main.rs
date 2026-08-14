//! calendry-solver — the gRPC service binary.
//!
//! Stateless and input/output only: the solver never touches Postgres. Nuxt
//! assembles a `SolverInput` snapshot and sends it over gRPC.

mod convert;
mod dates;
mod runs;
mod service;

use calendry_solver_proto::v1::solver_service_server::SolverServiceServer;
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "calendry_solver=info,tonic=warn".into()),
        )
        .init();

    let addr = std::env::var("CALENDRY_SOLVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:50051".to_string())
        .parse()?;

    tracing::info!(%addr, "calendry-solver listening");

    Server::builder()
        .add_service(SolverServiceServer::new(service::SolverSvc::new()))
        .serve_with_shutdown(addr, async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;

    Ok(())
}
