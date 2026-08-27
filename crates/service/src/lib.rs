//! calendry-solver — the gRPC service.
//!
//! Stateless and input/output only: the solver never touches Postgres. Nuxt
//! assembles a `SolverInput` snapshot and sends it over gRPC.
//!
//! This crate is also where the **proto↔core conversion** lives. The four-crate
//! split exists so that `core` cannot see prost types: they are `String`-id'd,
//! `Option`-wrapped and heap-heavy, and a local search evaluating millions of
//! moves cannot touch that representation. Keeping them in separate crates makes
//! erosion of the boundary a compile error rather than a convention.
//!
//! A library target as well as a binary, so that everything here has a **test
//! surface**. Before it existed, `convert`, `runs` and `service` sat behind `mod`
//! declarations in `main.rs`, no integration test could link them, and in
//! practice none of the conversion module's rejection paths and none of the run
//! registry's state machine was tested at all.

pub mod clock;
pub mod convert;
pub mod dates;
pub mod error;
pub mod runs;
pub mod service;

pub use error::{ConvertError, Resolver};
