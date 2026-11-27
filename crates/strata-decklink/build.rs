//! Compile le pont DeckLink (`shim/shim.cpp`) avec les en-têtes du SDK de `sdk/linux/`. Repris de CG.
//!
//! Il ne faut qu'un compilateur C++ : le pilote (Blackmagic Desktop Video, `libDeckLinkAPI.so`) est
//! chargé au lancement, il n'est nécessaire ni pour compiler ni pour démarrer. Si le pont ne se
//! compile pas, la construction continue sans lui (avec un avertissement) et le Core dit que la
//! prise en charge de DeckLink manque ; `STRATA_DECKLINK_REQUIRED=1` en fait une erreur (releases).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(decklink_shim)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=sdk");
    println!("cargo:rerun-if-env-changed=STRATA_DECKLINK_REQUIRED");
    match build() {
        Ok(()) => println!("cargo:rustc-cfg=decklink_shim"),
        Err(e) => {
            let required = std::env::var("STRATA_DECKLINK_REQUIRED").is_ok_and(|v| !v.is_empty() && v != "0");
            if required {
                panic!("cannot build the DeckLink shim: {e}");
            }
            println!("cargo:warning=DeckLink support left out of this build: {e}");
        }
    }
}

fn build() -> Result<(), String> {
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "linux" {
        return Err("Strata servers run Linux only".into());
    }
    if target_env == "musl" {
        // Un binaire musl statique ne peut pas charger le pilote (glibc).
        return Err("static musl builds cannot load the DeckLink runtime (build for a -gnu target)".into());
    }
    let sdk = root.join("sdk/linux");
    cc::Build::new()
        .cpp(true)
        .flag("-std=c++11")
        .warnings(false)
        .include(&sdk)
        .file(root.join("shim/shim.cpp"))
        .file(sdk.join("DeckLinkAPIDispatch.cpp"))
        .try_compile("strata_decklink")
        .map_err(|e| e.to_string())?;
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rustc-link-lib=pthread");
    Ok(())
}
