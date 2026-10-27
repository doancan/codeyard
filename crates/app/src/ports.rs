//! Port forwarding: find the ports listening on the server and bind them to a local port.
//! Binding goes through ssh's open ControlMaster connection (`host::forward`).
//!
//! Everything here reads text produced on the server, so the parsers accept any input: odd lines are skipped, the
//! number of entries and the length of process names are capped and control characters are dropped.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use host::Host;
use host::forward::RemoteTarget;

/// Most entries kept from one scan (a busy server lists thousands of sockets; only listeners are kept anyway).
const MAX_ENTRIES: usize = 5000;
/// Longest process name shown.
const MAX_NAME: usize = 64;

/// Address a port listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BindAddr {
    Loopback4,
    Loopback6,
    Any4,
    Any6,
    Specific(IpAddr),
}

impl BindAddr {
    pub fn from_ip(ip: IpAddr) -> Self {
        match ip {
            // Only the usual loopback address: a listener on another 127.x address must be reached exactly there.
            IpAddr::V4(a) if a == Ipv4Addr::LOCALHOST => BindAddr::Loopback4,
            IpAddr::V4(a) if a.is_unspecified() => BindAddr::Any4,
            IpAddr::V6(a) if a.is_loopback() => BindAddr::Loopback6,
            IpAddr::V6(a) if a.is_unspecified() => BindAddr::Any6,
            // IPv4-mapped (`::ffff:127.0.0.1`) is the IPv4 address.
            IpAddr::V6(a) => a.to_ipv4_mapped().map_or(BindAddr::Specific(ip), |v4| BindAddr::from_ip(IpAddr::V4(v4))),
            other => BindAddr::Specific(other),
        }
    }
}

/// The program that owns a listening socket (when the scan can see it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}

/// One listening port: every address it is bound on and its program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListeningPort {
    pub port: u16,
    pub binds: Vec<BindAddr>,
    pub process: Option<ProcessInfo>,
}

/// How the server's listening ports are read; the first that works is kept for the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMethod {
    /// `ss -H -ltnp` (Linux): addresses and programs.
    Ss,
    /// `/proc/net/tcp*` (Linux without `ss`): addresses only.
    Proc,
    /// `lsof` (macOS and BSD servers): addresses and programs.
    Lsof,
}

impl ScanMethod {
    const ALL: [ScanMethod; 3] = [ScanMethod::Ss, ScanMethod::Proc, ScanMethod::Lsof];

    fn command(self) -> (&'static str, &'static [&'static str]) {
        match self {
            ScanMethod::Ss => ("ss", &["-H", "-ltnp"]),
            ScanMethod::Proc => ("cat", &["/proc/net/tcp", "/proc/net/tcp6"]),
            ScanMethod::Lsof => ("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"]),
        }
    }

    fn parse(self, text: &str) -> Vec<ListeningPort> {
        match self {
            ScanMethod::Ss => parse_ss(text),
            ScanMethod::Proc => parse_proc_net_entries(text),
            ScanMethod::Lsof => parse_lsof_entries(text),
        }
    }
}

/// Program name as shown: printable, one line, capped.
pub fn clean_name(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    cleaned.trim().to_string()
}

/// `host:port` as `ss`/`lsof` print it: `127.0.0.1:8080`, `[::1]:3000`, `*:22`, `127.0.0.53%lo:53`, `[::]:22`.
fn parse_socket(addr: &str) -> Option<(BindAddr, u16)> {
    let (host, port) = addr.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host = host.split('%').next().unwrap_or(host); // scope id
    let bind = match host {
        "*" | "0.0.0.0" => BindAddr::Any4,
        "::" => BindAddr::Any6,
        h => BindAddr::from_ip(h.parse().ok()?),
    };
    Some((bind, port))
}

