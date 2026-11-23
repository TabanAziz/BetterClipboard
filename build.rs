//! Resources and linker settings.

fn main() {
    for watched in ["build.rs", "resources", "assets/bcb.ico", "assets/bcb-paused.ico"] {
        println!("cargo:rerun-if-changed={watched}");
    }
    embed_resource::compile("resources/bcb.rc", embed_resource::NONE);
    let link = |argument: &str| println!("cargo:rustc-link-arg-bins={argument}");
    // Static imports resolve from System32 only.
    link("/DEPENDENTLOADFLAG:0x800");
    // Keep the builder's folder names out of the executable.
    link("/PDBALTPATH:%_PDB%");
}
