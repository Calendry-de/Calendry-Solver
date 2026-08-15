//! Parametrized benchmark instance generator.
//!
//! Produces instances across a parameter space spanning small-school to
//! large-university scale, with named presets on top, for **performance and
//! solution-quality** measurement of the search.
//!
//! It is deliberately kept **separate from correctness fixtures**, which are
//! hand-written and checked in at `calendry_solver_core::testing`. A generator
//! bug that produced a wrong fixture would be a bug that silently validates
//! itself, so the two never share a source of truth. The tests in this crate
//! accordingly assert only that generated instances are *well-formed and
//! reproducible* — never that the solver's answer on one of them is right.
//!
//! ```no_run
//! use calendry_solver_gen::{Preset, generate};
//!
//! let instance = generate(&Preset::SmallSchool.params(), 42);
//! println!("saturation {:.3}", instance.stats.saturation);
//! ```
//!
//! The `bench` binary drives this: `cargo run --release -p calendry-solver-gen
//! --bin bench -- small-school`.

pub mod diagnose;
pub mod generate;
pub mod params;

pub use generate::{GeneratedInstance, InstanceStats, digest, generate, person_clique};
pub use params::{InstanceParams, Preset, TARGET_SATURATION};
