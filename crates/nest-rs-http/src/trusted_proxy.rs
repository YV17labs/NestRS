//! [`TrustedProxy`]: one entry of the proxies a deployment believes, an address
//! or a CIDR range, parsed once at boot and compared per request.

use std::borrow::Cow;
use std::net::IpAddr;
use std::str::FromStr;

use nest_rs_config::FromSetting;

use crate::error::NotATrustedProxy;

/// One entry of [`HttpConfig::trusted_proxies`](crate::HttpConfig::trusted_proxies):
/// a proxy's address, or the CIDR range an ingress's addresses move within.
///
/// An IPv4-mapped address is the IPv4 address it carries, so `10.0.0.1` names
/// the proxy a dual-stack listener reports as `::ffff:10.0.0.1`. No value of
/// this type holds a range of every address: `/0` is refused, since it would
/// believe every caller's forwarding headers.
///
/// A `?` needs a function returning `Result`, and a `#[module]`'s imports are
/// none: there a range is `.parse().expect("a CIDR range")`, as `pinned` writes it.
///
/// ```
/// use std::net::IpAddr;
/// use nest_rs_http::{HttpConfig, TrustedProxy};
///
/// let range: TrustedProxy = "10.0.0.0/8".parse()?;
/// let mapped: TrustedProxy = "::ffff:10.0.0.0/104".parse()?;
/// assert_eq!(range, mapped);
///
/// let proxy = TrustedProxy::from(IpAddr::from([10, 0, 0, 1]));
/// assert_eq!(proxy, "10.0.0.1/32".parse()?);
///
/// assert!("0.0.0.0/0".parse::<TrustedProxy>().is_err());
///
/// let pinned = HttpConfig {
///     trusted_proxies: vec![
///         IpAddr::from([10, 0, 0, 1]).into(),
///         "10.42.0.0/16".parse().expect("a CIDR range"),
///     ],
///     ..HttpConfig::default()
/// };
/// # assert_eq!(pinned.trusted_proxies[0], proxy);
/// # Ok::<(), nest_rs_http::NotATrustedProxy>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrustedProxy {
    network: IpAddr,
    /// From 1, so the bits it leaves out of a comparison are a valid shift.
    prefix: u8,
}

impl TrustedProxy {
    /// `address` and its `prefix` length, a mapped range being the IPv4 range
    /// it carries; or the way they are no range this type holds.
    fn range(address: IpAddr, prefix: u8) -> Result<Self, NotATrustedProxy> {
        let (network, prefix) = if let IpAddr::V6(v6) = address
            && prefix >= MAPPED_PREFIX
            && let Some(v4) = v6.to_ipv4_mapped()
        {
            (IpAddr::V4(v4), prefix - MAPPED_PREFIX)
        } else {
            (address, prefix)
        };
        if prefix > full_length(network) {
            return Err(NotATrustedProxy::PrefixLength);
        }
        if prefix == 0 {
            return Err(NotATrustedProxy::EveryAddress);
        }
        let past_the_prefix = match network {
            IpAddr::V4(v4) => v4.to_bits().checked_shl(prefix.into()).unwrap_or(0) != 0,
            IpAddr::V6(v6) => v6.to_bits().checked_shl(prefix.into()).unwrap_or(0) != 0,
        };
        if past_the_prefix {
            return Err(NotATrustedProxy::BitsPastPrefix);
        }
        Ok(Self { network, prefix })
    }

    /// Whether `ip`, in canonical form, is this proxy or lies in this range.
    /// An address of the other family never does.
    fn contains(self, ip: IpAddr) -> bool {
        let prefix = u32::from(self.prefix);
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                (network.to_bits() ^ ip.to_bits()) >> (u32::BITS - prefix) == 0
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                (network.to_bits() ^ ip.to_bits()) >> (u128::BITS - prefix) == 0
            }
            _ => false,
        }
    }
}

/// `::ffff:0:0/96`, the prefix every IPv4-mapped address shares (RFC 4291
/// §2.5.5.2).
const MAPPED_PREFIX: u8 = 96;

