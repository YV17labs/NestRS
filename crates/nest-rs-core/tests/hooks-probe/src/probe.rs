use nest_rs::core::{hooks, injectable};

/// A host whose one hook does nothing.
#[injectable]
#[derive(Default)]
pub struct HooksProbe;

#[hooks]
impl HooksProbe {
    #[on_module_init]
    async fn init(&self) {}
}