/// `ss -H -ltnp`: `LISTEN 0 4096 127.0.0.1:5432 0.0.0.0:* users:(("postgres",pid=812,fd=6))`.
pub fn parse_ss(text: &str) -> Vec<ListeningPort> {
    let entries = text.lines().take(MAX_ENTRIES * 4).filter_map(|line| {
        let mut cols = line.split_whitespace();
        if cols.next()? != "LISTEN" {
            return None;
        }
        let local = cols.nth(2)?;
        let (bind, port) = parse_socket(local)?;
        let process = line.split_once("users:((\"").and_then(|(_, rest)| {
            let (name, rest) = rest.split_once("\",")?;
            let pid = rest.split_once("pid=")?.1.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
            Some(ProcessInfo { pid, name: clean_name(name) })
        });
        Some(ListeningPort { port, binds: vec![bind], process })
    });
    merge(entries.collect())
}

/// One `/proc/net/tcp*` address (`0100007F:0BB8`): the hex words are in host (little-endian) byte order.
fn parse_proc_addr(field: &str) -> Option<(BindAddr, u16)> {
    let (addr, port) = field.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |s: &str| u32::from_str_radix(s, 16).ok().map(u32::swap_bytes);
    let ip = match addr.len() {
        8 => IpAddr::V4(Ipv4Addr::from(word(addr)?)),
        32 if addr.is_ascii() => {
            let mut bytes = [0u8; 16];
            for i in 0..4 {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&word(&addr[i * 8..i * 8 + 8])?.to_be_bytes());
            }
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
        _ => return None,
    };
    Some((BindAddr::from_ip(ip), port))
}

/// `/proc/net/tcp` and `tcp6`: sockets in state 0A (LISTEN).
pub fn parse_proc_net_entries(text: &str) -> Vec<ListeningPort> {
    let entries = text.lines().take(MAX_ENTRIES * 4).filter_map(|l| {
        let mut cols = l.split_whitespace();
        let _sl = cols.next()?;
        let local = cols.next()?;
        let _remote = cols.next()?;
        (cols.next()? == "0A").then_some(())?;
        let (bind, port) = parse_proc_addr(local)?;
        Some(ListeningPort { port, binds: vec![bind], process: None })
    });
    merge(entries.collect())
}

/// `lsof -nP -iTCP -sTCP:LISTEN -Fpcn`: `p<pid>` and `c<command>` apply to the `n<address>` lines after them.
pub fn parse_lsof_entries(text: &str) -> Vec<ListeningPort> {
    let mut out = Vec::new();
    let (mut pid, mut name): (Option<u32>, Option<String>) = (None, None);
    for line in text.lines().take(MAX_ENTRIES * 4) {
        match line.split_at_checked(1) {
            Some(("p", v)) => (pid, name) = (v.parse().ok(), None),
            Some(("c", v)) => name = Some(clean_name(v)),
            Some(("n", v)) => {
                if let Some((bind, port)) = parse_socket(v) {
                    let process = pid.zip(name.clone()).map(|(pid, name)| ProcessInfo { pid, name });
                    out.push(ListeningPort { port, binds: vec![bind], process });
                }
            }
            _ => {}
        }
    }
    merge(out)
}

/// One entry per port, sorted: addresses joined, the first known program kept.
pub fn merge(entries: Vec<ListeningPort>) -> Vec<ListeningPort> {
    let mut by_port: std::collections::BTreeMap<u16, ListeningPort> = std::collections::BTreeMap::new();
    for e in entries {
        if by_port.len() >= MAX_ENTRIES && !by_port.contains_key(&e.port) {
            continue;
        }
        let slot = by_port.entry(e.port).or_insert_with(|| ListeningPort { port: e.port, binds: Vec::new(), process: None });
        for b in e.binds {
            if !slot.binds.contains(&b) {
                slot.binds.push(b);
            }
        }
        if slot.process.is_none() {
            slot.process = e.process;
        }
    }
    by_port
        .into_values()
        .map(|mut p| {
            p.binds.sort();
            p
        })
        .collect()
}

