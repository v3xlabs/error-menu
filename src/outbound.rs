use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

pub fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(PublicResolver))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(15))
        .user_agent(concat!(
            "error.menu/",
            env!("CARGO_PKG_VERSION"),
            " (+https://error.menu)"
        ))
        .build()
}

/// Literal IP hosts bypass reqwest's DNS resolver, so every request URL must pass this check.
pub fn validate_url(url: &url::Url) -> io::Result<()> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "outbound URLs must use HTTPS without embedded credentials",
        ));
    }
    validate_host(url.host())
}

pub fn validate_host(host: Option<url::Host<&str>>) -> io::Result<()> {
    match host {
        Some(url::Host::Domain(host)) if !host.is_empty() => Ok(()),
        Some(url::Host::Ipv4(address)) if public_address(address.into()) => Ok(()),
        Some(url::Host::Ipv6(address)) if public_address(address.into()) => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "outbound destination is not public",
        )),
    }
}

pub struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|address| !public_address(address.ip()))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "outbound destination did not resolve exclusively to public addresses",
                )
                .into());
            }
            let addresses: Addrs = Box::new(addresses.into_iter());
            Ok(addresses)
        })
    }
}

fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => public_v4(address),
        IpAddr::V6(address) => public_v6(address),
    }
}

fn public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192
            && (b == 168
                || (b == 0 && matches!(c, 0 | 2))
                || (b == 31 && c == 196)
                || (b == 52 && c == 193)
                || (b == 88 && c == 99)
                || (b == 175 && c == 48)))
        && !(a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}

fn public_v6(address: Ipv6Addr) -> bool {
    let [a, b, c, ..] = address.segments();
    // Only allocated global unicast space is eligible; translation and transition ranges are not.
    (0x2000..=0x3fff).contains(&a)
        && !(a == 0x2001 && (b <= 0x01ff || b == 0x0db8))
        && a != 0x2002
        && !(a == 0x2620 && b == 0x004f && c == 0x8000)
        && !(a == 0x3fff && b <= 0x0fff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_public_destinations_are_rejected() {
        for address in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.9",
            "192.0.2.1",
            "192.31.196.1",
            "192.52.193.1",
            "192.88.99.1",
            "192.168.0.1",
            "192.175.48.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:8.8.8.8",
            "::8.8.8.8",
            "64:ff9b::808:808",
            "64:ff9b:1::1",
            "100::1",
            "2001::1",
            "2001:2::1",
            "2001:20::1",
            "2001:db8::1",
            "2002:7f00:1::1",
            "2620:4f:8000::1",
            "3fff::1",
            "5f00::1",
            "fc00::1",
            "fd00::1",
            "fe80::1",
            "ff02::1",
        ] {
            assert!(!public_address(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn public_addresses_remain_available() {
        for address in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.0.1.1",
            "198.17.255.255",
            "198.20.0.0",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
            "2a01:4f8::1",
        ] {
            assert!(public_address(address.parse().unwrap()), "{address}");
        }
    }

    #[tokio::test]
    async fn domain_resolving_to_loopback_is_rejected() {
        let name = "localhost".parse().unwrap();
        let result = PublicResolver.resolve(name).await;
        assert!(
            matches!(result, Err(error) if error.to_string().contains("did not resolve exclusively to public addresses"))
        );
    }

    #[test]
    fn literal_and_encoded_private_hosts_cannot_bypass_dns_policy() {
        for url in [
            "https://127.0.0.1/avatar",
            "https://2130706433/avatar",
            "https://0x7f000001/avatar",
            "https://127.1/avatar",
            "https://[::1]/avatar",
            "https://[::ffff:127.0.0.1]/avatar",
            "https://user:secret@example.com/avatar",
            "http://example.com/avatar",
        ] {
            assert!(
                validate_url(&url::Url::parse(url).unwrap()).is_err(),
                "{url}"
            );
        }
        assert!(
            validate_url(&url::Url::parse("https://forge.example.com/avatar").unwrap()).is_ok()
        );
    }
}
