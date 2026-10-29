use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use super::{TransportError, TransportPolicy};

pub(super) fn safe_header(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with("proxy-")
        && !name.starts_with("sec-")
        && !name.starts_with("x-forwarded-")
        && !matches!(
            name,
            "authorization"
                | "www-authenticate"
                | "authentication-info"
                | "cookie"
                | "cookie2"
                | "set-cookie"
                | "set-cookie2"
                | "host"
                | "connection"
                | "keep-alive"
                | "transfer-encoding"
                | "te"
                | "trailer"
                | "upgrade"
                | "content-length"
                | "expect"
                | "forwarded"
                | "via"
                | "x-api-key"
                | "api-key"
                | "accept-encoding"
        )
}

pub(super) fn check_addresses(
    addresses: &[SocketAddr],
    policy: &TransportPolicy,
) -> Result<(), TransportError> {
    if addresses.is_empty() || addresses.len() > 64 {
        return Err(TransportError::Dns);
    }
    // Reject the complete answer if even one candidate is disallowed. Filtering
    // the answer would hide mixed public/private DNS or rebinding attempts.
    for address in addresses {
        let public = match address.ip() {
            IpAddr::V4(ip) => public_ipv4(ip),
            IpAddr::V6(ip) => public_ipv6(ip),
        };
        if !public || policy.denied_addresses.contains(&address.ip()) {
            return Err(TransportError::Denied);
        }
    }
    Ok(())
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip != Ipv4Addr::new(168, 63, 129, 16)
        && !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && (b == 0 || b == 168 || (b == 88 && c == 99)))
        && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}

fn public_ipv6(ip: Ipv6Addr) -> bool {
    let [a, b, ..] = ip.segments();
    (0x2000..=0x3fff).contains(&a)
        && !(a == 0x2001 && (b <= 0x01ff || b == 0x0db8))
        && a != 0x2002
        && !(a == 0x3fff && b <= 0x0fff)
}

/// The HTTP connector cannot fall back to the OS resolver. TLS still receives
/// the original URL hostname for certificate verification and SNI.
pub(super) struct PinnedResolver {
    host: String,
    addresses: Vec<SocketAddr>,
}

impl PinnedResolver {
    pub(super) fn new(host: String, addresses: Vec<SocketAddr>) -> Self {
        Self { host, addresses }
    }
}

impl Resolve for PinnedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let matches = name.as_str() == self.host;
        let addresses = self.addresses.clone();
        Box::pin(async move {
            if !matches {
                return Err(
                    io::Error::new(io::ErrorKind::PermissionDenied, "destination denied").into(),
                );
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}
