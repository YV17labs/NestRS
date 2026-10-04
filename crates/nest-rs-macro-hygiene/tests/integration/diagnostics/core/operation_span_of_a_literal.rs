fn main() {
    let correlation = nest_rs::core::Correlation::minted(None);
    let _span = nest_rs::core::operation_span!("http.request", &correlation);
}
