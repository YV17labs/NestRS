//! `RedisThrottlerModule`'s composition, in process: registered by an importer
//! that never collects it, the store it would bind is named rather than absent.

use std::sync::Arc;

use nest_rs_redis::RedisThrottlerModule;
use nest_rs_throttler::ThrottlerStore;

#[tokio::test]
async fn the_binding_registered_without_its_collect_fails_the_boot_naming_its_store() {
    assert_eq!(
        crate::registered_alone::<RedisThrottlerModule>().await,
        std::any::type_name::<Arc<dyn ThrottlerStore>>()
    );
}
