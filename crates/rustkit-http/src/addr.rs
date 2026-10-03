//! Resolve, vet, connect.
//!
//! Name resolution happens exactly once, here, and every address the resolver
//! returns is checked against the client's [`AddressPolicy`] BEFORE a socket
//! is opened. The connect then goes to the vetted `SocketAddr`s, never back
//! through the name, so a DNS answer that changes between "check" and "use"
//! (rebinding) cannot reach a private address.

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;

use tokio::net::TcpStream;

use crate::HttpError;

/// Which resolved addresses a client may connect to.
#[derive(Clone, Default)]
pub enum AddressPolicy {
    /// No restriction (the historical behaviour; subresources and downloads).
    #[default]
    Any,
    /// Globally routable unicast addresses only: loopback, RFC 1918,
    /// link-local, unique-local, CGNAT, unspecified, multicast and
    /// documentation/benchmark ranges are refused.
    PublicOnly,
    /// A caller-supplied rule over the resolved socket address.
    Custom(Arc<dyn Fn(&SocketAddr) -> bool + Send + Sync>),
}

impl std::fmt::Debug for AddressPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressPolicy::Any => f.write_str("Any"),
            AddressPolicy::PublicOnly => f.write_str("PublicOnly"),
            AddressPolicy::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

impl AddressPolicy {
    /// May the client connect to this resolved address?
    pub fn permits(&self, addr: &SocketAddr) -> bool {
        match self {
            AddressPolicy::Any => true,
            AddressPolicy::PublicOnly => is_public_ip(addr.ip()),
            AddressPolicy::Custom(rule) => rule(addr),
        }
    }
}

/// Is this a globally routable unicast address?
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4.octets()),
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            let o = v6.octets();
            // IPv4-mapped (::ffff:a.b.c.d): judged as the embedded v4 address.
            if seg[..5] == [0, 0, 0, 0, 0] && seg[5] == 0xffff {
                return is_public_v4([o[12], o[13], o[14], o[15]]);
            }
            // NAT64 well-known prefix 64:ff9b::/96: judged as the embedded v4.
            if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
                return is_public_v4([o[12], o[13], o[14], o[15]]);
            }
            // 6to4 (2002::/16) embeds the v4 address in bits 16..48.
            if seg[0] == 0x2002 {
                return is_public_v4([o[2], o[3], o[4], o[5]]);
            }
            let first = seg[0];
            !(v6.is_unspecified()
                || v6.is_loopback()
                || first & 0xff00 == 0xff00 // multicast ff00::/8
                || first & 0xfe00 == 0xfc00 // unique-local fc00::/7
                || first & 0xffc0 == 0xfe80 // link-local fe80::/10
                || first & 0xffc0 == 0xfec0 // deprecated site-local fec0::/10
                || (seg[0] == 0x0100 && seg[1..4] == [0, 0, 0]) // discard-only 100::/64
                || seg[0] == 0x2001 && (seg[1] < 0x0200 || seg[1] == 0x0db8) // 2001::/23 protocol assignments, 2001:db8::/32 docs
                || seg[0] == 0x64 && seg[1] == 0xff9b && seg[2] == 1) // 64:ff9b:1::/48 local-use NAT64
        }
    }
}

fn is_public_v4(o: [u8; 4]) -> bool {
    !(o[0] == 0 // 0.0.0.0/8
        || o[0] == 10
        || o[0] == 127
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
        || (o[0] == 169 && o[1] == 254)
        || (o[0] == 172 && (o[1] & 0xf0) == 16)
        || (o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2)) // 192.0.0.0/24, 192.0.2.0/24
        || (o[0] == 192 && o[1] == 88 && o[2] == 99) // 6to4 relay anycast
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15 benchmarking
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || o[0] >= 224) // multicast, reserved, broadcast
}

pub fn is_local_name(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost")
}

pub type ResolveFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>>;

/// Name resolution, injectable so tests can answer for a name without DNS.
pub trait Resolve: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a>;
}

/// The platform resolver.
pub struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a> {
        Box::pin(async move { Ok(tokio::net::lookup_host((host, port)).await?.collect()) })
    }
}

