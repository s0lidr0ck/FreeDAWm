//! FreeDAWm over the internet (spec `docs/superpowers/specs/2026-11-02-remote-audio-recording-design.md`,
//! section 3): a direct, encrypted connection between the two PCs through home routers (QUIC
//! with hole punching), a relay when no direct path can be made, with no accounts, VPN or server
//! of our own. Built on iroh (MIT/Apache-2.0): each PC has a key pair kept in its network
//! settings; its public key is its address (with a relay and its IP addresses as hints).
//!
//! The LAN protocol runs over it unchanged: each QUIC stream is bridged to a local TCP
//! connection. On the host, a stream from a remote PC becomes a connection to the hub's
//! internet listener (loopback only, where only pairing by invite is allowed, see
//! [`crate::hub::Hub::listen_internet`]); on the remote PC, the follower connects to a local
//! port ([`Internet::bridge_to`]) and each connection becomes a stream to the host. Everything
//! here runs on its own tokio runtime's threads; nothing touches the audio thread.
//!
//! Relays: n0's public relays by default (free, rate-limited, meant for hobby use), or the one
//! named in Setup › Network (a self-hosted `iroh-relay` later needs no code change).

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The protocol name both ends speak.
pub const ALPN: &[u8] = b"freedawm/rig/1";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Which relays to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relays {
    /// n0's public relays (and their address lookup).
    Public,
    /// One relay by URL (`https://relay.example.org`).
    Custom(String),
    /// None at all: direct only, no lookup (tests on one machine).
    LocalOnly,
}

impl Relays {
    /// Setup › Network's relay field: empty for the public relays.
    pub fn from_setting(s: &str) -> Relays {
        let s = s.trim();
        if s.is_empty() { Relays::Public } else { Relays::Custom(s.to_string()) }
    }
}

/// How one connection is doing (Setup › Network shows it).
#[derive(Debug, Clone, PartialEq)]
pub struct Quality {
    /// The other PC's key, short.
    pub peer: String,
    /// A direct path (else through a relay).
    pub direct: bool,
    pub rtt_ms: f64,
    /// Packets lost on the selected path (%), and the path's estimated bandwidth (kb/s).
    pub lost_pct: f64,
    pub bandwidth_kbps: Option<f64>,
}

impl Quality {
    /// Plain words: "Direct, 38 ms" / "Through a relay, 120 ms".
    pub fn text(&self) -> String {
        let via = if self.direct { "Direct" } else { "Through a relay" };
        let mut t = format!("{via}, {:.0} ms round trip", self.rtt_ms);
        if self.lost_pct >= 0.5 {
            t.push_str(&format!(", {:.1} % lost", self.lost_pct));
        }
        t
    }

    /// Too weak to carry the mix stream (256 kb/s) reliably.
    pub fn weak(&self) -> bool {
        self.rtt_ms > 400.0 || self.lost_pct > 5.0 || self.bandwidth_kbps.is_some_and(|b| b < 400.0)
    }
}

/// This PC's internet endpoint.
pub struct Internet {
    rt: tokio::runtime::Runtime,
    ep: Endpoint,
    conns: Arc<Mutex<Vec<iroh::endpoint::Connection>>>,
}

/// A new key pair (32 bytes, for the network settings).
pub fn new_key() -> [u8; 32] {
    SecretKey::generate().to_bytes()
}

