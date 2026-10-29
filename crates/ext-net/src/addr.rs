//! Which peer addresses an extension may connect to: public unicast only. A host in `net` can resolve to anything (a
//! DNS answer is the host owner's to choose, and a rebinding attack changes it between two lookups), so the address of
//! the connection itself is what decides.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The kind of address a connection was refused for. `as_str` is the stable name shown in errors and the Inspector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// 127.0.0.0/8, ::1.
    Loopback,
    /// 10/8, 172.16/12, 192.168/16.
    Private,
    /// 100.64.0.0/10, shared by carriers and by some cloud metadata services.
    Cgnat,
    /// 169.254.0.0/16, fe80::/10 (and the old site-local fec0::/10).
    LinkLocal,
    /// 169.254.169.254, fd00:ec2::254: the cloud metadata services, which hand out credentials.
    Metadata,
    /// 0.0.0.0/8, ::.
    Unspecified,
    /// 224.0.0.0/4, ff00::/8, 255.255.255.255.
    Multicast,
    /// 64:ff9b::/96, 64:ff9b:1::/48: NAT64 can reach any IPv4 address, private ones included.
    Nat64,
    /// fc00::/7.
    UniqueLocal,
    /// 192.0.2/24, 198.51.100/24, 203.0.113/24, 2001:db8::/32, 3fff::/20.
    Documentation,
    /// Everything else that is not public unicast: 192.0.0/24, 198.18/15, 240/4, IPv4-compatible and 6to4 addresses,
    /// Teredo, and IPv6 outside 2000::/3.
    Reserved,
}

impl Blocked {
    pub fn as_str(self) -> &'static str {
        match self {
            Blocked::Loopback => "loopback",
            Blocked::Private => "private",
            Blocked::Cgnat => "cgnat",
            Blocked::LinkLocal => "link-local",
            Blocked::Metadata => "metadata",
            Blocked::Unspecified => "unspecified",
            Blocked::Multicast => "multicast",
            Blocked::Nat64 => "nat64",
            Blocked::UniqueLocal => "unique-local",
            Blocked::Documentation => "documentation",
            Blocked::Reserved => "reserved",
        }
    }
}

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `Ok` for a public unicast address; otherwise the range it is in. IPv4-mapped (`::ffff:a.b.c.d`) and
/// IPv4-compatible (`::a.b.c.d`) addresses are judged by the IPv4 address inside.
pub fn check(ip: IpAddr) -> Result<(), Blocked> {
    match ip {
        IpAddr::V4(v4) => check_v4(v4),
        IpAddr::V6(v6) => check_v6(v6),
    }
}

fn check_v4(ip: Ipv4Addr) -> Result<(), Blocked> {
    let [a, b, c, _] = ip.octets();
    let blocked = if ip == Ipv4Addr::new(169, 254, 169, 254) {
        Blocked::Metadata
    } else if a == 0 {
        Blocked::Unspecified
    } else if a == 127 {
        Blocked::Loopback
    } else if a == 10 || (a == 172 && (16..32).contains(&b)) || (a == 192 && b == 168) {
        Blocked::Private
    } else if a == 100 && (64..128).contains(&b) {
        Blocked::Cgnat
    } else if a == 169 && b == 254 {
        Blocked::LinkLocal
    } else if (a == 192 && b == 0 && c == 2) || (a == 198 && b == 51 && c == 100) || (a == 203 && b == 0 && c == 113) {
        Blocked::Documentation
    } else if ip.is_broadcast() || (224..240).contains(&a) {
        Blocked::Multicast
    } else if (a == 192 && b == 0 && c == 0) || (a == 198 && (b == 18 || b == 19)) || a >= 240 {
        Blocked::Reserved
    } else {
        return Ok(());
    };
    Err(blocked)
}

fn check_v6(ip: Ipv6Addr) -> Result<(), Blocked> {
    let s = ip.segments();
    if ip == Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254) {
        return Err(Blocked::Metadata);
    }
    if ip.is_unspecified() {
        return Err(Blocked::Unspecified);
    }
    if ip.is_loopback() {
        return Err(Blocked::Loopback);
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return check_v4(v4);
    }
    // ::a.b.c.d, deprecated and never routed: refused, as the address inside if that is refused too.
    if s[..6] == [0; 6] {
        return Err(check_v4(embedded_v4(s[6], s[7])).err().unwrap_or(Blocked::Reserved));
    }
    if (s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4]) || (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1) {
        return Err(Blocked::Nat64);
    }
    // 6to4 (2002::/16) carries an IPv4 address in its next 32 bits and a relay would deliver to it. It is deprecated,
    // so it is refused either way, named after the address inside when that one is refused too.
    if s[0] == 0x2002 {
        return Err(check_v4(embedded_v4(s[1], s[2])).err().unwrap_or(Blocked::Reserved));
    }
    let blocked = match s[0] {
        0xfe80..=0xfeff => Blocked::LinkLocal,
        0xfc00..=0xfdff => Blocked::UniqueLocal,
        0xff00..=0xffff => Blocked::Multicast,
        0x2001 if s[1] == 0x0db8 => Blocked::Documentation,
        0x3fff if s[1] < 0x1000 => Blocked::Documentation,
        // 2001::/23 is IETF protocol space: Teredo (2001::/32), benchmarking, ORCHID. Global unicast assignments in it
        // (2001:4::/48 AS112, 2001:20::/28 ORCHIDv2) are not web servers either.
        0x2001 if s[1] < 0x0200 => Blocked::Reserved,
        0x2000..=0x3fff => return Ok(()),
        _ => Blocked::Reserved,
    };
    Err(blocked)
}

fn embedded_v4(hi: u16, lo: u16) -> Ipv4Addr {
    let [a, b] = hi.to_be_bytes();
    let [c, d] = lo.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}
