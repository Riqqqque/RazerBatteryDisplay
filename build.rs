use std::{env, path::Path};

// assets/app.res is compiled from assets/app.rc, so builds do not need rc.exe:
// rc /nologo /fo assets\app.res assets\app.rc
fn main() {
    println!("cargo:rerun-if-changed=assets/app.res");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let resource = Path::new(&manifest_dir).join("assets").join("app.res");
    println!("cargo:rustc-link-arg-bins={}", resource.display());
}