/// A key as hex, and back.
pub fn key_hex(k: &[u8; 32]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn key_from_hex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

async fn pump(stream: (iroh::endpoint::SendStream, iroh::endpoint::RecvStream), tcp: tokio::net::TcpStream) {
    let _ = tcp.set_nodelay(true);
    let (mut send, mut recv) = stream;
    let (mut r, mut w) = tcp.into_split();
    let down = tokio::io::copy(&mut recv, &mut w);
    let up = async {
        let _ = tokio::io::copy(&mut r, &mut send).await;
        let _ = send.finish();
    };
    tokio::select! {
        _ = down => {}
        _ = up => {}
    }
}

impl Internet {
    /// Starts this PC's endpoint with key `key` (see [`new_key`]).
    pub fn start(key: [u8; 32], relays: Relays) -> Result<Internet, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("freedawm-internet")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let secret = SecretKey::from_bytes(&key);
        let ep = rt.block_on(async {
            let b = match &relays {
                Relays::Public => Endpoint::builder(presets::N0),
                Relays::Custom(url) => {
                    let url: iroh::RelayUrl = url.parse().map_err(|e| format!("relay address `{url}`: {e}"))?;
                    Endpoint::builder(presets::N0).relay_mode(RelayMode::custom([url]))
                }
                Relays::LocalOnly => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled),
            };
            b.secret_key(secret).alpns(vec![ALPN.to_vec()]).bind().await.map_err(|e| e.to_string())
        })?;
        Ok(Internet { rt, ep, conns: Arc::new(Mutex::new(Vec::new())) })
    }

    /// This PC's address for an invite: its key, its relay and its IP addresses (waits up to
    /// `wait` for its relay, so a PC behind a router can be reached).
    pub fn addr(&self, wait: Duration) -> EndpointAddr {
        let ep = self.ep.clone();
        self.rt.block_on(async move {
            let _ = tokio::time::timeout(wait, ep.online()).await;
            ep.addr()
        })
    }

    /// The host: every stream from a remote PC becomes a connection to the hub's internet
    /// listener on `local_port`.
    pub fn serve_to(&self, local_port: u16) {
        let ep = self.ep.clone();
        let conns = Arc::clone(&self.conns);
        self.rt.spawn(async move {
            while let Some(incoming) = ep.accept().await {
                let conns = Arc::clone(&conns);
                tokio::spawn(async move {
                    let Ok(conn) = incoming.await else { return };
                    lock(&conns).push(conn.clone());
                    while let Ok(bi) = conn.accept_bi().await {
                        tokio::spawn(async move {
                            match tokio::net::TcpStream::connect(("127.0.0.1", local_port)).await {
                                Ok(tcp) => pump(bi, tcp).await,
                                Err(e) => log::warn!("internet: the hub is not listening: {e}"),
                            }
                        });
                    }
                    lock(&conns).retain(|c| c.stable_id() != conn.stable_id());
                });
            }
        });
    }

    /// The remote PC: a local port whose every connection becomes a stream to `host` (one
    /// connection to it, made again when it drops). The follower connects there as it would to
    /// a booth on the LAN.
    pub fn bridge_to(&self, host: EndpointAddr) -> Result<SocketAddr, String> {
        let listener = self.rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).map_err(|e| e.to_string())?;
        let local = listener.local_addr().map_err(|e| e.to_string())?;
        let ep = self.ep.clone();
        let conns = Arc::clone(&self.conns);
        let current: Arc<tokio::sync::Mutex<Option<iroh::endpoint::Connection>>> = Arc::new(tokio::sync::Mutex::new(None));
        self.rt.spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (ep, host, conns, current) = (ep.clone(), host.clone(), Arc::clone(&conns), Arc::clone(&current));
                tokio::spawn(async move {
                    let conn = {
                        let mut cur = current.lock().await;
                        let alive = cur.as_ref().is_some_and(|c| c.close_reason().is_none());
                        if !alive {
                            match tokio::time::timeout(Duration::from_secs(15), ep.connect(host.clone(), ALPN)).await {
                                Ok(Ok(c)) => {
                                    let mut all = lock(&conns);
                                    all.retain(|x| x.close_reason().is_none());
                                    all.push(c.clone());
                                    *cur = Some(c);
                                }
                                Ok(Err(e)) => {
                                    log::info!("internet: cannot reach the host: {e}");
                                    return;
                                }
                                Err(_) => {
                                    log::info!("internet: the host did not answer");
                                    return;
                                }
                            }
                        }
                        cur.clone()
                    };
                    let Some(conn) = conn else { return };
                    match conn.open_bi().await {
                        Ok(bi) => pump(bi, tcp).await,
                        Err(e) => log::info!("internet: {e}"),
                    }
                });
            }
        });
        Ok(local)
    }

    /// [`Internet::bridge_to`] the address an invite carried, as JSON (kept in the settings).
    pub fn bridge_to_json(&self, addr: &str) -> Result<SocketAddr, String> {
        let a: EndpointAddr = serde_json::from_str(addr).map_err(|_| "the host's internet address is damaged: join with a new invite".to_string())?;
        self.bridge_to(a)
    }

    /// How each open connection is doing.
    pub fn quality(&self) -> Vec<Quality> {
        let conns: Vec<_> = lock(&self.conns).iter().filter(|c| c.close_reason().is_none()).cloned().collect();
        conns
            .iter()
            .map(|c| {
                let paths = c.paths();
                let selected = paths.iter().find(|p| p.is_selected());
                let (direct, rtt, lost, bw) = match selected {
                    Some(p) => {
                        let s = p.stats();
                        let sent = s.udp_tx.datagrams.max(1) as f64;
                        (
                            p.is_ip(),
                            p.rtt().as_secs_f64() * 1000.0,
                            s.lost_packets as f64 * 100.0 / sent,
                            s.bandwidth_estimate.map(|b| b as f64 / 1000.0),
                        )
                    }
                    None => (false, 0.0, 0.0, None),
                };
                Quality { peer: c.remote_id().fmt_short().to_string(), direct, rtt_ms: rtt, lost_pct: lost, bandwidth_kbps: bw }
            })
            .collect()
    }

    /// This PC's key, short (shown in Setup › Network).
    pub fn id(&self) -> String {
        self.ep.id().fmt_short().to_string()
    }
}

