fn main() {
    let correlation = nest_rs::core::Correlation::minted(None);
    let _span = nest_rs::core::operation_span!(nest_rs::http::unit::REQUEST, &correlation);
}
