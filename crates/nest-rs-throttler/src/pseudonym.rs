//! [`PseudonymStore`] — what a store outside this process counts in: every
//! bucket's subject replaced by its HMAC-SHA256 under the deployment's
//! [`pseudonym_key`](crate::ThrottlerConfig::pseudonym_key), so the client's
//! address or identity never reaches it.

use std::sync::Arc;

use async_trait::async_trait;
use aws_lc_rs::hmac;

use crate::store::{Decision, ThrottlerStore};
use crate::throttle::Throttle;

/// The fewest bytes a pseudonym key holds: HMAC-SHA256's output, below which
/// the key rather than the hash bounds its strength (RFC 2104 §3).
pub(crate) const MIN_KEY_BYTES: usize = 32;

/// How many bytes of the tag a pseudonym keeps: 128 bits, which no two of a
/// store's subjects share by chance.
const PSEUDONYM_BYTES: usize = 16;

/// A [`ThrottlerStore`] whose counters leave this process, handed pseudonyms.
pub(crate) struct PseudonymStore {
    inner: Arc<dyn ThrottlerStore>,
    key: hmac::Key,
}

impl PseudonymStore {
    /// The store a guard counts in: `store` itself when its counters never
    /// leave this process, otherwise `store` behind `key`, which it cannot run
    /// without.
    pub(crate) fn wrap(
        store: Arc<dyn ThrottlerStore>,
        key: Option<&str>,
    ) -> anyhow::Result<Arc<dyn ThrottlerStore>> {
        if store.in_process() {
            return Ok(store);
        }
        let Some(key) = key else {
            anyhow::bail!(
                "`{}` keeps the rate limiter's counters outside this process, so it counts each \
                 bucket under an HMAC of its subject — never the client's address or identity — \
                 keyed alike on every replica: set {} to the same 32 bytes or more on each \
                 (`openssl rand -base64 32`), or `ThrottlerConfig::pseudonym_key` in code",
                store.name(),
                nest_rs_config::spellings("throttler", "PSEUDONYM_KEY"),
            );
        };
        Ok(Arc::new(Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes()),
            inner: store,
        }))
    }

    /// `subject`'s pseudonym: the leading [`PSEUDONYM_BYTES`] of its HMAC, in
    /// lowercase hex.
    fn pseudonym(&self, subject: &str) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let tag = hmac::sign(&self.key, subject.as_bytes());
        let mut pseudonym = String::with_capacity(PSEUDONYM_BYTES * 2);
        for byte in &tag.as_ref()[..PSEUDONYM_BYTES] {
            pseudonym.push(char::from(HEX[usize::from(byte >> 4)]));
            pseudonym.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        pseudonym
    }
}

#[async_trait]
impl ThrottlerStore for PseudonymStore {
    async fn hit(&self, key: &str, limit: Throttle) -> Decision {
        self.inner.hit(&self.pseudonym(key), limit).await
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryThrottler;

    const KEY: &str = "a pseudonym key of at least 32 bytes, for tests";

    fn keyed(key: &str) -> PseudonymStore {
        PseudonymStore {
            inner: Arc::new(InMemoryThrottler::new()),
            key: hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes()),
        }
    }

    #[test]
    fn a_pseudonym_is_the_subject_s_under_one_key_and_another_under_another() {
        let subject = "http\u{1f}/login\u{1f}203.0.113.7";
        let one = keyed(KEY).pseudonym(subject);
        assert_eq!(one, keyed(KEY).pseudonym(subject));
        assert_ne!(
            one,
            keyed(KEY).pseudonym("http\u{1f}/login\u{1f}203.0.113.8")
        );
        assert_ne!(one, keyed(&format!("{KEY}!")).pseudonym(subject));
        assert_eq!(one.len(), PSEUDONYM_BYTES * 2);
        assert!(
            one.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn an_in_process_store_is_counted_in_as_it_is() {
        let store: Arc<dyn ThrottlerStore> = Arc::new(InMemoryThrottler::new());
        let wrapped = PseudonymStore::wrap(Arc::clone(&store), None).expect("needs no key");
        assert!(Arc::ptr_eq(&store, &wrapped));
    }
}