impl Drop for Internet {
    fn drop(&mut self) {
        let ep = self.ep.clone();
        let _ = self.rt.block_on(async move { tokio::time::timeout(Duration::from_secs(2), ep.close()).await });
    }
}

// ---------------------------------------------------------------------------------------------
// Invites

/// What an invite code holds: the host's address, a one-time secret and its name.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Invite {
    #[serde(rename = "a")]
    pub addr: EndpointAddr,
    #[serde(rename = "s")]
    pub secret: String,
    #[serde(rename = "n")]
    pub name: String,
    /// What the invited PC does: `mix` (mixes the host), else it records with it (a performer).
    #[serde(rename = "r", default, skip_serializing_if = "String::is_empty")]
    pub role: String,
}

/// An invite's `role` for a remote mixer.
pub const ROLE_MIX: &str = "mix";

const PREFIX: &str = "freedawm-invite:";

impl Invite {
    /// The code to send the friend (one line of text).
    pub fn code(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!("{PREFIX}{}", crate::audio::b64_encode(&json))
    }

    /// An invite back from its code (spaces and line breaks from a chat window ignored).
    pub fn parse(code: &str) -> Result<Invite, String> {
        let text: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        let body = text.strip_prefix(PREFIX).ok_or("that is not a FreeDAWm invite (it starts with freedawm-invite:)")?;
        let json = crate::audio::b64_decode(body).ok_or("the invite is damaged: copy all of it")?;
        serde_json::from_slice(&json).map_err(|_| "the invite is damaged: copy all of it".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_invites_round_trip() {
        let k = new_key();
        assert_eq!(key_from_hex(&key_hex(&k)), Some(k));
        assert!(key_from_hex("zz").is_none());
        let addr = EndpointAddr::new(SecretKey::from_bytes(&k).public()).with_ip_addr("192.168.1.5:4433".parse().unwrap());
        let inv = Invite { addr, secret: "abc123".into(), name: "Booth".into(), role: String::new() };
        let code = inv.code();
        assert!(code.starts_with("freedawm-invite:"));
        // Pasted from a chat window, wrapped.
        let wrapped: String = code.chars().enumerate().flat_map(|(i, c)| if i % 30 == 29 { vec![c, '\n'] } else { vec![c] }).collect();
        assert_eq!(Invite::parse(&wrapped).unwrap(), inv);
        assert!(Invite::parse("hello").is_err());
        assert!(Invite::parse(&code[..code.len() - 8]).is_err());
        // A mixer's invite says so; one from before roles reads as a performer's.
        let mix = Invite { role: ROLE_MIX.into(), ..inv.clone() };
        assert_eq!(Invite::parse(&mix.code()).unwrap().role, "mix");
        assert!(!serde_json::to_string(&inv).unwrap().contains("\"r\""), "a performer's invite reads as before");
    }

    #[test]
    fn quality_in_plain_words() {
        let q = Quality { peer: "x".into(), direct: true, rtt_ms: 38.2, lost_pct: 0.0, bandwidth_kbps: None };
        assert_eq!(q.text(), "Direct, 38 ms round trip");
        assert!(!q.weak());
        let r = Quality { direct: false, rtt_ms: 450.0, lost_pct: 7.0, ..q };
        assert_eq!(r.text(), "Through a relay, 450 ms round trip, 7.0 % lost");
        assert!(r.weak());
    }
}
