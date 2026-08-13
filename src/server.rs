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
use tiny_http::{Header, Request, Response, Server};

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
    let listen_addr_str = format!("{}", server.server_addr());
    let bound: SocketAddr = listen_addr_str.parse()
        .context("LAN server addr couldn't be parsed")?;

    let mut threads = Vec::with_capacity(WORKER_THREADS);
    for _ in 0..WORKER_THREADS {
        let server = server.clone();
        let db = db.clone();
        let token = cfg.lan_token.clone();
        let _cfg = cfg.clone();
        threads.push(std::thread::spawn(move || {
            // Blocks in recv(); no polling, no timer, 0% idle CPU (PLAN.md §6).
            while let Ok(request) = server.recv() {
                handle(request, &_cfg, &db, &token);
            }
        }));
    }

    Ok(ServerHandle { addr: bound, server, threads })
}

fn xml_response(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    let ct_header = Header::from_bytes(&b"Content-Type"[..], &b"text/xml; charset=utf-8"[..])
        .expect("static header parses");
    let close_header = Header::from_bytes(&b"Connection"[..], &b"close"[..])
        .expect("static header parses");
    Response::from_data(body.into_bytes())
        .with_header(ct_header)
        .with_header(close_header)
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
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).expect("set timeout");
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
    fn server_spawns_without_error() {
        let db = Arc::new(Db::open(Path::new(":memory:")).unwrap());
        let mut cfg = Config::default();
        cfg.lan_enabled = true;
        cfg.lan_bind = "127.0.0.1".to_string();
        cfg.lan_port = 0;
        cfg.lan_token = "test".to_string();
        let handle = spawn(&cfg, db).expect("server should spawn");
        let addr = handle.addr;
        assert_ne!(addr.port(), 0, "server should have been assigned a port");
        handle.stop();
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
}
