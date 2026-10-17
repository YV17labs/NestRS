fn main() {
    nest_rs::core::operation_line!(
        nest_rs::http::unit::REQUEST,
        span: &tracing::Span::none(),
        outcome: nest_rs::core::operation_log::OK,
        started: std::time::Instant::now(),
    );
}
