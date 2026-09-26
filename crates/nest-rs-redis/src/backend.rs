//! [`BACKEND`] — what the Redis backend declares to the queue port: its
//! `messaging.system` and the optional capabilities it honours.

use nest_rs_queue::{Capabilities, QueueBackend};

/// The Redis backend, as the queue port reads it: `redis` is its
/// `messaging.system`, and it declares **no** optional capability.
///
/// Not one, because not one is honoured yet: a delayed push, a unique key, a
/// cancel, a throttle, a checkpoint and a dynamic queue each need state this
/// crate would keep in keys of its own beside the queue storage, and none of
/// those keys exists. Declaring a capability the worker does not keep is the
/// silent failure the port's refusals exist to prevent — the port refuses each
/// of them here, at the push or at the boot, naming this backend. What every
/// backend owes regardless — the retry budget the port counts, one transaction
/// per attempt, a method's `concurrency` — is honoured.
///
/// A capability joins this list with the keys that keep it and the e2e that
/// proves it, one by one — never as `Capabilities::ALL`, so a capability the port
/// adds later is not claimed before this crate honours it.
pub(crate) static BACKEND: QueueBackend = QueueBackend::new("redis", Capabilities::NONE);
