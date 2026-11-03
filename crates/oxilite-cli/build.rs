//! With the `elpi` feature, the binary links `libleanelpi` by `@rpath` (macOS) or by name
//! (Linux): give it the library's directory as a run path, so `oxilite elpi` finds it without
//! `DYLD_LIBRARY_PATH` / `LD_LIBRARY_PATH`. The `leanelpi` crate's own run path does not reach
//! the binaries that depend on it.
fn main() {
    println!("cargo:rerun-if-env-changed=LEANELPI_DIST");
    if std::env::var_os("CARGO_FEATURE_ELPI").is_none() {
        return;
    }
    if let Some(dist) = std::env::var_os("LEANELPI_DIST") {
        let dist = std::path::PathBuf::from(dist);
        let dist = dist.canonicalize().unwrap_or(dist);
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{}", dist.display());
    }
}
