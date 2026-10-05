use nest_rs::queue::{input, queue};

#[input]
#[derive(Clone, Debug)]
pub struct NoopCommand {
    pub seq: u32,
    pub pushed_us: u64,
    /// Ballast, `--pad` numbers long: what a job carrying a list of ids weighs.
    #[serde(default)]
    pub pad: Vec<u64>,
}

#[queue(name = "bench-c1", job = NoopCommand)]
pub struct C1Queue;

#[queue(name = "bench-c16", job = NoopCommand)]
pub struct C16Queue;

#[queue(name = "bench-r16", job = NoopCommand)]
pub struct R16Queue;
