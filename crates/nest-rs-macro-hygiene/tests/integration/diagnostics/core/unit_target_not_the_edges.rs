use nest_rs::core::operation_log::Unit;

const JOB: Unit = nest_rs::core::unit!("queue.job", target: "nest_rs::redis", kind: Consumer);

fn main() {
    let _ = JOB;
}
