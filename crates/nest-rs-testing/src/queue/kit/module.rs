//! [`QueueKitModule`] — the module a case's app imports beside the backend's,
//! so the kit's processor is reachable and its methods run.

use nest_rs_core::module;

use super::processor::QueueKitProcessor;

/// The kit's processor, reachable.
#[module(providers = [QueueKitProcessor])]
pub(crate) struct QueueKitModule;
