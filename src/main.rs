//! Wyvencraft binary entry point.

// A release build on Windows is a GUI program: without this, a console window
// opens beside the game. Debug builds keep it so `cargo run` still shows logs.
// The launcher reads stderr through a pipe, which works either way.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    // Honour RUST_LOG; default to info for our crate.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wyvencraft=info"),
    )
    .init();

    if let Err(err) = wyvencraft::app::run() {
        log::error!("fatal: {err}");
        std::process::exit(1);
    }
}
