use nest_rs::core::operation_log::Unit;

const JOB: Unit = nest_rs::core::unit!("Queue.Job", target: "nest_rs::queue", kind: Consumer);

fn main() {
    let _ = JOB;
}
