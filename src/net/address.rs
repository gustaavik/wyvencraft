//! Turning what a player typed into somewhere to send packets.
//!
//! Deliberately its own module, because the two callers that need it — the
//! server-list probe and the connecting screen — must both do it *off* the
//! frame that draws the UI. [`resolve`] blocks: it is a DNS lookup, and a
//! server whose name no longer resolves would otherwise freeze the menu for as
//! long as the resolver takes to give up.

use std::net::{SocketAddr, ToSocketAddrs};

pub use wyven_net::DEFAULT_PORT;

/// Resolve `host:port` — or a bare `host`, taking [`DEFAULT_PORT`] — to one
/// address to try.
///
/// **Blocks.** Call it from a worker, never from `ui`.
pub fn resolve(input: &str) -> Result<SocketAddr, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter a server address".to_string());
    }
    // A bare IPv6 literal has colons of its own, so "contains a colon" would
    // read `::1` as a host with a port. Trying it as an address first settles
    // that without a parser: `::1` parses, `::1:25565` does not.
    let with_port = if trimmed.parse::<SocketAddr>().is_ok() {
        trimmed.to_string()
    } else if trimmed.parse::<std::net::IpAddr>().is_ok() {
        format!("{trimmed}:{DEFAULT_PORT}")
    } else if trimmed.rsplit(':').next().is_some_and(is_port) {
        trimmed.to_string()
    } else {
        format!("{trimmed}:{DEFAULT_PORT}")
    };

    with_port
        .to_socket_addrs()
        .map_err(|err| format!("Can't find {trimmed} ({err})"))?
        .next()
        .ok_or_else(|| format!("Can't find {trimmed}"))
}

/// Whether this trailing segment is a port number rather than part of a host.
fn is_port(segment: &str) -> bool {
    !segment.is_empty() && segment.parse::<u16>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_loopback_address_takes_the_default_port() {
        let addr = resolve("127.0.0.1").expect("resolves");
        assert_eq!(addr.port(), DEFAULT_PORT);
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
    }

    #[test]
    fn an_explicit_port_is_kept() {
        let addr = resolve(" 127.0.0.1:30000 ").expect("resolves");
        assert_eq!(addr.port(), 30_000);
    }

    /// `::1` is an address, not a host called `:` with a port. Splitting on the
    /// last colon without checking would send the player to `::1:25565`, which
    /// resolves to nothing at all.
    #[test]
    fn an_ipv6_literal_is_not_mistaken_for_a_host_and_port() {
        let addr = resolve("::1").expect("resolves");
        assert_eq!(addr.port(), DEFAULT_PORT);
        assert!(addr.is_ipv6());
    }

    #[test]
    fn an_empty_address_is_rejected_before_any_lookup() {
        assert!(resolve("   ").is_err());
    }

    #[test]
    fn an_unresolvable_host_reports_the_name_it_could_not_find() {
        let err = resolve("wyvencraft.invalid").expect_err("no such host");
        assert!(err.contains("wyvencraft.invalid"), "unhelpful error: {err}");
    }
}
