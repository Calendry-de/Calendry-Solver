//! The benchmark harness binary.
//!
//! Deliberately thin: parse argv, run, print. Everything it does lives in
//! `calendry_solver_gen::bench`, so that it has a test surface — an integration
//! test cannot link a binary, so anything left in here is untestable by
//! construction.
//!
//! ```text
//! cargo run --release -p calendry-solver-gen --bin bench -- [preset...] \
//!     [--gen-seed N] [--seeds N] [--moves N] [--wall SECONDS] [--probe N] \
//!     [--calibrate] [--diagnose N] [--evaluate] [--elective RATIO]
//!     [--preferences RATIO]
//! ```

use calendry_solver_gen::bench::{self, Args};

fn main() {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    print!("{}", bench::run(&args));
}
