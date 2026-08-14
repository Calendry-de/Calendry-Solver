//! # calendry-solver-core
//!
//! The timetabling optimizer: data model, constraint evaluators, and search.
//!
//! This crate is deliberately free of protobuf, tokio, I/O and any clock. It is
//! a pure function from a problem instance to a solution, which is what makes
//! the search testable and a run reproducible from its seed.
//!
//! See the repository `CLAUDE.md` for the domain model and the architecture
//! decisions this implements.

pub mod bitset;
pub mod constraints;
pub mod evaluator;
pub mod groups;
pub mod ids;
pub mod problem;
pub mod rng;
pub mod search;
pub mod slots;
pub mod solution;

pub mod testing;

pub use groups::{GroupClosure, GroupCycle};
pub use problem::{ConstraintInstance, ConstraintSet, Immovable, Problem};
pub use search::{Budget, SolveOutcome, solve};
pub use solution::{Occupancy, Placement, Solution};