pub(crate) async fn connect_vetted(
    resolver: &dyn Resolve,
    policy: &AddressPolicy,
    host: &str,
    port: u16,
) -> Result<TcpStream, HttpError> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    // localhost is private by definition; refuse it by name so no lookup runs
    // and a resolver that answers something else cannot make it public.
    if matches!(policy, AddressPolicy::PublicOnly) && is_local_name(host) {
        return Err(HttpError::AddressDenied(format!("{host} is a local name")));
    }
    let addrs: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => resolver
            .resolve(host, port)
            .await
            .map_err(|e| HttpError::ConnectionFailed(e.to_string()))?,
    };
    if addrs.is_empty() {
        return Err(HttpError::ConnectionFailed(format!("{host}: no addresses")));
    }
    for addr in &addrs {
        if !policy.permits(addr) {
            return Err(HttpError::AddressDenied(format!("{host} resolves to {}", addr.ip())));
        }
    }
    TcpStream::connect(&addrs[..])
        .await
        .map_err(|e| HttpError::ConnectionFailed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::net::TcpListener;

    struct Stub(HashMap<&'static str, Vec<IpAddr>>);

    impl Resolve for Stub {
        fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a> {
            let out = self
                .0
                .get(host)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect();
            Box::pin(async move { Ok(out) })
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    async fn listener() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        (l, p)
    }

    async fn nobody_connected(l: &TcpListener) -> bool {
        tokio::time::timeout(Duration::from_millis(150), l.accept()).await.is_err()
    }

    #[test]
    fn non_public_ranges_are_not_public() {
        for s in [
            "127.0.0.1", "127.255.255.254", "0.0.0.0", "10.0.0.1", "172.16.0.1", "172.31.255.255",
            "192.168.1.1", "169.254.169.254", "100.64.0.1", "100.127.255.255", "198.18.0.1",
            "192.0.0.1", "192.0.2.1", "198.51.100.1", "203.0.113.1", "224.0.0.1", "240.0.0.1",
            "255.255.255.255", "::", "::1", "fe80::1", "fc00::1", "fd12:3456::1", "fec0::1",
            "ff02::1", "2001:db8::1", "2001::1", "2002:7f00:1::", "::ffff:127.0.0.1",
            "::ffff:10.0.0.1", "64:ff9b::7f00:1", "100::1",
        ] {
            assert!(!is_public_ip(ip(s)), "{s} must not be public");
        }
    }

    #[test]
    fn routable_addresses_are_public() {
        for s in [
            "8.8.8.8", "1.1.1.1", "93.184.216.34", "172.15.255.255", "172.32.0.1", "100.63.255.255",
            "100.128.0.1", "198.17.255.255", "198.20.0.1", "2606:4700:4700::1111",
            "2a00:1450:4001:81b::200e", "::ffff:8.8.8.8",
        ] {
            assert!(is_public_ip(ip(s)), "{s} must be public");
        }
    }

    #[tokio::test]
    async fn public_only_refuses_ip_literals_without_opening_a_socket() {
        let (l, port) = listener().await;
        for host in ["127.0.0.1", "0.0.0.0", "[::1]", "10.0.0.1", "[::ffff:127.0.0.1]"] {
            let r = connect_vetted(&Stub(HashMap::new()), &AddressPolicy::PublicOnly, host, port).await;
            assert!(matches!(r, Err(HttpError::AddressDenied(_))), "{host}: {r:?}");
        }
        assert!(nobody_connected(&l).await, "a socket was opened to a refused address");
    }

    #[tokio::test]
    async fn public_only_refuses_localhost_by_name_without_a_lookup() {
        let (l, port) = listener().await;
        let stub = Stub(HashMap::from([("localhost", vec![ip("8.8.8.8")])]));
        for host in ["localhost", "LOCALHOST", "app.localhost", "localhost."] {
            let r = connect_vetted(&stub, &AddressPolicy::PublicOnly, host, port).await;
            assert!(matches!(r, Err(HttpError::AddressDenied(_))), "{host}: {r:?}");
        }
        assert!(nobody_connected(&l).await);
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_loopback_is_refused_before_any_connect() {
        let (l, port) = listener().await;
        let stub = Stub(HashMap::from([("rebind.test", vec![ip("127.0.0.1")])]));
        let r = connect_vetted(&stub, &AddressPolicy::PublicOnly, "rebind.test", port).await;
        assert!(matches!(r, Err(HttpError::AddressDenied(_))), "{r:?}");
        assert!(nobody_connected(&l).await, "the loopback listener saw a connection");
    }

    #[tokio::test]
    async fn one_private_answer_among_public_ones_refuses_the_whole_name() {
        let (l, port) = listener().await;
        let stub = Stub(HashMap::from([(
            "mixed.test",
            vec![ip("93.184.216.34"), ip("127.0.0.1")],
        )]));
        let r = connect_vetted(&stub, &AddressPolicy::PublicOnly, "mixed.test", port).await;
        assert!(matches!(r, Err(HttpError::AddressDenied(_))), "{r:?}");
        assert!(nobody_connected(&l).await);
    }

    #[tokio::test]
    async fn any_policy_still_connects() {
        let (l, port) = listener().await;
        let stub = Stub(HashMap::from([("ok.test", vec![ip("127.0.0.1")])]));
        let accept = tokio::spawn(async move { l.accept().await.is_ok() });
        connect_vetted(&stub, &AddressPolicy::Any, "ok.test", port).await.expect("connect");
        assert!(accept.await.unwrap());
    }

    #[tokio::test]
    async fn a_custom_rule_decides_per_socket_address() {
        let (l, port) = listener().await;
        let only = Arc::new(move |a: &SocketAddr| a.port() == port);
        let deny = AddressPolicy::Custom(Arc::new(|_| false));
        let r = connect_vetted(&Stub(HashMap::new()), &deny, "127.0.0.1", port).await;
        assert!(matches!(r, Err(HttpError::AddressDenied(_))));
        let accept = tokio::spawn(async move { l.accept().await.is_ok() });
        connect_vetted(&Stub(HashMap::new()), &AddressPolicy::Custom(only), "127.0.0.1", port)
            .await
            .expect("custom rule allows this port");
        assert!(accept.await.unwrap());
    }
}
