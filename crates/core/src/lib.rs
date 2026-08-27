//! # calendry-solver-core
//!
//! The timetabling optimizer: data model, constraint evaluators, and search.
//!
//! This crate is deliberately free of protobuf, tokio, I/O and any clock. It is
//! a pure function from a problem instance to a solution, which is what makes
//! the search testable and a run reproducible from its seed.
//!
//! See `CONTEXT.md` for the domain vocabulary and `docs/adr/` for the decisions
//! this implements.

pub mod aggregates;
pub mod bitset;
pub mod constraints;
pub mod evaluator;
pub mod groups;
pub mod ids;
pub mod preferences;
pub mod problem;
pub mod rng;
pub mod search;
pub mod slots;
pub mod soft;
pub mod solution;

pub mod testing;

pub use aggregates::{ShareInstance, ShareWindow};
pub use constraints::{ConstraintType, Violation};
pub use groups::{GroupClosure, GroupCycle};
pub use preferences::{Preference, PreferenceInstance};
pub use problem::{
    ConstraintInstance, ConstraintSet, Immovable, Problem, ProblemBuilder, ProblemSpec, ScopeSpec,
};
pub use search::{Budget, SolveOutcome, Trial, solve, solve_with};
pub use soft::{Objective, SoftInstance, SoftParams};
pub use solution::{Placement, SearchState, Solution};
