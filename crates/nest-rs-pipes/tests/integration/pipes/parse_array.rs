//! Covers `src/pipes/parse_array.rs`.

use nest_rs_pipes::{ParseArray, Pipe};

/// A live secret, sent where a list item belongs.
const SECRET: &str = "sk_live_51HsecretTOKEN";

/// The first eight-character run of [`SECRET`] that `text` spells, so a refusal
/// quoting the item cut short or elided still counts as quoting it.
fn quoted_run(text: &str) -> Option<&'static str> {
    (0..=SECRET.len() - 8)
        .map(|start| &SECRET[start..start + 8])
        .find(|run| text.contains(run))
}

/// A refused item is never quoted back, wherever it sits in the list, and the
/// refusal still says what an item must be. Every edge renders the refusal
/// whole — HTTP's `400` detail, the WS frame and its `warn`, the queue's
/// dead-letter line and record — and it spelled the item, so whatever a client
/// put in the list reached all of them.
#[test]
fn a_refused_item_is_never_quoted_back_and_the_expected_type_is_named() {
    for input in [
        SECRET.to_owned(),
        format!("1,{SECRET},3"),
        format!("1, 2,  {SECRET} "),
    ] {
        let refusal = ParseArray::<u64>::transform(input.clone()).unwrap_err();
        // `Debug` spells everything the refusal carries: the message every edge
        // renders, and the details an edge forwards as `errors`.
        let carried = format!("{refusal:?}");
        assert_eq!(
            quoted_run(&carried),
            None,
            "`{input}` is quoted back: {carried}"
        );
        assert!(
            refusal.message().contains("u64"),
            "the refusal names what an item must be: {refusal}",
        );
    }
}
