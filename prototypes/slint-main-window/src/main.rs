// PROTOTYPE — throwaway Slint translation of CAO's main window, for wayfinder ticket #466
// ("Prototype the main window in Slint"). Not the port. Delete when the ticket is resolved.
//
// Run: cargo run --release -- [--style fluent|material|cosmic] [--menu native|drawn] [--log rows|plain] [--light]
// The yellow PROTOTYPE pill in the window switches between these (by relaunching).

mod shared;

// One module per Slint style, all generated from ui/main.slint by build.rs. The same app.rs is
// textually included into each, so `super::*` resolves to that style's generated types.
mod fluent {
    include!(concat!(env!("OUT_DIR"), "/fluent.rs"));
    pub mod app {
        use super::*;
        include!("app.rs");
    }
}

mod material {
    include!(concat!(env!("OUT_DIR"), "/material.rs"));
    pub mod app {
        use super::*;
        include!("app.rs");
    }
}

mod cosmic {
    include!(concat!(env!("OUT_DIR"), "/cosmic.rs"));
    pub mod app {
        use super::*;
        include!("app.rs");
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let cfg = shared::Config::from_args();
    if cfg.menu == "drawn" {
        // Undocumented: makes Slint draw the MenuBar itself instead of using a native muda
        // HMENU. Read at window creation, so it must be set before the first window exists.
        // SAFETY: single-threaded at this point; no other thread reads the environment yet.
        unsafe { std::env::set_var("SLINT_NO_MUDA", "1") };
    }
    match cfg.style.as_str() {
        "material" => material::app::run(cfg),
        "cosmic" => cosmic::app::run(cfg),
        _ => fluent::app::run(cfg),
    }
}
