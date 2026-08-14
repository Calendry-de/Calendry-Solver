//! Parametrized benchmark instance generator.
//!
//! **Placeholder — scheduled for slice 5.** The crate exists now so the
//! workspace shape is settled, but it deliberately contains no generator yet:
//! benchmark data is for performance and solution-quality testing, and there is
//! no metaheuristic to measure until simulated annealing / LNS lands.
//!
//! When it is built, it produces instances across a parameter space spanning
//! small-school to large-university scale, with a few named presets on top.
//!
//! It must stay **separate from correctness fixtures**, which are hand-written
//! and checked in at `calendry_solver_core::testing`. A generator bug that
//! produced a wrong fixture would be a bug that silently validates itself.

/// Named scale presets, to be built on top of the parameter space.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Preset {
    SmallSchool,
    LargeSchool,
    SmallUniversity,
    LargeUniversity,
}
