use nest_rs_core::operation_log::Unit;

const JOB: Unit = nest_rs_core::unit!("Queue.Job", target: "nest_rs::queue", kind: Consumer);

fn main() {
    let _ = JOB;
}
