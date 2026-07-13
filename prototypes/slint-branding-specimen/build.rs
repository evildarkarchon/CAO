/// Compiles the throwaway Slint specimen with the production-selected Fluent baseline.
fn main() {
    // PROTOTYPE: an explicit style keeps identity comparisons stable across developer machines.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app-window.slint", config)
        .expect("failed to compile the Slint branding specimen");
}
