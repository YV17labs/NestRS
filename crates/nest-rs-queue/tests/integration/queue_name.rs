//! The rule a queue name, a dynamic queue's prefix and its key share — the
//! port's copy, pinned against the one `#[queue]` checks its literals with,
//! since the macro crate cannot depend on this one.

use nest_rs_queue::QueueName;

#[test]
fn the_decorators_copy_of_the_rule_agrees_with_the_port() {
    let longest = "a".repeat(QueueName::MAX_LEN);
    let too_long = "a".repeat(QueueName::MAX_LEN + 1);
    let corpus = [
        "audio",
        "audio.preview",
        "audio-preview_2",
        "A1",
        longest.as_str(),
        "",
        too_long.as_str(),
        "nestrs:queue",
        "tenant#acme",
        "with space",
        "tab\tname",
        "é",
        "slash/name",
        "*",
    ];
    for value in corpus {
        assert_eq!(
            nest_rs_codegen::is_valid_queue_name(value),
            QueueName::is_valid(value),
            "the two copies disagree on {value:?}",
        );
    }
}

/// The two copies refuse in one sentence: the decorator's is the port's with the
/// site in front, so a limit or a reason changed on one side fails here.
#[test]
fn the_decorators_refusal_is_the_ports_with_the_site_in_front() {
    for value in ["billing:invoices", "tenant#acme", ""] {
        let port = QueueName::new(value.to_owned())
            .expect_err("outside the rule")
            .to_string();
        assert_eq!(
            nest_rs_codegen::invalid_queue_name("queue", "name", "queue name", value),
            format!("#[queue] `name`: {port}"),
        );
    }
    let too_long = "a".repeat(QueueName::MAX_LEN + 1);
    let port = QueueName::new(too_long.clone())
        .expect_err("too long")
        .to_string();
    assert!(
        port.contains(&format!("1 to {}", QueueName::MAX_LEN)),
        "{port}"
    );
}
