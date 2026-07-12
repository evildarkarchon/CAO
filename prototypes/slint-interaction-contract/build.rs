/// Compiles the throwaway Slint markup without touching the production CMake graph.
fn main() {
    // PROTOTYPE: keep the UI isolated from the production CMake build.
    slint_build::compile("ui/app-window.slint").expect("failed to compile the Slint prototype");
}
