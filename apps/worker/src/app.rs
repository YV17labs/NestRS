use nestrs_config::env_var;
use nestrs_core::module;
use nestrs_queue::{QueueModule, QueueOptions};

use crate::audio::AudioModule;

#[module(imports = [
    QueueModule::for_root(QueueOptions {
        url: env_var("REDIS_URL").unwrap_or_else(|| "redis://127.0.0.1/".into()),
    }),
    AudioModule,
])]
pub struct AppModule;
