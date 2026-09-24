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
        std::process::exit(exit_code(&err));
    }
}

/// This machine has no GPU the game can run on. A contract with wc-launcher,
/// which matches this number (`exitNoVulkan` in `internal/gamesvc/exitcode.go`)
/// to tell the player why instead of showing a bare code. Never reuse it.
const EXIT_NO_VULKAN: i32 = 3;

/// Any other fatal error. A panic exits 101, Rust's own code.
const EXIT_FATAL: i32 = 1;

fn exit_code(err: &wyvencraft::app::AppError) -> i32 {
    match err {
        wyvencraft::app::AppError::NoVulkan(_) => EXIT_NO_VULKAN,
        _ => EXIT_FATAL,
    }
}
