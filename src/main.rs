//! Wyvencraft binary entry point.

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
