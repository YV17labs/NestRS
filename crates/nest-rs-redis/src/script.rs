//! [`RedisScript`] — a Lua script this crate runs, cached by Valkey under its
//! SHA1, and [`Invocation`], one call of it.
//!
//! **Every key of a call sits in one hash slot.** A Cluster refuses a script
//! whose keys sit on two nodes, so a call naming keys of two slots is refused on
//! every topology, before it is sent: a script that runs standalone runs on a
//! Cluster.

use redis::cluster_routing::Slot;
use redis::{Cmd, ErrorKind, ServerErrorKind, ToRedisArgs};

/// A Lua script, and the `SCRIPT LOAD` that caches it.
pub(crate) struct RedisScript {
    source: &'static str,
    hash: String,
}

impl RedisScript {
    pub(crate) fn new(source: &'static str) -> Self {
        Self {
            source,
            hash: redis::Script::new(source).get_hash().to_owned(),
        }
    }

    /// A call of the script with `key` as its first key.
    pub(crate) fn key(&self, key: impl ToRedisArgs) -> Invocation<'_> {
        let mut invocation = self.prepare();
        invocation.key(key);
        invocation
    }

    /// A call of the script, with no key or argument yet.
    pub(crate) fn prepare(&self) -> Invocation<'_> {
        Invocation {
            script: self,
            keys: Vec::new(),
            args: Vec::new(),
        }
    }

    /// The `SCRIPT LOAD` that caches the script on the server it reaches.
    pub(crate) fn load_cmd(&self) -> Cmd {
        let mut load = redis::cmd("SCRIPT");
        load.arg("LOAD").arg(self.source);
        load
    }
}

/// One call of a [`RedisScript`]: its keys, then its arguments.
pub(crate) struct Invocation<'a> {
    script: &'a RedisScript,
    keys: Vec<Vec<u8>>,
    args: Vec<Vec<u8>>,
}

impl<'a> Invocation<'a> {
    /// Add `key` to the call's `KEYS`.
    pub(crate) fn key(&mut self, key: impl ToRedisArgs) -> &mut Self {
        self.keys.extend(key.to_redis_args());
        self
    }

    /// Add `arg` to the call's `ARGV`.
    pub(crate) fn arg(&mut self, arg: impl ToRedisArgs) -> &mut Self {
        self.args.extend(arg.to_redis_args());
        self
    }

    /// The script called.
    pub(crate) fn script(&self) -> &'a RedisScript {
        self.script
    }

    /// The `EVALSHA` this call sends.
    pub(crate) fn eval_cmd(&self) -> Cmd {
        let mut eval = redis::cmd("EVALSHA");
        eval.arg(&self.script.hash)
            .arg(self.keys.len())
            .arg(&self.keys)
            .arg(&self.args);
        eval
    }

    /// The hash slot every key of the call sits in — `None` for a call naming
    /// no key — refusing a call whose keys sit in two.
    pub(crate) fn slot(&self) -> Result<Option<Slot>, redis::RedisError> {
        let mut slots = self.keys.iter().map(Slot::for_key);
        let Some(first) = slots.next() else {
            return Ok(None);
        };
        if slots.any(|slot| slot != first) {
            return Err(redis::RedisError::from((
                ErrorKind::Server(ServerErrorKind::CrossSlot),
                "a script's keys sit in more than one hash slot",
                "every key a script names is its queue's own, under its hash tag".to_owned(),
            )));
        }
        Ok(Some(first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ECHO: &str = "return {KEYS[1], ARGV[1]}";

    #[test]
    fn a_call_is_sent_by_the_hash_valkey_caches_it_under() {
        let script = RedisScript::new(ECHO);
        let mut call = script.key("nestrs:queue:{audio}:jobs");
        call.key("nestrs:queue:{audio}:dead").arg(7);
        let sent: Vec<Vec<u8>> = call
            .eval_cmd()
            .args_iter()
            .map(|arg| match arg {
                redis::Arg::Simple(bytes) => bytes.to_vec(),
                _ => panic!("an EVALSHA carries no cursor"),
            })
            .collect();
        assert_eq!(
            sent,
            [
                b"EVALSHA".to_vec(),
                redis::Script::new(ECHO).get_hash().as_bytes().to_vec(),
                b"2".to_vec(),
                b"nestrs:queue:{audio}:jobs".to_vec(),
                b"nestrs:queue:{audio}:dead".to_vec(),
                b"7".to_vec(),
            ]
        );
    }

    #[test]
    fn a_call_whose_keys_share_a_hash_tag_sits_in_one_slot() {
        let script = RedisScript::new(ECHO);
        let mut call = script.key("nestrs:queue:{audio}:jobs");
        call.key("nestrs:queue:{audio}:checkpoints");
        assert_eq!(call.slot().expect("one slot"), Some(Slot::for_key("audio")));
        assert_eq!(script.prepare().slot().expect("no key"), None);
    }

    #[test]
    fn a_call_naming_keys_of_two_slots_is_refused_before_it_is_sent() {
        let script = RedisScript::new(ECHO);
        let mut call = script.key("nestrs:queue:{audio}:jobs");
        call.key("nestrs:queue:{video}:jobs");
        let refused = call.slot().expect_err("two slots");
        assert_eq!(
            refused.kind(),
            ErrorKind::Server(ServerErrorKind::CrossSlot)
        );
    }
}
