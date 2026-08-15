//! The three unary RPCs.

use std::sync::Arc;

use calendry_solver_core::search::{self, Budget};
use calendry_solver_proto::v1 as pb;
use calendry_solver_proto::v1::solver_service_server::SolverService;
use tonic::{Request, Response, Status};

use crate::convert;
use crate::runs::{Registry, RunHalt};

#[derive(Default)]
pub struct SolverSvc {
    registry: Arc<Registry>,
}

impl SolverSvc {
    pub fn new() -> Self {
        Self::default()
    }
}

#[tonic::async_trait]
impl SolverService for SolverSvc {
    /// Returns a run id immediately and optimizes in the background.
    ///
    /// Input validation happens before the run is created, so a malformed
    /// snapshot comes back as INVALID_ARGUMENT on this call rather than as a
    /// run that fails a poll later.
    async fn start_run(
        &self,
        request: Request<pb::StartRunRequest>,
    ) -> Result<Response<pb::StartRunResponse>, Status> {
        let req = request.into_inner();

        let input = req
            .input
            .ok_or_else(|| Status::invalid_argument("request.input is required"))?;
        let scope = req
            .scope
            .ok_or_else(|| Status::invalid_argument("request.scope is required"))?;
        let budget = req.budget.unwrap_or_default();

        // Conversion is CPU-bound and the snapshot can be large, so keep it off
        // the async runtime — but still await it, so validation errors surface
        // on this call.
        let problem = tokio::task::spawn_blocking(move || convert::convert(&input, &scope))
            .await
            .map_err(|e| Status::internal(format!("conversion panicked: {e}")))??;

        // 0 means "pick one and report it", so a run is always reproducible.
        let seed = if req.seed == 0 {
            let mut s = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x5EED);
            if s == 0 {
                s = 0x5EED;
            }
            s
        } else {
            req.seed
        };

        let run = self
            .registry
            .create(seed, budget.max_wall_millis, budget.max_moves, &req.idempotency_key);

        let core_budget = Budget {
            max_wall_millis: budget.max_wall_millis,
            max_moves: budget.max_moves,
        };

        let bg = run.clone();
        tokio::task::spawn_blocking(move || {
            bg.mark_running();
            let halt = RunHalt(bg.clone());
            let outcome = search::solve(&problem, bg.seed, core_budget, &halt);

            bg.record_moves(outcome.moves_evaluated);
            let output = convert::build_output(&problem, &outcome, bg.elapsed_millis());

            let status = if outcome.termination_reason == "cancelled" {
                pb::RunStatus::Cancelled
            } else {
                pb::RunStatus::Succeeded
            };
            bg.finish(status, Some(output), String::new());
        });

        Ok(Response::new(pb::StartRunResponse {
            run_id: run.id.clone(),
            seed,
        }))
    }

    async fn get_status(
        &self,
        request: Request<pb::GetStatusRequest>,
    ) -> Result<Response<pb::GetStatusResponse>, Status> {
        let req = request.into_inner();
        let run = self
            .registry
            .get(&req.run_id)
            .ok_or_else(|| Status::not_found(format!("unknown run '{}'", req.run_id)))?;

        let p = run.snapshot();

        // The full placement is only returned when asked for: this call is
        // polled on a timer and a large-university result is a big message.
        let result = if req.include_result { p.result } else { None };

        Ok(Response::new(pb::GetStatusResponse {
            status: p.status,
            progress: p.progress,
            best_objective: p.best_objective,
            moves_evaluated: p.moves_evaluated,
            elapsed_millis: p.elapsed_millis,
            result,
            error_detail: p.error_detail,
        }))
    }

    async fn cancel_run(
        &self,
        request: Request<pb::CancelRunRequest>,
    ) -> Result<Response<pb::CancelRunResponse>, Status> {
        let req = request.into_inner();
        let run = self
            .registry
            .get(&req.run_id)
            .ok_or_else(|| Status::not_found(format!("unknown run '{}'", req.run_id)))?;

        let (cancelled, status) = run.cancel();
        Ok(Response::new(pb::CancelRunResponse { cancelled, status }))
    }
}
