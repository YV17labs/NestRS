//! Covers `src/commands.rs` against the live server: once something records
//! client operations, every command and every pipeline a connection sends is
//! handed over by its name, on its database — never by a key it names.

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

    let recorded = RECORDED.lock().expect("the recorded operations").clone();
    assert_eq!(
        recorded,
        vec![
            ("GET".to_owned(), Some(db.clone()), None),
            ("BATCH GET".to_owned(), Some(db.clone()), None),
            ("NOSUCHCOMMAND".to_owned(), Some(db), Some("ERR".to_owned())),
        ],
    );
    assert!(!format!("{recorded:?}").contains(&key), "{recorded:?}");
}
