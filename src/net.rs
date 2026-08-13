//! LAN address detection and bind safety (PLAN.md §9: "Binds to LAN
//! interface only, token required, refuses public binds without an explicit
//! flag"). No dependencies — `std::net` covers all of it.

use std::net::{IpAddr, UdpSocket};

use anyhow::{anyhow, Result};

/// True if binding this address keeps the server off the public internet.
/// Loopback and link-local count: they are strictly *less* reachable than a
/// private LAN address, so anything that passes here is safe by the same
/// argument.
pub fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        // `Ipv6Addr::is_unique_local` is still unstable, so test fc00::/7 by
        // hand. Also accept loopback (::1) and link-local (fe80::/10).
        IpAddr::V6(v6) => {
            let o = v6.octets();
            v6.is_loopback() || (o[0] & 0xfe) == 0xfc || (o[0] == 0xfe && (o[1] & 0xc0) == 0x80)
        }
    }
}

/// The address of the interface that routes toward the default gateway —
/// i.e. the machine's LAN IP, which is what a phone on the same Wi-Fi has to
/// connect to.
///
/// UDP `connect()` sends no packets; it only asks the kernel to pick a route
/// and bind a local address, so this costs one syscall and works with no
/// network traffic and no dependency on an interface-enumeration crate.
pub fn detect_lan_ip() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // Any routable address works as a routing hint; nothing is sent to it.
    sock.connect("8.8.8.8:80").ok()?;
    Some(sock.local_addr().ok()?.ip())
}

/// Decides what address to bind, refusing anything publicly reachable unless
/// the user explicitly opted in.
///
/// An empty `lan_bind` means "figure it out": use the detected LAN address.
pub fn resolve_bind(lan_bind: &str, allow_public: bool) -> Result<IpAddr> {
    let trimmed = lan_bind.trim();
    if trimmed.is_empty() {
        let ip = detect_lan_ip().ok_or_else(|| {
            anyhow!(
                "could not detect a LAN address; set `lan_bind` in config.toml \
                 to this machine's LAN IP"
            )
        })?;
        if !is_private(&ip) && !allow_public {
            return Err(anyhow!(
                "detected address {ip} is not a private LAN address; set \
                 `lan_bind` explicitly, or `lan_allow_public = true` if you \
                 really mean to serve the public internet"
            ));
        }
        return Ok(ip);
    }

    let ip: IpAddr = trimmed
        .parse()
        .map_err(|_| anyhow!("`lan_bind` is not a valid IP address: {trimmed}"))?;
    let unspecified = ip.is_unspecified(); // 0.0.0.0 / :: = every interface
    if (unspecified || !is_private(&ip)) && !allow_public {
        return Err(anyhow!(
            "refusing to bind {ip}: it is not a private LAN address. Set \
             `lan_bind` to this machine's LAN IP, or set \
             `lan_allow_public = true` in config.toml to override"
        ));
    }
    Ok(ip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn private_ranges_are_private_and_public_ones_are_not() {
        assert!(is_private(&v4(192, 168, 1, 20)));
        assert!(is_private(&v4(10, 0, 0, 5)));
        assert!(is_private(&v4(172, 16, 4, 4)));
        assert!(is_private(&v4(127, 0, 0, 1)), "loopback counts as safe to bind");
        assert!(is_private(&v4(169, 254, 3, 3)), "link-local counts as safe to bind");
        assert!(!is_private(&v4(8, 8, 8, 8)));
        assert!(!is_private(&v4(172, 32, 0, 1)), "172.32 is outside 172.16/12");
    }

    #[test]
    fn explicit_private_bind_is_allowed() {
        let ip = resolve_bind("192.168.1.20", false).unwrap();
        assert_eq!(ip, v4(192, 168, 1, 20));
    }

    #[test]
    fn explicit_public_bind_is_refused_without_override() {
        let err = resolve_bind("8.8.8.8", false).unwrap_err().to_string();
        assert!(err.contains("lan_allow_public"), "error must name the override: {err}");
    }

    #[test]
    fn explicit_public_bind_is_allowed_with_override() {
        assert_eq!(resolve_bind("8.8.8.8", true).unwrap(), v4(8, 8, 8, 8));
    }

    #[test]
    fn wildcard_bind_needs_the_override_too() {
        // 0.0.0.0 means "every interface", which includes a public one if the
        // machine has one. PLAN.md §9: refuse public binds without a flag.
        assert!(resolve_bind("0.0.0.0", false).is_err());
        assert_eq!(resolve_bind("0.0.0.0", true).unwrap(), v4(0, 0, 0, 0));
    }

    #[test]
    fn garbage_bind_is_an_error_not_a_panic() {
        assert!(resolve_bind("not-an-ip", false).is_err());
    }
}
