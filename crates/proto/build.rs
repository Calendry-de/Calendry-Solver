use std::path::PathBuf;

/// The `.proto` files are NOT vendored into this repository — they are the
/// contract, owned by github.com/MindCollaps/calendry-proto and shared with the
/// Nuxt app. This build script locates that checkout and runs codegen against
/// it.
///
/// Resolution order:
///   1. `CALENDRY_PROTO_DIR` — explicit override, used by CI.
///   2. `proto/` inside this repo — the git submodule, once calendry-proto has
///      commits to pin. This is the intended steady state: a submodule is how a
///      *language-neutral* proto repo gets pinned to a revision, since it has no
///      Cargo.toml for cargo to depend on directly.
///   3. `../calendry-proto/proto` — a sibling checkout. Works today, while
///      calendry-proto is still unpushed and has no revision to pin to.
fn resolve_proto_root() -> PathBuf {
    println!("cargo:rerun-if-env-changed=CALENDRY_PROTO_DIR");

    if let Ok(dir) = std::env::var("CALENDRY_PROTO_DIR") {
        return PathBuf::from(dir);
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest.parent().unwrap().parent().unwrap();

    let submodule = repo_root.join("proto");
    if submodule.join("calendry/solver/v1/service.proto").exists() {
        return submodule;
    }

    repo_root.parent().unwrap().join("calendry-proto/proto")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = resolve_proto_root();

    let files = [
        "calendry/solver/v1/constraints.proto",
        "calendry/solver/v1/model.proto",
        "calendry/solver/v1/service.proto",
    ];

    for f in &files {
        let path = root.join(f);
        if !path.exists() {
            return Err(format!(
                "cannot find {}\n\
                 calendry-proto was not located at {}.\n\
                 Set CALENDRY_PROTO_DIR, add the submodule, or clone \
                 https://github.com/MindCollaps/calendry-proto as a sibling directory.",
                f,
                root.display()
            )
            .into());
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let paths: Vec<PathBuf> = files.iter().map(|f| root.join(f)).collect();
    let includes: Vec<PathBuf> = vec![root.clone()];

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&paths, &includes)?;

    Ok(())
}
