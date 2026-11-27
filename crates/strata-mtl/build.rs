//! Construit le pont C (`shim/mtl.c`) avec les en-têtes de MTL (`include/`, copiés de la version
//! livrée avec le Core). Sans compilateur C ou sans en-têtes, le crate se construit sans pont :
//! le Core dit alors que le ST 2110 par MTL n'est pas disponible. `STRATA_MTL_REQUIRED=1` en
//! fait une erreur (releases).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(mtl_shim)");
    println!("cargo:rerun-if-changed=shim/mtl.c");
    println!("cargo:rerun-if-changed=include");
    println!("cargo:rerun-if-env-changed=STRATA_MTL_REQUIRED");
    let r = cc::Build::new().file("shim/mtl.c").include("include").flag_if_supported("-Wno-unused-parameter").warnings(false).try_compile("strata_mtl");
    match r {
        Ok(()) => {
            println!("cargo:rustc-cfg=mtl_shim");
            println!("cargo:rustc-link-lib=dl");
        }
        Err(e) if std::env::var("STRATA_MTL_REQUIRED").is_ok_and(|v| !v.is_empty() && v != "0") => panic!("MTL bridge not built: {e}"),
        Err(e) => println!("cargo:warning=MTL bridge not built: {e}"),
    }
}
