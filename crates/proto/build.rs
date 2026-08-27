use std::path::PathBuf;

/// Where the schema comes from.
///
/// The `.proto` files are NOT vendored into this repository. They are the
/// contract, owned by github.com/MindCollaps/calendry-proto and shared with the
/// Nuxt app, and they are consumed here through a git submodule pinned to an
/// exact revision.
///
/// Resolution order:
///   1. `CALENDRY_PROTO_DIR` — explicit override, for CI and reproducible-build
///      environments that provide the checkout themselves.
///   2. `vendor/calendry-proto/proto` — the submodule. This is the steady state:
///      a submodule is how a *language-neutral* proto repo gets pinned, since it
///      has no Cargo.toml for cargo to depend on directly.
///   3. Nothing. There is deliberately no third option — see below.
///
/// # Why there is no sibling-checkout fallback
///
/// An earlier revision of this script fell back to `../calendry-proto/proto`.
/// That was removed on purpose. A sibling checkout is unpinned and unversioned,
/// so the fallback let the build *succeed* against whatever happened to be in a
/// directory next door whenever the submodule was missing or uninitialized —
/// which is precisely the silent schema-drift failure this project guards
/// against everywhere else. A missing submodule must fail loudly and say how to
/// fix it, not quietly resolve somewhere unpinned.
const SUBMODULE_REL: &str = "vendor/calendry-proto/proto";
const SENTINEL: &str = "calendry/solver/v1/service.proto";

fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // crates/proto -> crates -> <repo root>
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}

fn resolve_proto_root() -> Result<PathBuf, String> {
    println!("cargo:rerun-if-env-changed=CALENDRY_PROTO_DIR");

    if let Ok(dir) = std::env::var("CALENDRY_PROTO_DIR") {
        let root = PathBuf::from(dir);
        if !root.join(SENTINEL).exists() {
            return Err(format!(
                "CALENDRY_PROTO_DIR is set to {}, but {SENTINEL} is not there.",
                root.display()
            ));
        }
        return Ok(root);
    }

    let root = repo_root().join(SUBMODULE_REL);
    if root.join(SENTINEL).exists() {
        return Ok(root);
    }

    Err(format!(
        "the calendry-proto submodule is not checked out at {}.\n\
         \n\
         Fix it with:\n\
             git submodule update --init --recursive\n\
         \n\
         Or clone with submodules in the first place:\n\
             git clone --recurse-submodules https://github.com/MindCollaps/calendry-solver.git\n\
         \n\
         To build against a checkout somewhere else, set CALENDRY_PROTO_DIR to a\n\
         directory containing {SENTINEL}.",
        root.display()
    ))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = resolve_proto_root()?;

    let files = [
        "calendry/solver/v1/constraints.proto",
        "calendry/solver/v1/model.proto",
        "calendry/solver/v1/service.proto",
    ];

    for f in &files {
        let path = root.join(f);
        if !path.exists() {
            return Err(format!(
                "{} is missing from the schema checkout at {}. The submodule may be \
                 pinned to a revision that predates it.",
                f,
                root.display()
            )
            .into());
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }

    // Re-run when the submodule pointer itself moves, so a `git submodule update`
    // does not leave stale generated code behind.
    let gitlink = repo_root().join(".git/modules/vendor/calendry-proto/HEAD");
    if gitlink.exists() {
        println!("cargo:rerun-if-changed={}", gitlink.display());
    }

    let paths: Vec<PathBuf> = files.iter().map(|f| root.join(f)).collect();
    let includes: Vec<PathBuf> = vec![root];

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&paths, &includes)?;

    Ok(())
}
