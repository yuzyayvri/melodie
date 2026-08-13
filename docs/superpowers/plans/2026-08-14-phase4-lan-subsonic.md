# Phase 4 — LAN / OpenSubsonic Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Melodie serves its library over the LAN as a read-only OpenSubsonic API, so an off-the-shelf Subsonic Android client (and later Melodie's own companion app) can browse and stream it.

**Architecture:** A new `src/server.rs` runs `tiny_http` on two fixed worker threads, following the existing `spotisync::spawn(cfg, Arc<Db>, ...)` pattern. It reads the SQLite cache and the filesystem directly and holds no state of its own. Artist/album "folders" are computed per request by grouping `Db::list_tracks()`, so no schema change is needed. Responses are hand-written XML.

**Tech Stack:** Rust (stable, edition 2021), `tiny_http` 0.12, `md5` 0.8, `fastrand` 2, `qrcode` 0.14 (no default features), plus the crate's existing `rusqlite`, `lofty`, `fltk`, `anyhow`, `serde`.

**Spec:** `docs/superpowers/specs/2026-08-14-lan-subsonic-and-android-design.md`

## Global Constraints

- **No async runtime.** `tokio`/`async-std` are banned by PLAN.md §2. `tiny_http` is synchronous by design; keep it that way.
- **Resource budgets (PLAN.md §6) are hard requirements.** Specifically: nothing loads a whole audio file into memory — `stream` must seek and stream. Idle CPU stays at 0.0% — no polling loops, no timers; threads block in `Server::recv()`.
- **The filesystem is the source of truth (PLAN.md §3).** The server reads the DB cache for metadata and opens files directly for bytes. It never writes to the library, never writes to `tracks`/`playlists`, and exposes no mutating endpoints.
- **`SCHEMA_VERSION` must not change.** No new tables, no new columns. `db.rs::migrate` drops all data on a version bump, so a schema change would wipe the user's library cache for no reason. Artist/album IDs are computed, not stored.
- **Subsonic API version reported: `1.16.1`.** Server type string: `melodie`.
- **One message enum rule (CLAUDE.md).** Do NOT call `fltk::app::channel::<T>()` for anything new. If the server ever needs to talk to the UI, add a variant to `app::Message`. (This plan's server does not need to.)
- **Cargo feature `lan` is in `default`.** The feature gates *compilation*; `config.toml`'s `lan_enabled` (default `false`) gates whether the server actually binds. "Off by default" in PLAN.md §7 means the runtime behaviour, not that the user must recompile.
- **This repo is not a git repository.** Where the standard plan format says "commit", run the verification command instead. Do not run `git init` — it was not requested.
- **Test convention:** inline `#[cfg(test)] mod tests` at the bottom of the file under test (matches `db.rs`, `matcher.rs`, `playlist.rs`). There is no `tests/` directory; do not create one.
- **Every task ends with `cargo test` passing** (not just the new test).

---

### Task 1: Config surface + LAN address safety

Adds the LAN settings to `config.toml` and the one piece of logic that decides *which address is safe to bind*. PLAN.md §9 requires the server to bind a LAN interface only and refuse a public bind without an explicit override.

**Files:**
- Modify: `Cargo.toml` (add deps + `lan` feature)
- Modify: `src/config.rs` (add fields, defaults, `lan_token` generation)
- Create: `src/net.rs` (address detection + safety decision, with tests)
- Modify: `src/main.rs` (add `mod net;`)

**Interfaces:**
- Consumes: `crate::config::Config` (existing struct, `#[serde(default)]`).
- Produces:
  - `Config.lan_enabled: bool`, `Config.lan_port: u16`, `Config.lan_bind: String`, `Config.lan_token: String`, `Config.lan_allow_public: bool`
  - `Config::ensure_lan_token(&mut self) -> bool` — generates a token if empty, returns true if it changed.
  - `net::is_private(ip: &IpAddr) -> bool`
  - `net::detect_lan_ip() -> Option<IpAddr>`
  - `net::resolve_bind(lan_bind: &str, allow_public: bool) -> anyhow::Result<IpAddr>`

- [ ] **Step 1: Add dependencies and the `lan` feature to `Cargo.toml`**

Add a `[features]` section (the file currently has none) directly above `[dependencies]`:

```toml
[features]
# Compiled in by default; the server still only binds when config.toml sets
# `lan_enabled = true` (PLAN.md §7: "off by default" = runtime, not a recompile).
default = ["lan"]
lan = ["dep:tiny_http", "dep:md5", "dep:fastrand", "dep:qrcode"]
```

Add to `[dependencies]`:

```toml
# --- feature "lan" (PLAN.md §7 Tier 0) ---
# Synchronous HTTP. Deliberately not axum/hyper: no async runtime in this
# binary at all (PLAN.md §2).
tiny_http = { version = "0.12", optional = true }
# Subsonic's salted-token auth is defined as md5(password + salt).
md5 = { version = "0.8", optional = true }
# LAN token generation. Tiny, zero-dependency alternative to `rand`.
fastrand = { version = "2", optional = true }
# Pairing QR. default-features off drops the `image`/`svg` renderers (and
# their dependency trees) — Melodie renders the bit matrix itself.
qrcode = { version = "0.14", default-features = false, optional = true }
```

- [ ] **Step 2: Write the failing tests for `src/net.rs`**

Create `src/net.rs` containing only this test module for now (the functions come next, so it must fail to compile — that is the expected failure):

```rust
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
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --lib net::`
Expected: FAIL — compile error, `cannot find function `is_private` in this scope`.

(If `cargo test --lib` reports "no library targets found", use `cargo test net::` — this crate is a single binary target.)

- [ ] **Step 4: Implement `src/net.rs`**

Put this *above* the test module already in the file:

```rust
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
```

- [ ] **Step 5: Register the module**

In `src/main.rs`, the `mod` list at the top is alphabetical. Add `net` between `mediakeys` and `playlist`:

```rust
mod mediakeys;
mod net;
mod playlist;
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test net::`
Expected: PASS, 6 tests.

- [ ] **Step 7: Add the config fields**

In `src/config.rs`, add to the `Config` struct (after `default_volume`):

```rust
    /// Serve the library over the LAN (PLAN.md §7 Tier 0). Off by default:
    /// a music player should not open a socket nobody asked for.
    pub lan_enabled: bool,
    /// Port for the Subsonic API. 4533 is the Navidrome/Subsonic convention,
    /// so clients often pre-fill it.
    pub lan_port: u16,
    /// Address to bind. Empty = auto-detect this machine's LAN IP. A public
    /// address is refused unless `lan_allow_public` is set (PLAN.md §9).
    pub lan_bind: String,
    /// Shared secret every request must present. Generated on first use;
    /// there are no accounts (PLAN.md §7: "no login, no account").
    pub lan_token: String,
    /// Escape hatch for binding a non-private address. Leave false.
    pub lan_allow_public: bool,
```

And to `impl Default for Config`, after `default_volume: 0.8,`:

```rust
            lan_enabled: false,
            lan_port: 4533,
            lan_bind: String::new(),
            lan_token: String::new(),
            lan_allow_public: false,
```

- [ ] **Step 8: Write the failing test for token generation**

Add to the bottom of `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_lan_token_generates_once_and_is_stable() {
        let mut cfg = Config::default();
        assert!(cfg.lan_token.is_empty());

        assert!(cfg.ensure_lan_token(), "first call generates and reports a change");
        let first = cfg.lan_token.clone();
        assert!(first.len() >= 24, "token must be long enough to not be guessable: {first}");
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric()), "URL-safe: {first}");

        assert!(!cfg.ensure_lan_token(), "second call is a no-op");
        assert_eq!(cfg.lan_token, first);
    }
}
```

- [ ] **Step 9: Run the test to verify it fails**

Run: `cargo test config::`
Expected: FAIL — `no method named `ensure_lan_token` found`.

- [ ] **Step 10: Implement `ensure_lan_token`**

Add to `impl Config` in `src/config.rs`, after `ensure_dirs`:

```rust
    /// Generates `lan_token` if it is unset. Returns true when it changed,
    /// so the caller knows to `save()`.
    #[cfg(feature = "lan")]
    pub fn ensure_lan_token(&mut self) -> bool {
        if !self.lan_token.is_empty() {
            return false;
        }
        // 32 alphanumeric chars ~= 190 bits. It travels in a URL query
        // string, so keep it to characters that need no escaping.
        self.lan_token = (0..32).map(|_| fastrand::alphanumeric()).collect();
        true
    }
```

Gate the test module the same way so a `--no-default-features` build still compiles — change the test module header to:

```rust
#[cfg(all(test, feature = "lan"))]
mod tests {
```

- [ ] **Step 11: Run the tests**

Run: `cargo test config::`
Expected: PASS, 1 test.

- [ ] **Step 12: Verify the whole suite and both feature configurations still build**

Run: `cargo test`
Expected: PASS, all pre-existing tests plus the 7 new ones.

Run: `cargo build --no-default-features`
Expected: builds clean (proves the `lan` gating is correct).

---

### Task 2: Server skeleton — routing, auth, XML envelope, `ping`

The smallest end-to-end server: it binds, authenticates, and answers `ping.view` and `getLicense.view`. Everything after this task is more handlers behind the same routing.

**Files:**
- Create: `src/server.rs`
- Modify: `src/main.rs` (add gated `mod server;`)

**Interfaces:**
- Consumes: `net::resolve_bind`, `Config.lan_*`, `Arc<Db>`.
- Produces:
  - `server::spawn(cfg: &Config, db: Arc<Db>) -> anyhow::Result<ServerHandle>`
  - `ServerHandle { pub addr: std::net::SocketAddr }` with `ServerHandle::stop(self)`
  - `server::escape_xml(s: &str) -> String`
  - `server::percent_decode(s: &str) -> String`
  - `server::parse_query(url: &str) -> HashMap<String, String>`
  - `server::method_of(url: &str) -> &str`
  - `server::check_auth(token: &str, q: &HashMap<String, String>) -> bool`
  - `server::ok_envelope(body: &str) -> String`
  - `server::error_envelope(code: u32, message: &str) -> String`

- [ ] **Step 1: Write the failing unit tests**

Create `src/server.rs` with only this content (it must fail to compile — that is the expected failure):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_xml_covers_all_five_entities() {
        assert_eq!(escape_xml("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&apos;f");
        assert_eq!(escape_xml("plain"), "plain");
    }

    #[test]
    fn percent_decode_handles_escapes_plus_and_malformed_input() {
        assert_eq!(percent_decode("Bon%20Iver"), "Bon Iver");
        assert_eq!(percent_decode("Simon+%26+Garfunkel"), "Simon & Garfunkel");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        // A stray % must not panic or eat the rest of the string.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn parse_query_splits_pairs_and_tolerates_junk() {
        let q = parse_query("/rest/stream.view?id=tr7&u=melodie&p=secret");
        assert_eq!(q.get("id").unwrap(), "tr7");
        assert_eq!(q.get("u").unwrap(), "melodie");
        assert_eq!(q.get("p").unwrap(), "secret");

        assert!(parse_query("/rest/ping.view").is_empty());
        // A valueless key is present but empty, not missing.
        assert_eq!(parse_query("/x?flag").get("flag").unwrap(), "");
    }

    #[test]
    fn method_of_strips_path_and_view_suffix() {
        assert_eq!(method_of("/rest/ping.view?u=a"), "ping");
        assert_eq!(method_of("/rest/getMusicDirectory.view"), "getMusicDirectory");
        // OpenSubsonic clients may omit the .view suffix entirely.
        assert_eq!(method_of("/rest/getPlaylists"), "getPlaylists");
    }

    fn query_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn check_auth_accepts_salted_token_and_plaintext() {
        let token = "s3cret";
        // t = md5(password + salt), the standard Subsonic scheme.
        let salt = "abc";
        let t = format!("{:x}", md5::compute(format!("{token}{salt}").as_bytes()));
        assert!(check_auth(token, &query_of(&[("t", &t), ("s", salt)])));
        assert!(check_auth(token, &query_of(&[("p", token)])));
        // Hex-encoded plaintext, the "enc:" form older clients send.
        let enc = format!("enc:{}", token.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>());
        assert!(check_auth(token, &query_of(&[("p", &enc)])));
    }

    #[test]
    fn check_auth_rejects_wrong_and_missing_credentials() {
        let token = "s3cret";
        assert!(!check_auth(token, &query_of(&[("p", "wrong")])));
        assert!(!check_auth(token, &query_of(&[("t", "deadbeef"), ("s", "abc")])));
        assert!(!check_auth(token, &query_of(&[])));
        assert!(!check_auth(token, &query_of(&[("t", "x")])), "t without s is not a credential");
        // An unset token must never authenticate anything.
        assert!(!check_auth("", &query_of(&[("p", "")])));
    }

    #[test]
    fn envelopes_are_well_formed() {
        let ok = ok_envelope("<license valid=\"true\"/>");
        assert!(ok.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(ok.contains("status=\"ok\""));
        assert!(ok.contains("version=\"1.16.1\""));
        assert!(ok.contains("<license valid=\"true\"/>"));
        assert!(ok.trim_end().ends_with("</subsonic-response>"));

        let err = error_envelope(40, "Wrong username or password");
        assert!(err.contains("status=\"failed\""));
        assert!(err.contains("code=\"40\""));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test server::`
Expected: FAIL — `cannot find function `escape_xml` in this scope` (and similar).

- [ ] **Step 3: Implement the pure helpers and the envelope**

Insert above the test module in `src/server.rs`:

```rust
//! Read-only OpenSubsonic subset (PLAN.md §7, Tier 0), so any existing
//! Subsonic client — and Melodie's own Android companion — can browse and
//! stream the library over the LAN.
//!
//! Deliberately hand-written XML rather than a serialization crate: the
//! whole response vocabulary is a dozen elements, and the alternative is a
//! dependency plus a derive layer for something `format!` already does.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::config::Config;
use crate::db::Db;
use crate::net;

/// The Subsonic protocol version we claim. 1.16.1 is the last Subsonic
/// release's version and what OpenSubsonic servers report.
const API_VERSION: &str = "1.16.1";

pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 3 <= bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(v) => {
                    out.push(v);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_query(url: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some((_, query)) = url.split_once('?') else {
        return map;
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

/// `/rest/getMusicDirectory.view?id=1` -> `getMusicDirectory`.
pub fn method_of(url: &str) -> &str {
    let path = url.split('?').next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    last.strip_suffix(".view").unwrap_or(last)
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Subsonic auth: either `t=md5(password+salt)&s=salt`, or `p=password`
/// (optionally hex-encoded behind an `enc:` prefix). There are no user
/// accounts — `u` is accepted and ignored; the token is the whole secret.
///
/// ponytail: plain `==` comparison, not constant-time. The attacker model
/// here is a device on the user's home LAN, and the response is a
/// read-only music listing; timing-attacking an md5 compare over Wi-Fi to
/// obtain it is not a threat worth a dependency.
pub fn check_auth(token: &str, q: &HashMap<String, String>) -> bool {
    if token.is_empty() {
        return false;
    }
    if let (Some(t), Some(s)) = (q.get("t"), q.get("s")) {
        let expected = format!("{:x}", md5::compute(format!("{token}{s}").as_bytes()));
        return t.eq_ignore_ascii_case(&expected);
    }
    match q.get("p") {
        Some(p) => {
            let plain = match p.strip_prefix("enc:") {
                Some(hex) => match hex_decode(hex).map(String::from_utf8) {
                    Some(Ok(decoded)) => decoded,
                    _ => return false,
                },
                None => p.clone(),
            };
            plain == token
        }
        None => false,
    }
}

pub fn ok_envelope(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <subsonic-response xmlns=\"http://subsonic.org/restapi\" status=\"ok\" \
         version=\"{API_VERSION}\" type=\"melodie\" serverVersion=\"{}\" openSubsonic=\"true\">\
         {body}</subsonic-response>",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn error_envelope(code: u32, message: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <subsonic-response xmlns=\"http://subsonic.org/restapi\" status=\"failed\" \
         version=\"{API_VERSION}\" type=\"melodie\">\
         <error code=\"{code}\" message=\"{}\"/></subsonic-response>",
        escape_xml(message)
    )
}
```

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test server::`
Expected: PASS, 7 tests.

- [ ] **Step 5: Write the failing integration test**

Add these tests *inside* the existing `mod tests` in `src/server.rs`:

```rust
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::path::Path;

    /// Minimal HTTP/1.1 GET over a raw socket. Avoids adding an HTTP client
    /// dependency just for tests; returns (status_line, headers, body).
    fn http_get(addr: SocketAddr, path: &str) -> (String, String, Vec<u8>) {
        http_get_with(addr, path, &[])
    }

    fn http_get_with(
        addr: SocketAddr,
        path: &str,
        extra_headers: &[(&str, &str)],
    ) -> (String, String, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        let mut req = format!("GET {path} HTTP/1.1\r\nHost: melodie\r\nConnection: close\r\n");
        for (k, v) in extra_headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).expect("write request");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).expect("read response");
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("response has a header/body separator");
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let body = raw[split + 4..].to_vec();
        let (status, headers) = head.split_once("\r\n").unwrap_or((head.as_str(), ""));
        (status.to_string(), headers.to_string(), body)
    }

    /// A server on an ephemeral port with an empty in-memory library.
    fn test_server() -> (ServerHandle, Config, Arc<Db>) {
        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0; // let the OS pick a free port
        cfg.lan_token = "testtoken".to_string();
        let handle = spawn(&cfg, db.clone()).expect("server starts");
        (handle, cfg, db)
    }

    #[test]
    fn ping_requires_credentials_and_succeeds_with_them() {
        let (handle, _cfg, _db) = test_server();

        let (status, _h, body) = http_get(handle.addr, "/rest/ping.view?u=melodie&p=testtoken&c=test");
        assert!(status.contains("200"), "status was {status}");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("status=\"ok\""), "body was {body}");

        let (_s, _h, body) = http_get(handle.addr, "/rest/ping.view?u=melodie&p=nope&c=test");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("status=\"failed\""), "body was {body}");
        assert!(body.contains("code=\"40\""), "wrong-credentials is code 40: {body}");

        handle.stop();
    }

    #[test]
    fn get_license_reports_a_valid_license() {
        let (handle, _cfg, _db) = test_server();
        let (_s, _h, body) = http_get(handle.addr, "/rest/getLicense.view?u=m&p=testtoken&c=test");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("<license valid=\"true\""), "body was {body}");
        handle.stop();
    }

    #[test]
    fn unknown_method_is_a_subsonic_error_not_a_404() {
        let (handle, _cfg, _db) = test_server();
        let (_s, _h, body) = http_get(handle.addr, "/rest/getPodcasts.view?u=m&p=testtoken&c=test");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("status=\"failed\""), "body was {body}");
        assert!(body.contains("code=\"70\""), "unsupported method is code 70: {body}");
        handle.stop();
    }
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test server::`
Expected: FAIL — `cannot find function `spawn`` / `cannot find type `ServerHandle``.

- [ ] **Step 7: Implement the server loop and the two handlers**

Append to `src/server.rs`, above the test module:

```rust
/// A running server. Dropping this does not stop it; call `stop()`.
pub struct ServerHandle {
    /// The address actually bound — with `lan_port = 0` this is the
    /// OS-assigned port, which is what tests need.
    pub addr: SocketAddr,
    server: Arc<Server>,
    threads: Vec<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn stop(self) {
        // Wakes every thread blocked in `recv()` so they can exit.
        self.server.unblock();
        for t in self.threads {
            let _ = t.join();
        }
    }
}

/// Number of request-serving threads.
///
/// ponytail: fixed at 2, not a pool. One long `stream` response must not
/// block a concurrent `getCoverArt`, and this is a single-user LAN server —
/// raise it only if real concurrent clients ever appear.
const WORKER_THREADS: usize = 2;

/// Binds and starts serving. Binding happens synchronously so a bad address
/// or a taken port is reported to the caller instead of vanishing into a
/// thread.
pub fn spawn(cfg: &Config, db: Arc<Db>) -> Result<ServerHandle> {
    let ip = net::resolve_bind(&cfg.lan_bind, cfg.lan_allow_public)?;
    let addr = SocketAddr::new(ip, cfg.lan_port);
    let server = Server::http(addr)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("binding {addr} for the LAN server"))?;
    let server = Arc::new(server);
    let bound = server
        .server_addr()
        .to_ip()
        .context("LAN server bound a non-IP address")?;

    let mut threads = Vec::with_capacity(WORKER_THREADS);
    for _ in 0..WORKER_THREADS {
        let server = server.clone();
        let db = db.clone();
        let token = cfg.lan_token.clone();
        let cfg = cfg.clone();
        threads.push(std::thread::spawn(move || {
            // Blocks in recv(); no polling, no timer, 0% idle CPU (PLAN.md §6).
            while let Ok(request) = server.recv() {
                handle(request, &cfg, &db, &token);
            }
        }));
    }

    Ok(ServerHandle { addr: bound, server, threads })
}

fn xml_response(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/xml; charset=utf-8"[..])
        .expect("static header parses");
    Response::from_data(body.into_bytes()).with_header(header)
}

fn handle(request: Request, cfg: &Config, db: &Db, token: &str) {
    let url = request.url().to_string();
    let query = parse_query(&url);

    if !check_auth(token, &query) {
        let _ = request.respond(xml_response(error_envelope(40, "Wrong username or password")));
        return;
    }

    let body = match method_of(&url) {
        "ping" => ok_envelope(""),
        "getLicense" => ok_envelope("<license valid=\"true\"/>"),
        _ => error_envelope(70, "Requested data was not found"),
    };
    let _ = request.respond(xml_response(body));

    // Silence unused-parameter warnings until later tasks use these.
    let _ = (cfg, db);
}
```

- [ ] **Step 8: Register the module**

In `src/main.rs`, add after `mod playlist;`:

```rust
#[cfg(feature = "lan")]
mod server;
```

- [ ] **Step 9: Run the tests**

Run: `cargo test server::`
Expected: PASS, 10 tests.

- [ ] **Step 10: Verify the full suite and the no-feature build**

Run: `cargo test`
Expected: PASS.

Run: `cargo build --no-default-features`
Expected: builds clean.

---

### Task 3: Browsing — artists, albums, directories

Implements the two browse families. Different real clients hard-require one or the other, and both are the same grouping of `Db::list_tracks()` rendered two ways.

**Files:**
- Modify: `src/server.rs`

**Interfaces:**
- Consumes: `Db::list_tracks() -> Result<Vec<Track>>` (already exists; ordered by artist, album, track_no, title), `escape_xml`, `ok_envelope`, `error_envelope`.
- Produces:
  - `server::artist_id(artist: &str) -> String` (`ar` + 16 hex chars)
  - `server::album_id(artist: &str, album: &str) -> String` (`al` + 16 hex chars)
  - `server::track_id(id: i64) -> String` (`tr` + decimal id)
  - `server::parse_track_id(id: &str) -> Option<i64>`
  - `server::Album { id, name, artist, artist_id, tracks: Vec<Track> }`
  - `server::ArtistGroup { id, name, albums: Vec<Album> }`
  - `server::group_library(tracks: Vec<Track>) -> Vec<ArtistGroup>`
  - `server::song_xml(t: &Track, parent_album: &Album) -> String`

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `src/server.rs`:

```rust
    fn track(id: i64, artist: &str, album: &str, title: &str, no: i64) -> crate::db::Track {
        crate::db::Track {
            id,
            path: format!("/music/{artist}/{album}/{title}.mp3"),
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            track_no: Some(no),
            duration_ms: 210_000,
            mtime: 1,
            size: 3_400_000,
            added_at: 1,
        }
    }

    #[test]
    fn ids_are_stable_prefixed_and_distinct() {
        assert_eq!(artist_id("Boards of Canada"), artist_id("Boards of Canada"));
        assert_ne!(artist_id("Boards of Canada"), artist_id("Board of Canada"));
        assert!(artist_id("x").starts_with("ar"));
        assert!(album_id("a", "b").starts_with("al"));
        // The separator must keep ("ab","c") from colliding with ("a","bc").
        assert_ne!(album_id("ab", "c"), album_id("a", "bc"));
        assert_eq!(track_id(7), "tr7");
        assert_eq!(parse_track_id("tr7"), Some(7));
        assert_eq!(parse_track_id("al1234"), None);
        assert_eq!(parse_track_id("trxyz"), None);
    }

    #[test]
    fn group_library_nests_albums_under_artists() {
        let tracks = vec![
            track(1, "Aphex Twin", "SAW II", "Rhubarb", 1),
            track(2, "Aphex Twin", "SAW II", "Curtains", 2),
            track(3, "Aphex Twin", "Drukqs", "Avril 14th", 1),
            track(4, "Boards of Canada", "Music Has the Right", "Roygbiv", 1),
        ];
        let groups = group_library(tracks);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Aphex Twin");
        assert_eq!(groups[0].albums.len(), 2);
        assert_eq!(groups[0].albums[0].tracks.len(), 2);
        assert_eq!(groups[1].albums.len(), 1);
        assert_eq!(groups[0].albums[0].artist_id, groups[0].id);
    }

    #[test]
    fn song_xml_escapes_metadata_and_reports_a_content_type() {
        let t = track(9, "AC/DC", "Back in Black", "Rock & Roll <live>", 3);
        let groups = group_library(vec![t.clone()]);
        let album = &groups[0].albums[0];
        let xml = song_xml(&t, album);
        assert!(xml.contains("title=\"Rock &amp; Roll &lt;live&gt;\""), "{xml}");
        assert!(xml.contains("id=\"tr9\""), "{xml}");
        assert!(xml.contains("contentType=\"audio/mpeg\""), "{xml}");
        assert!(xml.contains("suffix=\"mp3\""), "{xml}");
        assert!(xml.contains("duration=\"210\""), "seconds, not ms: {xml}");
        assert!(xml.contains(&format!("albumId=\"{}\"", album.id)), "{xml}");
    }
```

And add this integration test, which needs a library on disk:

```rust
    /// A DB with two real tracks. Returns the temp dir so the caller keeps it
    /// alive — dropping it deletes the files.
    fn seeded_server() -> (ServerHandle, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("melodie-server-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Real bytes on disk so `stream` has something to serve.
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, vec![0xAAu8; 5000]).unwrap();
        std::fs::write(&b, vec![0xBBu8; 4000]).unwrap();

        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        for (path, artist, album, title, no) in [
            (&a, "Aphex Twin", "SAW II", "Rhubarb", 1i64),
            (&b, "Boards of Canada", "Geogaddi", "Dandelion", 1),
        ] {
            db.upsert_track(&crate::db::NewTrack {
                path: path.to_str().unwrap(),
                title,
                artist,
                album,
                track_no: Some(no),
                duration_ms: 210_000,
                mtime: 1,
                size: std::fs::metadata(path).unwrap().len() as i64,
            })
            .unwrap();
        }

        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0;
        cfg.lan_token = "testtoken".to_string();
        let handle = spawn(&cfg, db).expect("server starts");
        (handle, dir)
    }

    #[test]
    fn browse_endpoints_expose_the_library() {
        let (handle, dir) = seeded_server();
        let auth = "u=m&p=testtoken&c=test";

        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/getIndexes.view?{auth}"));
        let body = String::from_utf8_lossy(&body).into_owned();
        assert!(body.contains("Aphex Twin"), "{body}");
        assert!(body.contains("Boards of Canada"), "{body}");
        assert!(body.contains("<index name=\"A\">"), "indexed by first letter: {body}");

        let (_s, _h, body2) = http_get(handle.addr, &format!("/rest/getArtists.view?{auth}"));
        let body2 = String::from_utf8_lossy(&body2).into_owned();
        assert!(body2.contains("albumCount=\"1\""), "{body2}");

        // Drill into the first artist via the id the listing just handed us.
        let aphex = artist_id("Aphex Twin");
        let (_s, _h, body3) =
            http_get(handle.addr, &format!("/rest/getMusicDirectory.view?{auth}&id={aphex}"));
        let body3 = String::from_utf8_lossy(&body3).into_owned();
        assert!(body3.contains("SAW II"), "{body3}");
        assert!(body3.contains("isDir=\"true\""), "album children are directories: {body3}");

        let saw = album_id("Aphex Twin", "SAW II");
        let (_s, _h, body4) = http_get(handle.addr, &format!("/rest/getAlbum.view?{auth}&id={saw}"));
        let body4 = String::from_utf8_lossy(&body4).into_owned();
        assert!(body4.contains("Rhubarb"), "{body4}");
        assert!(body4.contains("<song "), "{body4}");

        let (_s, _h, body5) =
            http_get(handle.addr, &format!("/rest/getMusicDirectory.view?{auth}&id={saw}"));
        let body5 = String::from_utf8_lossy(&body5).into_owned();
        assert!(body5.contains("Rhubarb"), "album directory lists songs: {body5}");

        let (_s, _h, body6) = http_get(handle.addr, &format!("/rest/getMusicFolders.view?{auth}"));
        assert!(String::from_utf8_lossy(&body6).contains("musicFolder"), "{body6:?}");

        handle.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test server::`
Expected: FAIL — `cannot find function `artist_id``.

- [ ] **Step 3: Implement grouping, IDs, and the song element**

Append to `src/server.rs`, above the test module:

```rust
// --------------------------------------------------------------- identity

/// FNV-1a. Not cryptographic and does not need to be: these IDs are opaque
/// handles a client echoes back, and the only requirement is that the same
/// artist/album string maps to the same ID across requests and restarts.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn artist_id(artist: &str) -> String {
    format!("ar{:016x}", fnv1a(artist))
}

pub fn album_id(artist: &str, album: &str) -> String {
    // \u{1} can't occur in a tag, so ("ab","c") and ("a","bc") can't collide.
    format!("al{:016x}", fnv1a(&format!("{artist}\u{1}{album}")))
}

pub fn track_id(id: i64) -> String {
    format!("tr{id}")
}

pub fn parse_track_id(id: &str) -> Option<i64> {
    id.strip_prefix("tr")?.parse().ok()
}

// --------------------------------------------------------------- grouping

pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub artist_id: String,
    pub tracks: Vec<crate::db::Track>,
}

impl Album {
    fn duration_secs(&self) -> i64 {
        self.tracks.iter().map(|t| t.duration_ms / 1000).sum()
    }
}

pub struct ArtistGroup {
    pub id: String,
    pub name: String,
    pub albums: Vec<Album>,
}

/// Folds the track list into artists -> albums.
///
/// `Db::list_tracks` already orders by artist, album, track_no, title, so
/// this is a linear pass, not a sort or a hash map.
pub fn group_library(tracks: Vec<crate::db::Track>) -> Vec<ArtistGroup> {
    let mut artists: Vec<ArtistGroup> = Vec::new();
    for track in tracks {
        if artists.last().map(|a| a.name != track.artist).unwrap_or(true) {
            artists.push(ArtistGroup {
                id: artist_id(&track.artist),
                name: track.artist.clone(),
                albums: Vec::new(),
            });
        }
        let artist = artists.last_mut().expect("just pushed");
        if artist.albums.last().map(|al| al.name != track.album).unwrap_or(true) {
            artist.albums.push(Album {
                id: album_id(&track.artist, &track.album),
                name: track.album.clone(),
                artist: track.artist.clone(),
                artist_id: artist.id.clone(),
                tracks: Vec::new(),
            });
        }
        artist.albums.last_mut().expect("just pushed").tracks.push(track);
    }
    artists
}

fn extension_of(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn content_type_for(ext: &str) -> &'static str {
    match ext {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" | "mp4" | "m4b" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// The `<song>` element, shared by getAlbum, getPlaylist and
/// getMusicDirectory (Subsonic uses `<child>` for the last one, but the
/// attribute set is identical, so the caller just renames the tag).
pub fn song_xml(t: &crate::db::Track, album: &Album) -> String {
    let ext = extension_of(&t.path);
    let track_attr = match t.track_no {
        Some(n) => format!(" track=\"{n}\""),
        None => String::new(),
    };
    format!(
        "<song id=\"{id}\" parent=\"{album_id}\" isDir=\"false\" title=\"{title}\" \
         album=\"{album}\" artist=\"{artist}\"{track_attr} duration=\"{duration}\" \
         size=\"{size}\" suffix=\"{suffix}\" contentType=\"{ctype}\" \
         albumId=\"{album_id}\" artistId=\"{artist_id}\" coverArt=\"{id}\" type=\"music\"/>",
        id = track_id(t.id),
        album_id = album.id,
        artist_id = album.artist_id,
        title = escape_xml(&t.title),
        album = escape_xml(&t.album),
        artist = escape_xml(&t.artist),
        duration = t.duration_ms / 1000,
        size = t.size,
        suffix = escape_xml(&ext),
        ctype = content_type_for(&ext),
    )
}

/// The bucket letter a name sorts under in `getIndexes`/`getArtists`.
fn index_letter(name: &str) -> String {
    match name.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        Some(_) => "#".to_string(),
        None => "#".to_string(),
    }
}

fn indexes_xml(groups: &[ArtistGroup], with_album_count: bool) -> String {
    let mut out = String::new();
    let mut current = String::new();
    for artist in groups {
        let letter = index_letter(&artist.name);
        if letter != current {
            if !current.is_empty() {
                out.push_str("</index>");
            }
            out.push_str(&format!("<index name=\"{}\">", escape_xml(&letter)));
            current = letter;
        }
        let count = if with_album_count {
            format!(" albumCount=\"{}\"", artist.albums.len())
        } else {
            String::new()
        };
        out.push_str(&format!(
            "<artist id=\"{}\" name=\"{}\"{count}/>",
            artist.id,
            escape_xml(&artist.name)
        ));
    }
    if !current.is_empty() {
        out.push_str("</index>");
    }
    out
}

fn album_element(album: &Album, tag: &str) -> String {
    format!(
        "<{tag} id=\"{id}\" name=\"{name}\" title=\"{name}\" artist=\"{artist}\" \
         artistId=\"{artist_id}\" parent=\"{artist_id}\" isDir=\"true\" \
         songCount=\"{songs}\" duration=\"{duration}\" coverArt=\"{id}\"/>",
        id = album.id,
        name = escape_xml(&album.name),
        artist = escape_xml(&album.artist),
        artist_id = album.artist_id,
        songs = album.tracks.len(),
        duration = album.duration_secs(),
    )
}

// --------------------------------------------------------------- handlers

fn handle_browse(method: &str, query: &HashMap<String, String>, db: &Db) -> Option<String> {
    let groups = || -> Vec<ArtistGroup> {
        group_library(db.list_tracks().unwrap_or_else(|e| {
            eprintln!("melodie: LAN server could not read the library: {e:#}");
            Vec::new()
        }))
    };

    match method {
        "getMusicFolders" => Some(ok_envelope(
            "<musicFolders><musicFolder id=\"0\" name=\"Melodie\"/></musicFolders>",
        )),

        "getIndexes" => Some(ok_envelope(&format!(
            "<indexes lastModified=\"0\" ignoredArticles=\"The El La Los Las Le Les\">{}</indexes>",
            indexes_xml(&groups(), false)
        ))),

        "getArtists" => Some(ok_envelope(&format!(
            "<artists ignoredArticles=\"The El La Los Las Le Les\">{}</artists>",
            indexes_xml(&groups(), true)
        ))),

        "getArtist" => {
            let id = query.get("id")?;
            let all = groups();
            let artist = all.iter().find(|a| &a.id == id)?;
            let albums: String = artist.albums.iter().map(|al| album_element(al, "album")).collect();
            Some(ok_envelope(&format!(
                "<artist id=\"{}\" name=\"{}\" albumCount=\"{}\">{albums}</artist>",
                artist.id,
                escape_xml(&artist.name),
                artist.albums.len()
            )))
        }

        "getAlbum" => {
            let id = query.get("id")?;
            let all = groups();
            let album = all.iter().flat_map(|a| &a.albums).find(|al| &al.id == id)?;
            let songs: String = album.tracks.iter().map(|t| song_xml(t, album)).collect();
            Some(ok_envelope(&format!(
                "<album id=\"{id}\" name=\"{name}\" artist=\"{artist}\" artistId=\"{artist_id}\" \
                 songCount=\"{count}\" duration=\"{duration}\" coverArt=\"{id}\">{songs}</album>",
                name = escape_xml(&album.name),
                artist = escape_xml(&album.artist),
                artist_id = album.artist_id,
                count = album.tracks.len(),
                duration = album.duration_secs(),
            )))
        }

        // One endpoint, two meanings: an artist id lists its albums, an album
        // id lists its songs. That is what the folder-browsing family is.
        "getMusicDirectory" => {
            let id = query.get("id")?;
            let all = groups();
            if let Some(artist) = all.iter().find(|a| &a.id == id) {
                let children: String =
                    artist.albums.iter().map(|al| album_element(al, "child")).collect();
                return Some(ok_envelope(&format!(
                    "<directory id=\"{}\" name=\"{}\">{children}</directory>",
                    artist.id,
                    escape_xml(&artist.name)
                )));
            }
            let album = all.iter().flat_map(|a| &a.albums).find(|al| &al.id == id)?;
            let children: String = album
                .tracks
                .iter()
                .map(|t| song_xml(t, album).replacen("<song ", "<child ", 1))
                .collect();
            Some(ok_envelope(&format!(
                "<directory id=\"{}\" parent=\"{}\" name=\"{}\">{children}</directory>",
                album.id,
                album.artist_id,
                escape_xml(&album.name)
            )))
        }

        _ => None,
    }
}
```

- [ ] **Step 4: Route the new methods**

In `fn handle`, replace the `match method_of(&url)` block with:

```rust
    let method = method_of(&url);
    let body = match method {
        "ping" => ok_envelope(""),
        "getLicense" => ok_envelope("<license valid=\"true\"/>"),
        _ => match handle_browse(method, &query, db) {
            Some(body) => body,
            // A `None` from a handler it *does* own means the id was bad;
            // for a method it doesn't own it means unsupported. Both are
            // "not found" to a Subsonic client (code 70).
            None => error_envelope(70, "Requested data was not found"),
        },
    };
    let _ = request.respond(xml_response(body));

    let _ = cfg;
```

- [ ] **Step 5: Run the tests**

Run: `cargo test server::`
Expected: PASS, 14 tests.

- [ ] **Step 6: Verify the full suite**

Run: `cargo test`
Expected: PASS.

---

### Task 4: Playlists

**Files:**
- Modify: `src/server.rs`

**Interfaces:**
- Consumes: `Db::list_playlists() -> Result<Vec<Playlist>>`, `Db::playlist_track_ids(id: i64) -> Result<Vec<i64>>`, `group_library`, `song_xml`.
- Produces: `handle_playlists(method, query, db) -> Option<String>`, and `server::playlist_id(id: i64) -> String` / `server::parse_playlist_id(&str) -> Option<i64>`.

- [ ] **Step 1: Write the failing test**

Add inside `mod tests`:

```rust
    #[test]
    fn playlist_ids_round_trip() {
        assert_eq!(playlist_id(4), "pl4");
        assert_eq!(parse_playlist_id("pl4"), Some(4));
        assert_eq!(parse_playlist_id("tr4"), None);
    }

    #[test]
    fn playlists_are_listed_and_expandable() {
        let (handle, dir) = seeded_server_with_playlist();
        let auth = "u=m&p=testtoken&c=test";

        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/getPlaylists.view?{auth}"));
        let body = String::from_utf8_lossy(&body).into_owned();
        assert!(body.contains("Chill"), "{body}");
        assert!(body.contains("songCount=\"2\""), "{body}");

        let (_s, _h, body2) =
            http_get(handle.addr, &format!("/rest/getPlaylist.view?{auth}&id=pl1"));
        let body2 = String::from_utf8_lossy(&body2).into_owned();
        assert!(body2.contains("<entry "), "{body2}");
        assert!(body2.contains("Rhubarb"), "{body2}");
        assert!(body2.contains("Dandelion"), "{body2}");

        handle.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
```

And this helper, next to `seeded_server`:

```rust
    /// Same library as `seeded_server`, plus a playlist holding both tracks.
    fn seeded_server_with_playlist() -> (ServerHandle, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("melodie-pl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, vec![0xAAu8; 5000]).unwrap();
        std::fs::write(&b, vec![0xBBu8; 4000]).unwrap();

        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        let mut ids = Vec::new();
        for (path, artist, album, title) in [
            (&a, "Aphex Twin", "SAW II", "Rhubarb"),
            (&b, "Boards of Canada", "Geogaddi", "Dandelion"),
        ] {
            ids.push(
                db.upsert_track(&crate::db::NewTrack {
                    path: path.to_str().unwrap(),
                    title,
                    artist,
                    album,
                    track_no: Some(1),
                    duration_ms: 210_000,
                    mtime: 1,
                    size: std::fs::metadata(path).unwrap().len() as i64,
                })
                .unwrap(),
            );
        }
        let pl = db.upsert_playlist("Chill", "m3u8", None).unwrap();
        db.set_playlist_tracks(pl, &ids).unwrap();
        assert_eq!(pl, 1, "test assumes the first playlist gets id 1");

        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0;
        cfg.lan_token = "testtoken".to_string();
        let handle = spawn(&cfg, db).expect("server starts");
        (handle, dir)
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test server::`
Expected: FAIL — `cannot find function `playlist_id``.

- [ ] **Step 3: Implement the playlist handlers**

Append to `src/server.rs`, above the test module:

```rust
pub fn playlist_id(id: i64) -> String {
    format!("pl{id}")
}

pub fn parse_playlist_id(id: &str) -> Option<i64> {
    id.strip_prefix("pl")?.parse().ok()
}

fn handle_playlists(method: &str, query: &HashMap<String, String>, db: &Db) -> Option<String> {
    match method {
        "getPlaylists" => {
            let playlists = db.list_playlists().unwrap_or_default();
            let mut out = String::from("<playlists>");
            for p in &playlists {
                let ids = db.playlist_track_ids(p.id).unwrap_or_default();
                // Duration needs the tracks anyway; the library is a few
                // thousand rows and this is a once-per-open request.
                let duration: i64 = ids
                    .iter()
                    .filter_map(|id| db.get_track(*id).ok().flatten())
                    .map(|t| t.duration_ms / 1000)
                    .sum();
                out.push_str(&format!(
                    "<playlist id=\"{}\" name=\"{}\" songCount=\"{}\" duration=\"{duration}\" \
                     owner=\"melodie\" public=\"false\"/>",
                    playlist_id(p.id),
                    escape_xml(&p.name),
                    ids.len(),
                ));
            }
            out.push_str("</playlists>");
            Some(ok_envelope(&out))
        }

        "getPlaylist" => {
            let raw = query.get("id")?;
            let id = parse_playlist_id(raw)?;
            let playlists = db.list_playlists().unwrap_or_default();
            let playlist = playlists.iter().find(|p| p.id == id)?;
            let track_ids = db.playlist_track_ids(id).unwrap_or_default();

            // Reuse the browse grouping so `<entry>` carries the same album
            // and artist ids the rest of the API hands out.
            let groups = group_library(db.list_tracks().unwrap_or_default());
            let mut entries = String::new();
            let mut duration = 0;
            for track_id_value in &track_ids {
                let Some((track, album)) = groups
                    .iter()
                    .flat_map(|a| &a.albums)
                    .find_map(|al| al.tracks.iter().find(|t| t.id == *track_id_value).map(|t| (t, al)))
                else {
                    continue;
                };
                duration += track.duration_ms / 1000;
                entries.push_str(&song_xml(track, album).replacen("<song ", "<entry ", 1));
            }
            Some(ok_envelope(&format!(
                "<playlist id=\"{}\" name=\"{}\" songCount=\"{}\" duration=\"{duration}\" \
                 owner=\"melodie\" public=\"false\">{entries}</playlist>",
                playlist_id(playlist.id),
                escape_xml(&playlist.name),
                track_ids.len(),
            )))
        }

        _ => None,
    }
}
```

- [ ] **Step 4: Route it**

In `fn handle`, change the fallback arm to try playlists before giving up:

```rust
        _ => match handle_browse(method, &query, db).or_else(|| handle_playlists(method, &query, db)) {
            Some(body) => body,
            None => error_envelope(70, "Requested data was not found"),
        },
```

- [ ] **Step 5: Run the tests**

Run: `cargo test server::`
Expected: PASS, 16 tests.

- [ ] **Step 6: Verify the full suite**

Run: `cargo test`
Expected: PASS.

---

### Task 5: Streaming with range support

The one endpoint where the resource budget bites: audio must be streamed from disk, never buffered whole, and `Range` must work or clients cannot seek.

**Files:**
- Modify: `src/server.rs`

**Interfaces:**
- Consumes: `Db::get_track(id) -> Result<Option<Track>>`, `parse_track_id`.
- Produces: `server::parse_range(value: &str, len: u64) -> Option<(u64, u64)>`; `stream`, `download` and `scrobble` routes.

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests`:

```rust
    #[test]
    fn parse_range_handles_the_forms_clients_actually_send() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        // Suffix range: "the last 200 bytes".
        assert_eq!(parse_range("bytes=-200", 1000), Some((800, 999)));
        // Past the end is clamped, not an error.
        assert_eq!(parse_range("bytes=0-99999", 1000), Some((0, 999)));
        // Unsatisfiable or nonsense -> None, caller falls back to a 200.
        assert_eq!(parse_range("bytes=1000-1005", 1000), None);
        assert_eq!(parse_range("bytes=500-100", 1000), None);
        assert_eq!(parse_range("items=0-10", 1000), None);
        assert_eq!(parse_range("garbage", 1000), None);
        assert_eq!(parse_range("bytes=0-10", 0), None, "empty file has no satisfiable range");
    }

    #[test]
    fn stream_serves_whole_files_and_byte_ranges() {
        let (handle, dir) = seeded_server();
        let auth = "u=m&p=testtoken&c=test";

        let (status, headers, body) =
            http_get(handle.addr, &format!("/rest/stream.view?{auth}&id=tr1"));
        assert!(status.contains("200"), "{status}");
        assert_eq!(body.len(), 5000, "whole file");
        assert!(body.iter().all(|b| *b == 0xAA));
        assert!(headers.contains("Accept-Ranges: bytes"), "{headers}");
        assert!(headers.to_lowercase().contains("audio/mpeg"), "{headers}");

        let (status, headers, body) = http_get_with(
            handle.addr,
            &format!("/rest/stream.view?{auth}&id=tr1"),
            &[("Range", "bytes=100-199")],
        );
        assert!(status.contains("206"), "partial content: {status}");
        assert_eq!(body.len(), 100);
        assert!(headers.contains("Content-Range: bytes 100-199/5000"), "{headers}");

        // `download` is the same bytes; offline-capable clients use it.
        let (status, _h, body) = http_get(handle.addr, &format!("/rest/download.view?{auth}&id=tr2"));
        assert!(status.contains("200"), "{status}");
        assert_eq!(body.len(), 4000);

        // A bad id is a Subsonic error, not a panic or a hang.
        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/stream.view?{auth}&id=tr999"));
        assert!(String::from_utf8_lossy(&body).contains("status=\"failed\""));

        // scrobble is accepted and does nothing.
        let (_s, _h, body) = http_get(handle.addr, &format!("/rest/scrobble.view?{auth}&id=tr1"));
        assert!(String::from_utf8_lossy(&body).contains("status=\"ok\""));

        handle.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test server::`
Expected: FAIL — `cannot find function `parse_range``.

- [ ] **Step 3: Implement range parsing and the stream handler**

Append to `src/server.rs`, above the test module:

```rust
/// Parses a single-range `Range:` header value against a known file length.
/// Multi-range requests (`bytes=0-10,20-30`) are not supported — no audio
/// client sends them — and fall back to a full-body 200.
pub fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (from, to) = spec.split_once('-')?;
    let (start, end) = if from.is_empty() {
        // "-N" = the last N bytes.
        let n: u64 = to.trim().parse().ok()?;
        if n == 0 {
            return None;
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let start: u64 = from.trim().parse().ok()?;
        let end = match to.trim() {
            "" => len - 1,
            other => other.parse().ok()?,
        };
        (start, end.min(len - 1))
    };
    if start > end || start >= len {
        return None;
    }
    Some((start, end))
}

/// Streams a track's bytes, honouring `Range`.
///
/// PLAN.md §6 ("nothing loads whole"): this seeks and hands tiny_http a
/// `Take<File>`, so serving a 40-minute FLAC costs the same memory as
/// serving a 2-minute MP3.
fn respond_stream(request: Request, db: &Db, query: &HashMap<String, String>) {
    let track = query
        .get("id")
        .and_then(|id| parse_track_id(id))
        .and_then(|id| db.get_track(id).ok().flatten());
    let Some(track) = track else {
        let _ = request.respond(xml_response(error_envelope(70, "Requested data was not found")));
        return;
    };

    let file = match std::fs::File::open(&track.path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("melodie: LAN server cannot open {}: {e}", track.path);
            let _ = request.respond(xml_response(error_envelope(70, "File not found")));
            return;
        }
    };
    let len = match file.metadata() {
        Ok(m) => m.len(),
        Err(_) => 0,
    };

    let content_type = content_type_for(&extension_of(&track.path));
    let range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| parse_range(h.value.as_str(), len));

    let mut headers = vec![
        Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).expect("static header"),
        Header::from_bytes(&b"Accept-Ranges"[..], &b"bytes"[..]).expect("static header"),
    ];

    let result = match range {
        Some((start, end)) => {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = file;
            if file.seek(SeekFrom::Start(start)).is_err() {
                let _ = request.respond(xml_response(error_envelope(0, "Seek failed")));
                return;
            }
            let count = end - start + 1;
            headers.push(
                Header::from_bytes(
                    &b"Content-Range"[..],
                    format!("bytes {start}-{end}/{len}").as_bytes(),
                )
                .expect("formatted header"),
            );
            request.respond(Response::new(
                StatusCode(206),
                headers,
                file.take(count),
                Some(count as usize),
                None,
            ))
        }
        None => request.respond(Response::new(
            StatusCode(200),
            headers,
            file,
            Some(len as usize),
            None,
        )),
    };
    if let Err(e) = result {
        // A phone that walked out of Wi-Fi range mid-song is normal, not an
        // error worth shouting about.
        eprintln!("melodie: LAN stream ended early: {e}");
    }
}
```

- [ ] **Step 4: Route the binary endpoints**

`stream` consumes the `Request` (it writes bytes, not an XML string), so it must be routed *before* the XML `match`. In `fn handle`, insert this immediately after the auth check:

```rust
    let method = method_of(&url);
    if method == "stream" || method == "download" {
        respond_stream(request, db, &query);
        return;
    }
```

Then change the XML `match` to reuse that `method` binding (delete the second `let method = method_of(&url);` if Step 3 of Task 3 left one), and add the scrobble arm:

```rust
    let body = match method {
        "ping" => ok_envelope(""),
        "getLicense" => ok_envelope("<license valid=\"true\"/>"),
        // Accepted and discarded: Melodie keeps no play counts (PLAN.md §7
        // lists scrobble as a no-op), but a client that gets an error here
        // may show it to the user after every song.
        "scrobble" => ok_envelope(""),
        _ => match handle_browse(method, &query, db).or_else(|| handle_playlists(method, &query, db)) {
            Some(body) => body,
            None => error_envelope(70, "Requested data was not found"),
        },
    };
```

- [ ] **Step 5: Run the tests**

Run: `cargo test server::`
Expected: PASS, 18 tests.

- [ ] **Step 6: Verify the full suite**

Run: `cargo test`
Expected: PASS.

---

### Task 6: Cover art

**Files:**
- Modify: `src/db.rs` (one query — no schema change)
- Modify: `src/server.rs`

**Interfaces:**
- Consumes: `lofty::probe::Probe` (already a dependency, used by `library.rs` and `fetch.rs`), `Config::covers_dir()`.
- Produces:
  - `Db::cover_video_id_for_track(&self, track_id: i64) -> Result<Option<String>>`
  - `server::respond_cover_art(request, cfg, db, query)`

- [ ] **Step 1: Write the failing DB test**

Add inside `mod tests` in `src/db.rs`:

```rust
    #[test]
    fn cover_video_id_resolves_through_the_spotisync_match() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let track_id = db
            .upsert_track(&NewTrack {
                path: "/music/Artist/Album/Song.aac",
                title: "Song",
                artist: "Artist",
                album: "Album",
                track_no: Some(1),
                duration_ms: 1000,
                mtime: 1,
                size: 100,
            })
            .unwrap();

        // No spotisync provenance yet.
        assert_eq!(db.cover_video_id_for_track(track_id).unwrap(), None);

        db.upsert_spotify_track(&SpotifyTrackRow {
            uri: "spotify:track:abc".to_string(),
            title: "Song".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration_ms: 1000,
            isrc: None,
        })
        .unwrap();
        db.upsert_match("spotify:track:abc", Some("vid123"), Some(90.0), true).unwrap();

        assert_eq!(
            db.cover_video_id_for_track(track_id).unwrap(),
            Some("vid123".to_string())
        );
        assert_eq!(db.cover_video_id_for_track(9999).unwrap(), None);
    }
```

Check `upsert_match`'s real signature in `src/db.rs` before running — if it takes different arguments, call it as declared there rather than as written above.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test db::`
Expected: FAIL — `no method named `cover_video_id_for_track``.

- [ ] **Step 3: Implement the query**

Add to `impl Db` in `src/db.rs`, after `get_track_id_by_tags`:

```rust
    /// The YouTube id a track was downloaded from, if SpotiSync fetched it —
    /// which is where `covers/<id>.jpg` gets its name.
    ///
    /// Joined on tags rather than path for the same reason
    /// `get_track_id_by_tags` exists: the on-disk extension isn't stable
    /// (PLAN.md §5.5's format fallback, plus the ADTS remux).
    pub fn cover_video_id_for_track(&self, track_id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT m.youtube_id
               FROM tracks t
               JOIN spotify_tracks s
                 ON s.title = t.title AND s.artist = t.artist AND s.album = t.album
               JOIN matches m ON m.spotify_uri = s.uri
              WHERE t.id = ?1 AND m.youtube_id IS NOT NULL
              LIMIT 1",
            params![track_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(Into::into)
    }
```

- [ ] **Step 4: Run the DB test**

Run: `cargo test db::cover_video_id`
Expected: PASS.

- [ ] **Step 5: Write the failing server test**

Add inside `mod tests` in `src/server.rs`:

```rust
    #[test]
    fn cover_art_falls_back_to_the_spotisync_thumbnail_and_404s_otherwise() {
        let (handle, dir) = seeded_server();
        let auth = "u=m&p=testtoken&c=test";

        // The seeded tracks are raw bytes with no embedded picture and no
        // spotisync provenance, so there is genuinely no art to serve.
        let (status, _h, _b) = http_get(handle.addr, &format!("/rest/getCoverArt.view?{auth}&id=tr1"));
        assert!(status.contains("404"), "no art available should 404: {status}");

        let (status, _h, _b) =
            http_get(handle.addr, &format!("/rest/getCoverArt.view?{auth}&id=trnope"));
        assert!(status.contains("404"), "{status}");

        handle.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test server::cover_art`
Expected: FAIL — the request currently returns a 200 XML error envelope, not a 404.

- [ ] **Step 7: Implement cover art**

Append to `src/server.rs`, above the test module:

```rust
/// Resolves a cover-art id (track, album, or artist) to representative
/// image bytes.
///
/// Tries the file's own embedded picture first (the library is the source
/// of truth, PLAN.md §3), then falls back to the thumbnail SpotiSync saved
/// in `covers/`. Downloaded tracks are raw ADTS `.aac`, which has no
/// container-level metadata slot at all, so for most of a synced library
/// the fallback *is* the art.
fn find_cover(cfg: &Config, db: &Db, id: &str) -> Option<(Vec<u8>, &'static str)> {
    let track = if let Some(track_id) = parse_track_id(id) {
        db.get_track(track_id).ok().flatten()
    } else {
        // An album or artist id: any track under it will do.
        let groups = group_library(db.list_tracks().unwrap_or_default());
        groups
            .iter()
            .flat_map(|a| &a.albums)
            .find(|al| al.id == id || al.artist_id == id)
            .and_then(|al| al.tracks.first().cloned())
    }?;

    if let Ok(tagged) = lofty::probe::Probe::open(&track.path).and_then(|p| p.read()) {
        use lofty::file::TaggedFileExt;
        if let Some(picture) = tagged.primary_tag().and_then(|t| t.pictures().first()) {
            let mime = match picture.mime_type() {
                Some(lofty::picture::MimeType::Png) => "image/png",
                _ => "image/jpeg",
            };
            return Some((picture.data().to_vec(), mime));
        }
    }

    let video_id = db.cover_video_id_for_track(track.id).ok().flatten()?;
    let path = cfg.covers_dir().join(format!("{video_id}.jpg"));
    let bytes = std::fs::read(path).ok()?;
    Some((bytes, "image/jpeg"))
}

fn respond_cover_art(request: Request, cfg: &Config, db: &Db, query: &HashMap<String, String>) {
    let art = query.get("id").and_then(|id| find_cover(cfg, db, id));
    let response = match art {
        Some((bytes, mime)) => {
            let header =
                Header::from_bytes(&b"Content-Type"[..], mime.as_bytes()).expect("static header");
            Response::from_data(bytes).with_header(header)
        }
        // A real 404 (not an XML error envelope) is what clients expect for
        // a missing image — they show their own placeholder.
        None => Response::from_data(Vec::new()).with_status_code(StatusCode(404)),
    };
    let _ = request.respond(response);
}
```

- [ ] **Step 8: Route it**

In `fn handle`, extend the binary-endpoint early return:

```rust
    let method = method_of(&url);
    if method == "stream" || method == "download" {
        respond_stream(request, db, &query);
        return;
    }
    if method == "getCoverArt" {
        respond_cover_art(request, cfg, db, &query);
        return;
    }
```

Now that `cfg` is used, delete the `let _ = cfg;` line left over from Task 3.

- [ ] **Step 9: Run the tests**

Run: `cargo test server::`
Expected: PASS, 19 tests.

- [ ] **Step 10: Verify the full suite**

Run: `cargo test`
Expected: PASS.

---

### Task 7: Wire it up — startup, pairing QR, docs

Starts the server from the app, and gives the user the host/port/token in a form a phone can consume. Nothing before this task changes Melodie's observable behaviour.

**Files:**
- Create: `src/ui/pair.rs`
- Modify: `src/ui/mod.rs` (add the module + a "Pair" button)
- Modify: `src/app.rs` (start the server, wire the button)
- Modify: `src/main.rs` (`--pair` flag + usage text)
- Modify: `README.md`
- Create: `docs/lan.md`

**Interfaces:**
- Consumes: `server::spawn`, `net::detect_lan_ip`, `Config::ensure_lan_token`.
- Produces:
  - `ui::pair::pairing_url(host: &IpAddr, port: u16, token: &str) -> String`
  - `ui::pair::qr_rgb(data: &str, scale: usize, quiet: usize) -> Option<(Vec<u8>, i32)>`
  - `ui::pair::show(url: &str)`

- [ ] **Step 1: Write the failing tests**

Create `src/ui/pair.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn pairing_url_is_the_format_the_android_app_parses() {
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(pairing_url(&ip, 4533, "abc123"), "melodie://192.168.1.20:4533/abc123");
    }

    #[test]
    fn qr_rgb_produces_a_square_rgb_buffer_with_a_quiet_zone() {
        let (buf, side) = qr_rgb("melodie://192.168.1.20:4533/abc123", 4, 4).unwrap();
        assert!(side > 0);
        assert_eq!(buf.len(), (side as usize) * (side as usize) * 3);
        // The quiet zone must be white, or scanners fail to lock on.
        assert_eq!(&buf[0..3], &[255, 255, 255]);
        // And something must actually be drawn.
        assert!(buf.iter().any(|b| *b == 0), "QR matrix has no dark modules");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test pair::`
Expected: FAIL — module not declared / functions missing.

- [ ] **Step 3: Implement `src/ui/pair.rs`**

Insert above the test module:

```rust
//! Phone pairing (PLAN.md §7): a QR code carrying host, port and token —
//! "no login, no account, matching the rest of the project".

use std::net::IpAddr;

use fltk::enums::ColorDepth;
use fltk::frame::Frame;
use fltk::image::RgbImage;
use fltk::prelude::*;
use fltk::window::Window;

use crate::ui::theme;

/// The payload encoded in the QR and accepted by the companion app.
pub fn pairing_url(host: &IpAddr, port: u16, token: &str) -> String {
    format!("melodie://{host}:{port}/{token}")
}

/// Renders `data` as a QR code into a raw RGB8 buffer, returning it with the
/// square's side length in pixels.
///
/// `scale` is pixels per QR module; `quiet` is the mandatory white border in
/// modules (4 is the spec minimum, and scanners really do need it).
pub fn qr_rgb(data: &str, scale: usize, quiet: usize) -> Option<(Vec<u8>, i32)> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let modules = code.to_colors();
    let side = (width + quiet * 2) * scale;
    let mut buf = vec![255u8; side * side * 3];
    for y in 0..width {
        for x in 0..width {
            if modules[y * width + x] != qrcode::Color::Dark {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let py = (y + quiet) * scale + dy;
                    let px = (x + quiet) * scale + dx;
                    let i = (py * side + px) * 3;
                    buf[i] = 0;
                    buf[i + 1] = 0;
                    buf[i + 2] = 0;
                }
            }
        }
    }
    Some((buf, side as i32))
}

/// Opens a small window showing the QR plus the same details as text, so a
/// phone can be paired by scanning *or* by typing.
pub fn show(url: &str) {
    let Some((buf, side)) = qr_rgb(url, 6, 4) else {
        eprintln!("melodie: could not render the pairing QR for {url}");
        return;
    };
    let Ok(image) = RgbImage::new(&buf, side, side, ColorDepth::Rgb8) else {
        eprintln!("melodie: could not build the pairing image");
        return;
    };

    let pad = 16;
    let text_h = 52;
    let win_w = side + pad * 2;
    let win_h = side + pad * 2 + text_h;
    let mut win = Window::new(300, 200, win_w, win_h, "Pair phone");
    win.set_color(theme::BG);

    let mut art = Frame::new(pad, pad, side, side, None);
    art.set_image(Some(image));

    let mut label = Frame::new(pad, side + pad, side, text_h, None);
    label.set_label_color(theme::FG);
    label.set_label_size(theme::FONT_SIZE - 1);
    label.set_label(&format!("Scan in the Melodie app, or enter it by hand:\n{url}"));

    win.end();
    win.show();
    // Deliberately not modal: pairing shouldn't block playback controls.
}
```

- [ ] **Step 4: Declare the module**

At the top of `src/ui/mod.rs`, alongside the other `pub mod` lines:

```rust
#[cfg(feature = "lan")]
pub mod pair;
```

- [ ] **Step 5: Run the tests**

Run: `cargo test pair::`
Expected: PASS, 2 tests.

- [ ] **Step 6: Add the Pair button**

In `src/ui/mod.rs`, add the field to `MainWindow` (after `review_btn`):

```rust
    #[cfg(feature = "lan")]
    pub pair_btn: Button,
```

In `fn build`, the top-bar buttons are laid out right-to-left from `WIN_W`. Replace the cluster-width calculation and the `review_btn`/`sync_btn` construction with:

```rust
    let choice_w = 140;
    let sync_w = 50;
    let review_w = 64;
    #[cfg(feature = "lan")]
    let pair_w = 46;
    #[cfg(not(feature = "lan"))]
    let pair_w = 0;
    let cluster_w = pair_w + sync_w + 4 + review_w + 4 + choice_w + if pair_w > 0 { 4 } else { 0 };

    let mut playlist_choice = Choice::new(WIN_W - choice_w - PAD, 6, choice_w, 24, None);
    playlist_choice.set_color(theme::BG);
    playlist_choice.set_label_color(theme::FG);
    playlist_choice.set_text_color(theme::FG);

    let mut review_btn = Button::new(WIN_W - choice_w - review_w - PAD - 4, 6, review_w, 24, "Review");
    let mut sync_btn = Button::new(WIN_W - choice_w - review_w - sync_w - PAD - 8, 6, sync_w, 24, "Sync");
    #[cfg(feature = "lan")]
    let mut pair_btn = Button::new(
        WIN_W - choice_w - review_w - sync_w - pair_w - PAD - 12,
        6,
        pair_w,
        24,
        "Pair",
    );

    for b in [&mut sync_btn, &mut review_btn] {
        b.set_color(theme::BG);
        b.set_label_color(theme::FG);
        b.set_label_size(theme::FONT_SIZE - 1);
        b.set_frame(theme::BUTTON_FRAME);
        add_hover(b, theme::BG, theme::BTN_HOVER);
    }
    #[cfg(feature = "lan")]
    {
        pair_btn.set_color(theme::BG);
        pair_btn.set_label_color(theme::FG);
        pair_btn.set_label_size(theme::FONT_SIZE - 1);
        pair_btn.set_frame(theme::BUTTON_FRAME);
        add_hover(&mut pair_btn, theme::BG, theme::BTN_HOVER);
    }
```

And add to the `MainWindow { ... }` literal at the end of `build`, after `review_btn,`:

```rust
        #[cfg(feature = "lan")]
        pair_btn,
```

- [ ] **Step 7: Start the server and wire the button in `app.rs`**

In `src/app.rs`, add near the other `use` lines:

```rust
#[cfg(feature = "lan")]
use crate::net;
```

In `pub fn run`, after the `spotisync::spawn` block and its `SyncInbox` send, add:

```rust
    // LAN server (PLAN.md §7 Tier 0). Held for the process lifetime; the
    // handle is only needed to stop it, and the process exiting does that.
    #[cfg(feature = "lan")]
    let _lan = {
        let mut cfg = cfg.clone();
        if cfg.lan_enabled {
            if cfg.ensure_lan_token() {
                if let Err(e) = cfg.save() {
                    eprintln!("melodie: could not save the generated LAN token: {e:#}");
                }
            }
            match crate::server::spawn(&cfg, db.clone()) {
                Ok(handle) => {
                    eprintln!("melodie: LAN server listening on http://{}", handle.addr);
                    Some(handle)
                }
                Err(e) => {
                    // Never fatal: a music player that refuses to start
                    // because a socket is busy is a broken music player.
                    eprintln!("melodie: LAN server disabled: {e:#}");
                    None
                }
            }
        } else {
            None
        }
    };

    #[cfg(feature = "lan")]
    {
        let cfg_for_pair = cfg.clone();
        win.pair_btn.set_callback(move |_| {
            let mut cfg = cfg_for_pair.clone();
            if !cfg.lan_enabled {
                eprintln!(
                    "melodie: set `lan_enabled = true` in {} and restart to pair a phone",
                    Config::config_path().display()
                );
                return;
            }
            if cfg.ensure_lan_token() {
                let _ = cfg.save();
            }
            let host = net::detect_lan_ip().unwrap_or(std::net::IpAddr::V4(
                std::net::Ipv4Addr::LOCALHOST,
            ));
            ui::pair::show(&ui::pair::pairing_url(&host, cfg.lan_port, &cfg.lan_token));
        });
    }
```

- [ ] **Step 8: Add the `--pair` CLI flag**

`--pair` prints the same details for headless use, where there is no window to click.

In `src/main.rs`, add to the flag parsing next to `rescan_only`:

```rust
    let pair_only = args.iter().any(|a| a == "--pair");
```

And after the `if rescan_only { ... }` block (before the window opens), add:

```rust
    #[cfg(feature = "lan")]
    if pair_only {
        let mut cfg = cfg;
        if cfg.ensure_lan_token() {
            if let Err(e) = cfg.save() {
                eprintln!("melodie: could not save the generated LAN token: {e:#}");
                return ExitCode::FAILURE;
            }
        }
        let host = net::detect_lan_ip()
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        println!("Host:  {host}");
        println!("Port:  {}", cfg.lan_port);
        println!("Token: {}", cfg.lan_token);
        println!("URL:   melodie://{host}:{}/{}", cfg.lan_port, cfg.lan_token);
        if !cfg.lan_enabled {
            println!(
                "\nNote: `lan_enabled` is false in {}. Set it to true and restart \
                 melodie before pairing.",
                Config::config_path().display()
            );
        }
        return ExitCode::SUCCESS;
    }
    #[cfg(not(feature = "lan"))]
    if pair_only {
        eprintln!("melodie: built without the `lan` feature");
        return ExitCode::FAILURE;
    }
```

Update `print_usage`'s text to include:

```
         --pair          print this machine's LAN host/port/token for the
         \x20               Android companion app (no window).
```

- [ ] **Step 9: Build and verify the wiring compiles both ways**

Run: `cargo build`
Expected: builds clean.

Run: `cargo build --no-default-features`
Expected: builds clean.

Run: `cargo test`
Expected: PASS, everything.

- [ ] **Step 10: Smoke-test against a real running server**

Run:

```sh
cargo run -- --pair
```

Expected: prints Host/Port/Token/URL and the "lan_enabled is false" note.

Then enable it and check with a real HTTP client:

```sh
sed -i 's/^lan_enabled = false/lan_enabled = true/' ~/.config/melodie/config.toml
cargo run -- --pair          # note the token
TOKEN=$(grep '^lan_token' ~/.config/melodie/config.toml | cut -d'"' -f2)
HOST=$(cargo run --quiet -- --pair | awk '/^Host:/{print $2}')
```

Start Melodie (`cargo run &`), then:

```sh
curl -s "http://$HOST:4533/rest/ping.view?u=m&p=$TOKEN&c=curl"
curl -s "http://$HOST:4533/rest/getIndexes.view?u=m&p=$TOKEN&c=curl" | head -c 400
curl -s -o /dev/null -w '%{http_code} %{size_download}\n' \
     -H 'Range: bytes=0-1023' "http://$HOST:4533/rest/stream.view?u=m&p=$TOKEN&c=curl&id=tr1"
```

Expected: `status="ok"`; an `<indexes>` listing with real artists; `206 1024`.

Record the actual outputs — they are the evidence this task is done.

- [ ] **Step 11: Document it**

Create `docs/lan.md`:

```markdown
# LAN / Subsonic server

Melodie can serve its library to the local network as a read-only
OpenSubsonic API (PLAN.md §7, Tier 0). It is compiled in by default and
switched off at runtime.

## Enabling

In `~/.config/melodie/config.toml`:

```toml
lan_enabled = true
lan_port = 4533
lan_bind = ""            # empty = auto-detect this machine's LAN IP
lan_token = ""           # generated on first use; this is the password
lan_allow_public = false # refuse to bind a non-private address
```

Restart Melodie. It prints the address it bound.

## Pairing a phone

Click **Pair** in the window, or run `melodie --pair` for a headless box.
Both give you host, port and token; the button also shows a QR code that
the Melodie Android app scans.

For a third-party Subsonic client, enter:

- Server: `http://<host>:<port>`
- Username: anything (there are no accounts)
- Password: the `lan_token` value

## Supported methods

`ping`, `getLicense`, `getMusicFolders`, `getIndexes`, `getMusicDirectory`,
`getArtists`, `getArtist`, `getAlbum`, `getPlaylists`, `getPlaylist`,
`stream` (with `Range`), `download`, `getCoverArt`, `scrobble` (no-op).

Everything is read-only: there is no endpoint that writes to the library.

## Safety

- The token is required on every request; an unset token authenticates
  nothing.
- The server refuses to bind a public or wildcard address unless
  `lan_allow_public = true`.
- Responses are XML only.
```

Then update `README.md`:

1. Change the "This build implements **Phases 0–3**" sentence to say Phases 0–4, and drop LAN/Subsonic from the not-implemented list.
2. Add `melodie --pair` to the Running section's command list.
3. Add a **Phase 4** bullet to "What's implemented": the OpenSubsonic read-only subset, token auth, QR pairing, and that mDNS was deliberately skipped.
4. Add to "Deviations from PLAN.md": *mDNS advertisement (PLAN.md §8) is not implemented — QR pairing already carries host, port and token, so LAN discovery buys nothing for a single-user pairing flow and costs a dependency plus periodic background chatter against the §6 CPU budget.* Also note *`getLicense` and `getMusicFolders` are served although PLAN.md §7's list omits them: real clients refuse to proceed without them.*

- [ ] **Step 12: Final verification**

Run: `cargo test`
Expected: PASS.

Run: `cargo build --release`
Expected: builds clean.

Run: `ls -lh target/release/melodie`
Record the stripped binary size and compare against PLAN.md §6 (≤6 MB target, 10 MB hard ceiling). The pre-Phase-4 build was ~6.9 MB; note the delta the four new crates cost, and add the new number to `README.md`'s "Resource budget" section.

---

## Self-Review

**Spec coverage:**

| Spec item | Task |
|---|---|
| `tiny_http`, `fastrand`, `qrcode`, `md5` deps | 1 |
| `lan` Cargo feature | 1 |
| Config additions (`lan_enabled`/`port`/`bind`/`token`/`allow_public`) | 1 |
| Refuse non-private bind without override | 1 |
| `ping`, `getLicense` | 2 |
| XML-only responses, no JSON | 2 (envelope), all handlers |
| Folder family (`getIndexes`, `getMusicDirectory`) | 3 |
| ID3 family (`getArtists`, `getArtist`, `getAlbum`) | 3 |
| Shared grouping over `list_tracks()` | 3 |
| Computed IDs, no schema change | 3 |
| `getPlaylists`, `getPlaylist` | 4 |
| `stream` with `Range` | 5 |
| `download` alias, `scrobble` no-op | 5 |
| `getCoverArt` via lofty + `covers/` fallback | 6 |
| 2 fixed worker threads | 2 |
| QR pairing window + `melodie://` payload | 7 |
| Unit tests: XML escape, id hashing, auth, range parsing | 2, 3, 5 |
| Integration test over raw `TcpStream` | 2 (helper), 3, 4, 5, 6 |
| mDNS explicitly skipped | 7 (documented) |
| `curl` smoke test | 7 |

No gaps.

**Placeholder scan:** none — every step carries the code or the exact command.

**Type consistency:** `ServerHandle.addr` is a `SocketAddr` in Task 2 and used as one in Tasks 3-6. `group_library` returns `Vec<ArtistGroup>` in Task 3 and is consumed as such in Tasks 4 and 6. `song_xml(&Track, &Album)` keeps that signature in Tasks 3, 4, 6. `parse_track_id -> Option<i64>` matches `Db::get_track(i64)`. `Config` is `Clone` (existing derive), which Tasks 2 and 7 rely on.
