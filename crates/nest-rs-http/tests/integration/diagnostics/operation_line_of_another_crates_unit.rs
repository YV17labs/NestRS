fn main() {
    nest_rs_core::operation_line!(
        nest_rs_http::unit::REQUEST,
        span: &tracing::Span::none(),
        outcome: nest_rs_core::operation_log::OK,
        started: std::time::Instant::now(),
    );
}
