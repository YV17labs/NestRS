//! The trusted-proxy entry grammar `trusted_proxy!` reads its literal with; a
//! copy of `nest_rs_http::TrustedProxy`'s `FromStr`, pinned against it over one
//! corpus by a test in `nest-rs-http`.

use std::net::IpAddr;

/// The address and the prefix length `text` spells, as `TrustedProxy`'s
/// `FromStr` reads them; `None` for text that is neither form.
pub fn trusted_proxy_entry(text: &str) -> Option<(IpAddr, Option<u8>)> {
    let Some((network, prefix)) = text.split_once('/') else {
        return Some((text.parse().ok()?, None));
    };
    // `u8`'s parser takes a leading `+`, which no CIDR notation writes.
    let prefix = Some(prefix)
        .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))?
        .parse()
        .ok()?;
    Some((network.parse().ok()?, Some(prefix)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_an_address_and_its_prefix_length() {
        let v4: IpAddr = "10.42.0.0".parse().unwrap();
        assert_eq!(trusted_proxy_entry("10.42.0.0/16"), Some((v4, Some(16))));
        assert_eq!(trusted_proxy_entry("10.42.0.0"), Some((v4, None)));
        assert_eq!(
            trusted_proxy_entry("fd00::/8"),
            Some(("fd00::".parse().unwrap(), Some(8)))
        );
        assert_eq!(trusted_proxy_entry("10.0.0.0/+8"), None);
    }
}