/// Where a forward to this port should connect on the server. A listener on one loopback family is targeted exactly
/// (`localhost` could resolve to the other one); anything wider is reached through `localhost`.
pub fn forward_target(binds: &[BindAddr]) -> RemoteTarget {
    match binds {
        [BindAddr::Loopback4] => RemoteTarget::V4(Ipv4Addr::LOCALHOST),
        [BindAddr::Loopback6] => RemoteTarget::V6(Ipv6Addr::LOCALHOST),
        [BindAddr::Specific(IpAddr::V4(a))] => RemoteTarget::V4(*a),
        [BindAddr::Specific(IpAddr::V6(a))] => RemoteTarget::V6(*a),
        _ => RemoteTarget::Localhost,
    }
}

/// Listening ports on the server. `method` remembers the scan that worked (cleared to probe again after a reconnect);
/// an error means no scan could run, so the caller keeps its last list instead of emptying it.
pub fn detect(host: &dyn Host, method: &mut Option<ScanMethod>) -> Result<Vec<ListeningPort>, String> {
    let order: Vec<ScanMethod> = match *method {
        Some(m) => vec![m],
        None => ScanMethod::ALL.to_vec(),
    };
    let mut last = String::from("no way to list ports on the server");
    for m in order {
        let (program, args) = m.command();
        match host.exec(program, args, None, None) {
            Ok(out) if out.ok() => {
                *method = Some(m);
                return Ok(m.parse(&out.stdout_str()));
            }
            Ok(out) => last = format!("{program}: {}", clean_name(out.stderr_str().lines().next().unwrap_or(""))),
            Err(e) => last = e.to_string(),
        }
    }
    *method = None;
    Err(last)
}

/// Port numbers of `/proc/net/tcp*` listeners.
#[cfg(test)]
pub fn parse_proc_net(text: &str) -> Vec<u16> {
    parse_proc_net_entries(text).into_iter().map(|p| p.port).collect()
}

/// Port numbers of `lsof -Fn` listeners.
#[cfg(test)]
pub fn parse_lsof(text: &str) -> Vec<u16> {
    parse_lsof_entries(text).into_iter().map(|p| p.port).collect()
}