/// How many bits an address of `network`'s family holds.
const fn full_length(network: IpAddr) -> u8 {
    match network {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    }
}

/// Whether `proxies` name `ip`, the one comparison every reader of the list
/// makes: a mapped address reads as the IPv4 address it carries.
pub(crate) fn lists(proxies: &[TrustedProxy], ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    proxies.iter().any(|proxy| proxy.contains(ip))
}

impl From<IpAddr> for TrustedProxy {
    /// The proxy at `address` alone.
    fn from(address: IpAddr) -> Self {
        let network = address.to_canonical();
        Self {
            network,
            prefix: full_length(network),
        }
    }
}

impl FromStr for TrustedProxy {
    type Err = NotATrustedProxy;

    /// An IP address, or one and its prefix length (`10.0.0.0/8`, `fd00::/8`).
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let address = |text: &str| text.parse::<IpAddr>().ok().ok_or(NotATrustedProxy::Grammar);
        let Some((network, prefix)) = text.split_once('/') else {
            return address(text).map(Self::from);
        };
        // `u8`'s parser takes a leading `+`, which no CIDR notation writes.
        let prefix = Some(prefix)
            .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<u8>().ok())
            .ok_or(NotATrustedProxy::Grammar)?;
        Self::range(address(network)?, prefix)
    }
}

impl FromSetting for TrustedProxy {
    fn from_setting(text: &str) -> Result<Self, Cow<'static, str>> {
        text.parse()
            .map_err(|refused: NotATrustedProxy| refused.predicate().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("a test literal is an address")
    }

    fn proxy(s: &str) -> TrustedProxy {
        s.parse().expect("a test literal is an entry")
    }

    #[test]
    fn a_range_holds_every_address_under_its_prefix_and_no_other() {
        for (entry, inside, outside) in [
            ("10.0.0.0/8", "10.255.255.255", "11.0.0.0"),
            ("10.0.0.0/8", "10.0.0.0", "9.255.255.255"),
            ("192.168.1.128/25", "192.168.1.128", "192.168.1.127"),
            ("128.0.0.0/1", "255.255.255.255", "127.255.255.255"),
            ("10.0.0.1/32", "10.0.0.1", "10.0.0.2"),
            ("10.0.0.1", "10.0.0.1", "10.0.0.0"),
            ("fd00::/8", "fdff:ffff::1", "fe00::"),
            ("2001:db8::/33", "2001:db8:7fff::", "2001:db8:8000::"),
            ("8000::/1", "ffff::", "7fff::"),
            ("2001:db8::1/128", "2001:db8::1", "2001:db8::2"),
            ("2001:db8::1", "2001:db8::1", "2001:db8::"),
        ] {
            let listed = [proxy(entry)];
            assert!(lists(&listed, ip(inside)), "{entry} holds {inside}");
            assert!(!lists(&listed, ip(outside)), "{entry} holds no {outside}");
        }
    }

    /// A family's range never answers for the other family, whichever range
    /// its bits would fall in.
    #[test]
    fn a_range_holds_no_address_of_the_other_family() {
        assert!(!lists(&[proxy("::/1")], ip("10.0.0.1")));
        assert!(!lists(&[proxy("::/1")], ip("::ffff:10.0.0.1")));
        assert!(!lists(&[proxy("10.0.0.0/8")], ip("a00::1")));
    }

    /// A mapped peer, a mapped entry and a mapped range are the IPv4 they carry.
    #[test]
    fn a_mapped_address_or_range_is_the_ipv4_it_carries() {
        for entry in ["10.0.0.1", "::ffff:10.0.0.1", "::ffff:10.0.0.1/128"] {
            assert_eq!(proxy(entry), proxy("10.0.0.1/32"), "{entry}");
            assert!(lists(&[proxy(entry)], ip("10.0.0.1")), "{entry}");
            assert!(lists(&[proxy(entry)], ip("::ffff:10.0.0.1")), "{entry}");
        }
        assert_eq!(proxy("::ffff:10.0.0.0/104"), proxy("10.0.0.0/8"));
        assert!(lists(&[proxy("10.0.0.0/8")], ip("::ffff:10.1.2.3")));
        assert_eq!(
            TrustedProxy::from(ip("::ffff:10.0.0.1")),
            TrustedProxy::from(ip("10.0.0.1"))
        );
        assert_eq!(TrustedProxy::from(ip("10.0.0.1")), proxy("10.0.0.1"));
        assert_eq!(TrustedProxy::from(ip("::1")), proxy("::1/128"));
    }

