//! Build-time hooks.
//!
//! `auth::DEFAULT_AUTH_URL` reads `WYVEN_AUTH_URL` through `option_env!`, which
//! cargo does not track on its own — without this, changing the variable would
//! silently reuse a stale binary.

fn main() {
    println!("cargo::rerun-if-env-changed=WYVEN_AUTH_URL");
}
