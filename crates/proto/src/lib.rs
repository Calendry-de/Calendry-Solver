//! Generated protobuf and gRPC bindings for the Calendry solver contract.
//!
//! The `.proto` sources live in a separate, language-neutral repository
//! (`github.com/MindCollaps/calendry-proto`) because the Nuxt app consumes the
//! same contract. They are never copied into this repo — see `build.rs` for how
//! the checkout is located.

pub mod v1 {
    tonic::include_proto!("calendry.solver.v1");
}

pub use v1::*;
