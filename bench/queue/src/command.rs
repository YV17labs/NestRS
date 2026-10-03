use nest_rs::queue::{input, queue};

#[input]
#[derive(Clone, Debug)]
pub struct NoopCommand {
    pub seq: u32,
    pub pushed_us: u64,
}

#[queue(name = "bench-c1", job = NoopCommand)]
pub struct C1Queue;

#[queue(name = "bench-c16", job = NoopCommand)]
pub struct C16Queue;
