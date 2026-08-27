//! calendry-solver — the gRPC service binary.
//!
//! Thin by design: everything it does lives in the library target, so that it is
//! also reachable from an integration test. See `lib.rs`.

use std::sync::Arc;
use std::time::Duration;

use calendry_solver::clock::SystemClock;
use calendry_solver::runs::Registry;
use calendry_solver::service::SolverSvc;
use calendry_solver_proto::v1::solver_service_server::SolverServiceServer;
use tonic::transport::Server;

/// How long a finished run's result is kept so a poller can collect it.
///
/// Nuxt persists progress into its own `solver_run` table, so this only has to
/// outlive a poll interval by a comfortable margin — not a session.
const RUN_RETENTION: Duration = Duration::from_secs(15 * 60);
const REAP_INTERVAL: Duration = Duration::from_secs(60);

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

    let registry = Arc::new(Registry::new(Arc::new(SystemClock)));

    // Run state dies with the process, and finished runs die before that. The
    // registry can now say so; without a reaper, every completed run retained
    // its full `SolverOutput` — every placed Session of a large university — for
    // as long as the process lived.
    let reaper = registry.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(REAP_INTERVAL);
        loop {
            ticker.tick().await;
            let dropped = reaper.reap(RUN_RETENTION);
            if dropped > 0 {
                tracing::debug!(dropped, "reaped finished runs");
            }
        }
    });

    tracing::info!(%addr, "calendry-solver listening");

    Server::builder()
        .add_service(SolverServiceServer::new(SolverSvc::new(registry)))
        .serve_with_shutdown(addr, async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;

    Ok(())
}
