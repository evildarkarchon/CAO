//! Compiles the window's `.slint` files in the Fluent style (#462, #466, #508).

fn main() {
    // Slint picks the widget style at compile time, so one style is one compile.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        // The `debug-info` feature: element ids, type names and descendant
        // traversal for the headless tests' `ElementHandle` queries.
        .with_debug_info(std::env::var_os("CARGO_FEATURE_DEBUG_INFO").is_some());
    slint_build::compile_with_config("ui/main.slint", config).unwrap();
}
