//! Team Create room codes: finding the host from just its code.
//!
//! The host answers "who has room X?" on UDP. A joiner asks every network
//! this PC is on (the home network and VPNs like Tailscale, ZeroTier, Radmin
//! or Hamachi), and the host that knows the code answers with its port. No
//! server is involved, so this works for people on the same network or the
//! same VPN; it can't reach a PC across the internet by itself.
//!
//! The code itself is never sent: a question carries a random nonce and a
//! tag made from the code and that nonce, and the answer carries a tag only
//! the code's holder can make. Joining still needs the host to click Allow.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// UDP ports a host listens on for questions (the first free one).
pub const PORTS: [u16; 5] = [47601, 47602, 47603, 47604, 47605];
const MAGIC: &str = "ANVILROOM2";

/// "abcd-ef23" -> "ABCDEF23"; None if it isn't a room code.
pub fn normalize(code: &str) -> Option<String> {
    let c: String = code.chars().filter(|c| !c.is_whitespace() && *c != '-').map(|c| c.to_ascii_uppercase()).collect();
    (c.len() == 8 && c.chars().all(|c| "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(c))).then_some(c)
}

fn tag(parts: &[&str]) -> u64 {
    let mut h = DefaultHasher::new();
    "anvil team create room".hash(&mut h);
    for p in parts {
        p.hash(&mut h);
    }
    h.finish()
}

fn question(code: &str, nonce: u64) -> String {
    let n = format!("{nonce:016x}");
    format!("{MAGIC} FIND {n} {:016x}", tag(&["find", code, &n]))
}

fn answer(code: &str, nonce: &str, port: u16) -> String {
    let p = port.to_string();
    format!("{MAGIC} HERE {nonce} {p} {:016x}", tag(&["here", code, nonce, &p]))
}

/// Host side: answers questions for one room until dropped.
pub struct Beacon {
    /// The UDP port it answers on.
    #[cfg_attr(not(test), allow(dead_code))]
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl Drop for Beacon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Beacon {
    /// Listens on the first free port of `ports`. `allow_internet` = also
    /// answer public addresses (otherwise LAN / VPN only).
    pub fn start(code: &str, tcp_port: u16, ports: &[u16], allow_internet: bool) -> Result<Beacon, String> {
        let code = normalize(code).ok_or("bad room code")?;
        let (sock, port) = ports.iter().find_map(|p| UdpSocket::bind(("0.0.0.0", *p)).ok().map(|s| (s, *p))).ok_or("every room-code port is taken on this PC")?;
        let port = sock.local_addr().map(|a| a.port()).unwrap_or(port);
        let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            let mut seen: HashMap<IpAddr, (Instant, u32)> = HashMap::new();
            while !st.load(Ordering::Relaxed) {
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                if !allow_internet && !crate::team::is_private_ip(from.ip()) {
                    continue;
                }
                // at most 30 answers per address per 10 s
                let e = seen.entry(from.ip()).or_insert((Instant::now(), 0));
                if e.0.elapsed() > Duration::from_secs(10) {
                    *e = (Instant::now(), 0);
                }
                e.1 += 1;
                if e.1 > 30 {
                    continue;
                }
                if seen.len() > 4096 {
                    seen.clear();
                }
                let Ok(text) = std::str::from_utf8(&buf[..n]) else { continue };
                let w: Vec<&str> = text.split(' ').collect();
                if w.len() != 4 || w[0] != MAGIC || w[1] != "FIND" || w[2].len() != 16 || !w[2].chars().all(|c| c.is_ascii_hexdigit()) {
                    continue;
                }
                if w[3] != format!("{:016x}", tag(&["find", &code, w[2]])) {
                    continue; // someone looking for another room
                }
                let _ = sock.send_to(answer(&code, w[2], tcp_port).as_bytes(), from);
            }
        });
        Ok(Beacon { port, stop })
    }
}

/// This PC's IPv4 addresses on the networks a room could be on: the main
/// one and common VPN adapters (found from the routing table; nothing is sent).
pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    let probes = [
        "10.254.254.254",  // the default route
        "100.100.100.100", // Tailscale
        "26.0.0.1",        // Radmin VPN
        "25.0.0.1",        // Hamachi
        "10.147.17.1",     // ZeroTier's usual ranges
        "10.147.18.1",
        "10.147.19.1",
        "10.147.20.1",
        "10.144.0.1",
        "10.241.0.1",
        "10.242.0.1",
        "10.243.0.1",
        "10.244.0.1",
        "172.22.0.1",
        "172.23.0.1",
        "172.24.0.1",
        "172.25.0.1",
        "172.26.0.1",
        "172.27.0.1",
        "172.28.0.1",
        "172.29.0.1",
        "172.30.0.1",
        "192.168.191.1",
        "192.168.192.1",
        "192.168.193.1",
        "192.168.194.1",
        "192.168.195.1",
        "192.168.196.1",
    ];
    let mut out = Vec::new();
    for p in probes {
        let Ok(s) = UdpSocket::bind("0.0.0.0:0") else { continue };
        if s.connect((p, 9)).is_err() {
            continue;
        }
        if let Ok(SocketAddr::V4(a)) = s.local_addr() {
            let ip = *a.ip();
            if !ip.is_unspecified() && !ip.is_loopback() && !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

/// Tailscale doesn't pass broadcasts, so ask its own command-line tool who
/// the other devices are (if Tailscale is installed). Read-only.
fn tailscale_peers() -> Vec<Ipv4Addr> {
    let exes = ["tailscale", r"C:\Program Files\Tailscale\tailscale.exe"];
    for exe in exes {
        let Ok(mut child) =
            std::process::Command::new(exe).args(["status", "--json"]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null()).spawn()
        else {
            continue;
        };
        let t = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if t.elapsed() < Duration::from_secs(2) => std::thread::sleep(Duration::from_millis(30)),
                _ => {
                    let _ = child.kill();
                    return Vec::new();
                }
            }
        }
        let mut text = String::new();
        if let Some(mut so) = child.stdout.take() {
            use std::io::Read;
            let _ = so.by_ref().take(4 << 20).read_to_string(&mut text);
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(peers) = v.get("Peer").and_then(|p| p.as_object()) {
            for p in peers.values().take(1000) {
                for ip in p.get("TailscaleIPs").and_then(|x| x.as_array()).into_iter().flatten() {
                    if let Some(Ok(IpAddr::V4(a))) = ip.as_str().map(|s| s.parse::<IpAddr>()) {
                        out.push(a);
                    }
                }
            }
        }
        return out;
    }
    Vec::new()
}

/// Where to ask: broadcasts on every network this PC is on, this PC itself,
/// and Tailscale devices.
pub fn targets() -> Vec<Ipv4Addr> {
    let mut t = vec![Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST];
    for ip in local_ipv4s() {
        let o = ip.octets();
        let wide = matches!(o[0], 25 | 26); // Hamachi / Radmin use one big /8
        t.push(if wide { Ipv4Addr::new(o[0], 255, 255, 255) } else { Ipv4Addr::new(o[0], o[1], o[2], 255) });
    }
    t.extend(tailscale_peers());
    t.dedup();
    t
}

/// Finds the host of `code`: asks `targets` on `ports` for up to `wait`.
/// Returns the host's Team Create address.
pub fn find(code: &str, targets: &[Ipv4Addr], ports: &[u16], wait: Duration) -> Result<SocketAddr, String> {
    let code = normalize(code).ok_or("That isn't a room code. Room codes are 8 letters and numbers, like ABCD-EF23.")?;
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("Couldn't search the network: {e}"))?;
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(150)));
    let nonce = crate::team::random_nonce();
    let n = format!("{nonce:016x}");
    let q = question(&code, nonce);
    let start = Instant::now();
    let mut last_send: Option<Instant> = None;
    let mut buf = [0u8; 256];
    while start.elapsed() < wait {
        if last_send.is_none_or(|t| t.elapsed() > Duration::from_millis(700)) {
            last_send = Some(Instant::now());
            for ip in targets {
                for p in ports {
                    let _ = sock.send_to(q.as_bytes(), (*ip, *p));
                }
            }
        }
        let Ok((len, from)) = sock.recv_from(&mut buf) else { continue };
        let Ok(text) = std::str::from_utf8(&buf[..len]) else { continue };
        let w: Vec<&str> = text.split(' ').collect();
        if w.len() != 5 || w[0] != MAGIC || w[1] != "HERE" || w[2] != n {
            continue;
        }
        let Ok(port) = w[3].parse::<u16>() else { continue };
        if port == 0 || w[4] != format!("{:016x}", tag(&["here", &code, &n, w[3]])) {
            continue; // not from the room's host
        }
        return Ok(SocketAddr::new(from.ip(), port));
    }
    Err(format!("No room {}-{} answered. Check the code, make sure the host is still hosting, and that you're both on the same network or VPN (Tailscale, ZeroTier, Radmin…).", &code[..4], &code[4..]))
}
