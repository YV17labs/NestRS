use nest_rs_core::module;
use nest_rs_storage::StorageModule;

use super::service::AudioService;

// `AudioService` injects the shared `Storage` client, so the port owns the
// storage import: every audio adapter (HTTP, queue, schedule) pulls it
// transitively, and both the `api` and `worker` apps get object storage by
// importing an audio edge — no per-app wiring to keep in sync.
#[module(imports = [StorageModule], providers = [AudioService])]
pub struct AudioModule;
