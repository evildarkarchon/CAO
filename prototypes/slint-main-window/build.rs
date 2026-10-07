// PROTOTYPE — compiles ui/main.slint once per Slint style so the switcher can compare them.
// Slint picks the widget style at compile time, so each style becomes its own Rust module.

const STYLES: &[&str] = &["fluent", "material", "cosmic"];

fn main() {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let input = manifest_dir.join("ui").join("main.slint");
    println!("cargo:rerun-if-changed=ui");

    for style in STYLES {
        let config = slint_build::CompilerConfiguration::new().with_style((*style).into());
        let deps = slint_build::compile_with_output_path(&input, out_dir.join(format!("{style}.rs")), config)
            .unwrap_or_else(|e| panic!("slint compile failed for style {style}: {e:?}"));
        for dep in deps {
            println!("cargo:rerun-if-changed={}", dep.display());
        }
    }
}
