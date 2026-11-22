//! Links the Windows icon into gf.exe. Other platforms need nothing here
//! (macOS gets its icon from the .app bundle, see scripts/package-macos.sh).

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/gf.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/gf.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("../../packaging/windows/gf.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("linking the Windows icon");
    }
}
