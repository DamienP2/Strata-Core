//! Construit le pont C (`shim/mtl.c`) avec les en-têtes de MTL (`include/`, copiés de la version
//! livrée avec le Core). Sans compilateur C ou sans en-têtes, le crate se construit sans pont :
//! le Core dit alors que le ST 2110 par MTL n'est pas disponible.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(mtl_shim)");
    println!("cargo:rerun-if-changed=shim/mtl.c");
    println!("cargo:rerun-if-changed=include");
    let r = cc::Build::new()
        .file("shim/mtl.c")
        .include("include")
        .flag_if_supported("-Wno-unused-parameter")
        .warnings(false)
        .try_compile("strata_mtl");
    match r {
        Ok(()) => {
            println!("cargo:rustc-cfg=mtl_shim");
            println!("cargo:rustc-link-lib=dl");
        }
        Err(e) => println!("cargo:warning=MTL bridge not built: {e}"),
    }
}
