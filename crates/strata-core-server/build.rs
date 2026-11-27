// L'interface web est embarquée dans le binaire (rust-embed, web/dist). Le serveur doit compiler
// sans elle (CI Rust, tests) : le dossier est créé vide s'il manque.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed=../../web/dist");
}
