use nest_rs_core::operation_log::Unit;

const CALL: Unit = nest_rs_core::unit!("grpc.call", target: "nest_rs::grpc", kind: Server);

fn main() {
    let _ = CALL;
}