    #[test]
    fn a_value_that_names_no_proxy_is_refused_by_kind() {
        for (entry, kind) in [
            ("", NotATrustedProxy::Grammar),
            ("proxy.example", NotATrustedProxy::Grammar),
            ("10.0.0.oops", NotATrustedProxy::Grammar),
            ("10.0.0.0/", NotATrustedProxy::Grammar),
            ("/8", NotATrustedProxy::Grammar),
            ("10.0.0.0/+8", NotATrustedProxy::Grammar),
            ("10.0.0.0/-8", NotATrustedProxy::Grammar),
            ("10.0.0.0/8/8", NotATrustedProxy::Grammar),
            ("10.0.0.0/255.0.0.0", NotATrustedProxy::Grammar),
            ("10.0.0.0/256", NotATrustedProxy::Grammar),
            ("10.0.0.0 /8", NotATrustedProxy::Grammar),
            ("[::1]", NotATrustedProxy::Grammar),
            ("10.0.0.0/33", NotATrustedProxy::PrefixLength),
            ("fd00::/129", NotATrustedProxy::PrefixLength),
            ("::ffff:10.0.0.0/129", NotATrustedProxy::PrefixLength),
            ("0.0.0.0/0", NotATrustedProxy::EveryAddress),
            ("10.0.0.0/0", NotATrustedProxy::EveryAddress),
            ("::/0", NotATrustedProxy::EveryAddress),
            ("::ffff:0:0/96", NotATrustedProxy::EveryAddress),
            ("::ffff:10.0.0.1/0", NotATrustedProxy::EveryAddress),
            ("10.0.0.1/8", NotATrustedProxy::BitsPastPrefix),
            ("fd00::1/8", NotATrustedProxy::BitsPastPrefix),
            ("::ffff:10.0.0.1/104", NotATrustedProxy::BitsPastPrefix),
            ("::ffff:0:0/95", NotATrustedProxy::BitsPastPrefix),
        ] {
            assert_eq!(entry.parse::<TrustedProxy>(), Err(kind), "{entry:?}");
        }
    }

    /// The error alone, what an app's own `?` prints, says what it refused and
    /// quotes nothing it was handed.
    #[test]
    fn a_refusal_read_on_its_own_says_what_it_refused() {
        for entry in ["hunter2_secret", "10.0.0.1/8", "0.0.0.0/0", "10.0.0.0/33"] {
            let said = entry.parse::<TrustedProxy>().expect_err(entry).to_string();
            assert!(said.starts_with("this trusted proxy entry is "), "{said}");
            assert!(!said.contains(entry), "{said}");
        }
    }

    /// What a boot's refusal ends with reads after `entry N of the list`, and
    /// quotes nothing it was handed.
    #[test]
    fn a_refusal_says_what_was_expected_and_quotes_no_value() {
        for entry in ["hunter2_secret", "10.0.0.1/8", "0.0.0.0/0", "10.0.0.0/33"] {
            let said = TrustedProxy::from_setting(entry).expect_err(entry);
            assert!(said.starts_with("is "), "{said}");
            assert!(!said.contains("hunter2_secret"), "{said}");
            assert!(!said.contains("10.0.0.1"), "{said}");
            assert!(!said.contains("10.0.0.0/33"), "{said}");
        }
        assert_eq!(
            TrustedProxy::from_setting("10.0.0.0/8"),
            Ok(proxy("10.0.0.0/8"))
        );
    }
}
