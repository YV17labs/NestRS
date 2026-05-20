use tracing_subscriber::EnvFilter;

/// Install a default `tracing` subscriber, controllable via `RUST_LOG`.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
