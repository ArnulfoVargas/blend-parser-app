mod catalog_bake;

fn main() {
    println!("cargo::rerun-if-changed=src/catalog.json");
    println!("cargo::rerun-if-changed=catalog_bake.rs");
    println!("cargo::rerun-if-changed=build.rs");
    let input = std::fs::read_to_string("src/catalog.json").expect("read the versioned printer catalogue");
    let baked = catalog_bake::bake(&input).expect("bake the printer catalogue");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    std::fs::write(output.join("printer_catalog.rs"), baked).expect("write the baked printer catalogue");
}