/// Auto-forward everything except privileged ports and ssh.
pub fn worth_forwarding(port: u16) -> bool {
    port >= 1024 && port != 22
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listening_ports() {
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1 1\n\
   1: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2 1\n\
   2: 0100007F:0BB8 0100007F:D2F0 01 00000000:00000000 00:00000000 00000000  1000        0 3 1\n\
   3: 00000000000000000000000000000000:1F90 00000000000000000000000000000000:0000 0A 0 0 0 0 0 4 1\n";
        assert_eq!(parse_proc_net(tcp), vec![22, 3000, 8080], "established (01) connections are not counted");
        assert_eq!(parse_lsof("p123\nf5\nn*:3000\nn127.0.0.1:5432\nn[::1]:3000\n"), vec![3000, 5432]);
        assert!(!worth_forwarding(22) && !worth_forwarding(80) && worth_forwarding(5173));
    }

    #[test]
    fn proc_net_addresses_are_decoded() {
        let line = |addr: &str| format!("0: {addr} 00000000:0000 0A 0 0 0 0 0 1 1\n");
        let one = |addr: &str| parse_proc_net_entries(&line(addr)).remove(0);
        assert_eq!(one("0100007F:0BB8").binds, vec![BindAddr::Loopback4]);
        assert_eq!(one("00000000:0BB8").binds, vec![BindAddr::Any4]);
        assert_eq!(one("00000000000000000000000001000000:0BB8").binds, vec![BindAddr::Loopback6]);
        assert_eq!(one("00000000000000000000000000000000:0BB8").binds, vec![BindAddr::Any6]);
        assert_eq!(one("0000000000000000FFFF00000100007F:0BB8").binds, vec![BindAddr::Loopback4], "IPv4-mapped");
        assert_eq!(one("0A00020F:0BB8").binds, vec![BindAddr::Specific("15.2.0.10".parse().unwrap())]);
    }

    #[test]
    fn ss_lines_give_addresses_and_programs() {
        let text = "LISTEN 0      4096   127.0.0.53%lo:53        0.0.0.0:*    users:((\"systemd-resolve\",pid=566,fd=14))\n\
LISTEN 0      511          [::1]:3000          [::]:*    users:((\"node\",pid=4242,fd=21))\n\
LISTEN 0      511        127.0.0.1:3000     0.0.0.0:*    users:((\"node\",pid=4242,fd=20))\n\
LISTEN 0      128             [::]:22             [::]:*\n\
ESTAB  0      0        10.0.0.2:22     10.0.0.9:51000\n\
LISTEN 0      5    *:8000 *:* users:((\"py,(\\\"x\",pid=7,fd=3))\n";
        let ports = parse_ss(text);
        assert_eq!(ports.iter().map(|p| p.port).collect::<Vec<_>>(), vec![22, 53, 3000, 8000]);
        let node = &ports[2];
        assert_eq!(node.binds, vec![BindAddr::Loopback4, BindAddr::Loopback6]);
        assert_eq!(node.process, Some(ProcessInfo { pid: 4242, name: "node".into() }));
        assert_eq!(ports[0].process, None);
        assert_eq!(ports[1].binds, vec![BindAddr::Specific("127.0.0.53".parse().unwrap())]);
        assert!(ports[3].process.as_ref().is_some_and(|p| p.pid == 7), "odd program names still parse: {:?}", ports[3]);
    }

    #[test]
    fn lsof_programs_apply_to_the_addresses_after_them() {
        let text = "n*:9999\np100\ncpostgres\nn127.0.0.1:5432\nn[::1]:5432\np200\ncvite\x1b[31m\nn*:5173\n";
        let ports = parse_lsof_entries(text);
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0].process.as_ref().map(|p| p.name.as_str()), Some("vite[31m"), "control characters are dropped");
        assert_eq!(ports[1].binds, vec![BindAddr::Loopback4, BindAddr::Loopback6]);
        assert_eq!(ports[2].process, None, "an address before any program has none");
    }

    #[test]
    fn the_forward_target_follows_the_listening_address() {
        assert_eq!(forward_target(&[BindAddr::Loopback4]), RemoteTarget::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(forward_target(&[BindAddr::Loopback6]), RemoteTarget::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(forward_target(&[BindAddr::Loopback4, BindAddr::Loopback6]), RemoteTarget::Localhost);
        assert_eq!(forward_target(&[BindAddr::Any6]), RemoteTarget::Localhost);
        assert_eq!(forward_target(&[]), RemoteTarget::Localhost, "a port typed by hand");
        let resolver = parse_ss("LISTEN 0 4096 127.0.0.53%lo:5353 0.0.0.0:*\n");
        assert_eq!(forward_target(&resolver[0].binds), RemoteTarget::V4("127.0.0.53".parse().unwrap()), "exactly where it listens");
    }

    #[test]
    fn hostile_scan_output_is_survived_and_capped() {
        let garbage = [
            "0: ZZZZZZZZ:0BB8 0 0A",
            "0: 0100007F:FFFFF 0 0A",
            "0: 0100007:0BB8 0 0A",
            "0: \u{0}\u{0}:\u{0} 0 0A",
            "LISTEN 0 0 [::1:80",
            "LISTEN 0 0 1.2.3.4:99999 x",
            "LISTEN",
            "p\nc\nn",
            "nfoo:bar",
        ];
        for g in garbage {
            let _ = (parse_proc_net_entries(g), parse_ss(g), parse_lsof_entries(g));
        }
        let huge = "LISTEN 0 0 127.0.0.1:3000 *\n".repeat(200_000);
        assert_eq!(parse_ss(&huge).len(), 1);
        let many: String = (1..=60_000u32).map(|p| format!("LISTEN 0 0 127.0.0.1:{} *\n", p % 65_535 + 1)).collect();
        assert!(parse_ss(&many).len() <= MAX_ENTRIES);
        let long = format!("p1\nc{}\nn*:1\n", "x".repeat(1_000_000));
        assert_eq!(parse_lsof_entries(&long)[0].process.as_ref().unwrap().name.chars().count(), MAX_NAME);
    }
}
