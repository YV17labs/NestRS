//! Covers `src/commands.rs` against the live server: once something records
//! client operations, every command and every pipeline a connection sends is
//! handed over by its name, on its database — never by a key it names — and a
//! transaction by the commands it runs, failed by a refusal inside its `EXEC`.

use std::sync::Mutex;

use nest_rs_core::client_operation::{ClientOperation, ClientValue};

use crate::{connect, unique_key};

/// Each operation the seeded recorder was handed: its name, the database it
/// ran on, and its class if it failed.
type Recorded = (String, Option<String>, Option<String>);

static RECORDED: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

fn capture(operation: &ClientOperation<'_>) {
    let namespace = operation
        .attributes()
        .iter()
        .find_map(|(key, value)| match (*key, value) {
            ("db.namespace", ClientValue::Text(namespace)) => Some((*namespace).to_owned()),
            _ => None,
        });
    RECORDED.lock().expect("the recorded operations").push((
        operation.name().to_owned(),
        namespace,
        operation.error_type().map(str::to_owned),
    ));
}

#[tokio::test]
async fn every_command_and_pipeline_is_recorded_by_its_name_on_its_database() {
    nest_rs_core::__private::set_client_recorder(capture);
    let mut conn = connect().await;
    let key = unique_key("commands");
    let db = redis::aio::ConnectionLike::get_db(&conn).to_string();

    let _: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .expect("a GET answers");
    let _: (Option<String>, Option<String>) = redis::pipe()
        .cmd("GET")
        .arg(&key)
        .cmd("GET")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .expect("a pipeline answers");
    redis::cmd("NOSUCHCOMMAND")
        .query_async::<()>(&mut conn)
        .await
        .expect_err("the server refuses a command it does not know");
    // The key expires on its own, however the test ends.
    redis::pipe()
        .atomic()
        .cmd("SET")
        .arg(&key)
        .arg("text")
        .arg("EX")
        .arg(60)
        .cmd("LPUSH")
        .arg(&key)
        .arg(1)
        .query_async::<()>(&mut conn)
        .await
        .expect_err("EXEC answers the list push on a string with a refusal");

    let recorded = RECORDED.lock().expect("the recorded operations").clone();
    assert_eq!(
        recorded,
        vec![
            ("GET".to_owned(), Some(db.clone()), None),
            ("BATCH GET".to_owned(), Some(db.clone()), None),
            (
                "NOSUCHCOMMAND".to_owned(),
                Some(db.clone()),
                Some("ERR".to_owned())
            ),
            ("BATCH".to_owned(), Some(db), Some("WRONGTYPE".to_owned())),
        ],
    );
    assert!(!format!("{recorded:?}").contains(&key), "{recorded:?}");
}

/// A command its caller gave up on — a timeout around it, a unit cut at its
/// deadline — is recorded all the same, with no failure: the slowest commands
/// are the ones a histogram must not miss.
#[tokio::test]
async fn a_command_its_caller_gives_up_on_is_recorded_with_no_failure() {
    nest_rs_core::__private::set_client_recorder(capture);
    let mut conn = connect().await;
    let key = unique_key("abandoned");
    let db = redis::aio::ConnectionLike::get_db(&conn).to_string();

    // The server waits a second on a list nothing pushes to; the caller, a tenth.
    let waited = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        redis::cmd("BLPOP")
            .arg(&key)
            .arg(1)
            .query_async::<Option<(String, String)>>(&mut conn),
    )
    .await;

    assert!(waited.is_err(), "the caller gave up first");
    assert_eq!(
        RECORDED.lock().expect("the recorded operations").clone(),
        vec![("BLPOP".to_owned(), Some(db), None)],
    );
}
