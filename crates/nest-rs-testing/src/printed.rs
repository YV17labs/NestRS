use std::error::Error;

/// Everything `error` prints, however it is read: its
/// [`error_message`](nest_rs_core::error_message), then each link of its chain
/// by its `Display` and its `Debug`, which covers what a `main` returning it
/// writes. What a test asserts a failure never quotes — a value, a secret.
///
/// ```
/// let refused = std::io::Error::other("the store refused the call");
/// let printed = nest_rs_testing::printed(&refused);
/// assert!(printed.contains("refused the call"));
/// assert!(!printed.contains("ada@example.com"));
/// ```
pub fn printed(error: &(dyn Error + 'static)) -> String {
    let mut all = nest_rs_core::error_message(error);
    let mut link = Some(error);
    while let Some(current) = link {
        all.push_str(&format!("\n{current}\n{current:?}"));
        link = current.source();
    }
    all
}
