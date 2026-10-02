use nest_rs_core::operation_log::Unit;

const JOB: Unit = nest_rs_core::unit!("queue.job", target: "nest_rs::redis", kind: Consumer);

fn main() {
    let _ = JOB;
}
