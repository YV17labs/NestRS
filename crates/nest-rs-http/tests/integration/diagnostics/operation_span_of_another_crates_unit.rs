fn main() {
    let correlation = nest_rs_core::Correlation::minted(None);
    let _span = nest_rs_core::operation_span!(nest_rs_http::unit::REQUEST, &correlation);
}
