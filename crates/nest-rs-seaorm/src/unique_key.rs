//! Read once, at the binding's boot, never on the transaction a violation
//! aborted: Postgres runs nothing more on it, and the edge renders the error
//! synchronously after the unit rolled back. Only an edge rendering a `#[crud]`
//! write reads it, so a binary with neither sends no catalog query.

#[cfg(any(feature = "http", feature = "graphql"))]
use std::collections::{BTreeSet, HashMap};
#[cfg(any(feature = "http", feature = "graphql"))]
use std::future::Future;
#[cfg(any(feature = "http", feature = "graphql"))]
use std::sync::Arc;
#[cfg(any(feature = "http", feature = "graphql"))]
use std::time::Duration;

#[cfg(any(feature = "http", feature = "graphql"))]
use nest_rs_core::{Unanswered, within};
#[cfg(any(feature = "http", feature = "graphql"))]
use sea_orm::sqlx;
#[cfg(any(feature = "http", feature = "graphql"))]
use sea_orm::{DatabaseConnection, DatabaseConnectionType, DbErr, RuntimeErr};

/// One `#[expose]` entity, submitted at link time so the binding reads its
/// table's unique keys at boot; emitted by the decorator, never by hand.
///
/// Fn pointers, since each value is a call and the entry must be a `static`.
pub struct UniqueKeyRegistration {
    /// The entity's table, as `EntityName::table_name` spells it.
    pub table: fn() -> &'static str,
    /// The schema the entity names, `None` for the search path's.
    pub schema: fn() -> Option<String>,
}

inventory::collect!(UniqueKeyRegistration);

/// The key columns of every unique index on the registered tables and their
/// partitions — a primary key, a unique constraint (whose index carries its name)
/// or a unique index, partial included — by the table an entity writes and the
/// table and name a unique violation reports. An index over an expression is left
/// out, and so is an `INCLUDE` column, which no key holds.
#[cfg(any(feature = "http", feature = "graphql"))]
#[derive(Debug, Default)]
pub(crate) struct UniqueKeys(HashMap<(String, String, String), Vec<String>>);

/// One row per key column, in the key's order: `$1` the registered tables, `$2`
/// their schemas, `''` for a table the search path finds. A violation reports a
/// partition and the partition's index, so `pg_inherits` is walked from each.
#[cfg(any(feature = "http", feature = "graphql"))]
const CATALOG: &str = r#"
WITH RECURSIVE registered AS (
  SELECT c.oid AS table_oid, c.relname AS root
  FROM unnest($1::text[], $2::text[]) AS r(tbl, nsp)
  JOIN pg_class c ON c.relname = r.tbl
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE CASE WHEN r.nsp = '' THEN pg_table_is_visible(c.oid) ELSE r.nsp = n.nspname END
  UNION
  SELECT h.inhrelid, registered.root
  FROM pg_inherits h
  JOIN registered ON h.inhparent = registered.table_oid
)
SELECT n.nspname::text AS "schema", registered.root::text AS "root", t.relname::text AS "table",
       i.relname::text AS "index", a.attname::text AS "column"
FROM registered
JOIN pg_index x ON x.indrelid = registered.table_oid
JOIN pg_class t ON t.oid = x.indrelid
JOIN pg_namespace n ON n.oid = t.relnamespace
JOIN pg_class i ON i.oid = x.indexrelid
CROSS JOIN LATERAL unnest(x.indkey::int2[]) WITH ORDINALITY AS k(attnum, position)
JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum
WHERE x.indisunique
  AND x.indexprs IS NULL
  AND k.position <= x.indnkeyatts
ORDER BY 1, 2, 3, 4, k.position
"#;

#[cfg(any(feature = "http", feature = "graphql"))]
impl UniqueKeys {
    /// The keys of every `#[expose]` entity's table, read from `db`'s catalog.
    /// The pool bounds the wait for a connection and Postgres the statement,
    /// which `statement_net` stands above, for a server that stopped answering:
    /// past them the read fails, and with it the boot.
    ///
    /// None on a connection that is no Postgres pool, when nothing registers, and
    /// on a pool holding no connection, said at `warn`: one opened lazily proved
    /// no server, and the boot does not wait on one here.
    pub(crate) async fn read(
        db: &DatabaseConnection,
        statement_net: Duration,
    ) -> Result<Self, DbErr> {
        let DatabaseConnectionType::SqlxPostgresPoolConnection(_) = db.inner else {
            return Ok(Self::default());
        };
        let (schemas, tables): (Vec<String>, Vec<String>) =
            inventory::iter::<UniqueKeyRegistration>
                .into_iter()
                .map(|entry| {
                    (
                        (entry.schema)().unwrap_or_default(),
                        (entry.table)().to_owned(),
                    )
                })
                .unzip();
        if tables.is_empty() {
            return Ok(Self::default());
        }
        let pool = db.get_postgres_connection_pool();
        if pool.size() == 0 {
            tracing::warn!(
                target: crate::target::ORM,
                reason = "no_open_connection",
                "unique keys not read: a unique conflict names no field",
            );
            return Ok(Self::default());
        }
        let net = pool.options().get_acquire_timeout() + statement_net;
        let rows = answered_within(net, async {
            sqlx::query_as::<_, (String, String, String, String, String)>(CATALOG)
                .bind(tables)
                .bind(schemas)
                .fetch_all(pool)
                .await
        })
        .await?;
        let keys = Self::from_columns(rows.into_iter().map(
            |(schema, root, table, index, column)| KeyColumn {
                schema,
                root,
                table,
                index,
                column,
            },
        ));
        tracing::debug!(
            target: crate::target::ORM,
            keys = keys.0.len(),
            "unique keys read",
        );
        Ok(keys)
    }

    /// The keys `columns` spell, each in the order its rows come. A table and
    /// index name two schemas share is left out: a violation reports no
    /// schema to tell them apart.
    fn from_columns(columns: impl IntoIterator<Item = KeyColumn>) -> Self {
        let mut keys: HashMap<(String, String, String), (String, Vec<String>)> = HashMap::new();
        let mut shared = BTreeSet::new();
        for KeyColumn {
            schema,
            root,
            table,
            index,
            column,
        } in columns
        {
            let key = (root, table, index);
            match keys.get_mut(&key) {
                Some((owner, held)) if *owner == schema => held.push(column),
                Some(_) => {
                    shared.insert(key);
                }
                None => {
                    keys.insert(key, (schema, vec![column]));
                }
            }
        }
        Self(
            keys.into_iter()
                .filter(|(key, _)| !shared.contains(key))
                .map(|(key, (_, held))| (key, held))
                .collect(),
        )
    }

    /// The key columns of the unique index `index` on `table`, in the key's
    /// order, `table` being `root` or one of its partitions; `None` for one this
    /// map does not know.
    pub(crate) fn columns(&self, root: &str, table: &str, index: &str) -> Option<&[String]> {
        self.0
            .get(&(root.to_owned(), table.to_owned(), index.to_owned()))
            .map(Vec::as_slice)
    }
}

/// `read`'s answer within `net`, or why there is none: the boot's error reads the
/// sentence, and an unwind is filed here once.
#[cfg(any(feature = "http", feature = "graphql"))]
async fn answered_within<T>(
    net: Duration,
    read: impl Future<Output = Result<T, sqlx::Error>>,
) -> Result<T, DbErr> {
    match within(net, read).await {
        Ok(answered) => answered.map_err(|err| DbErr::Query(RuntimeErr::SqlxError(Arc::new(err)))),
        Err(Unanswered::Panicked(payload)) => {
            nest_rs_core::contained_panic!(
                target: crate::target::ORM,
                payload.as_ref(),
                "the read of the unique keys panicked",
            );
            Err(DbErr::Custom("the read unwound".to_owned()))
        }
        Err(_) => Err(DbErr::Custom(format!("no answer within {net:?}"))),
    }
}

/// One key column the catalog read answers.
#[cfg(any(feature = "http", feature = "graphql"))]
struct KeyColumn {
    schema: String,
    root: String,
    table: String,
    index: String,
    column: String,
}

/// The keys the binding the running unit was opened by read at its boot;
/// `None` outside every unit, or under another driver's binding.
#[cfg(any(feature = "http", feature = "graphql"))]
pub(crate) fn current() -> Option<Arc<UniqueKeys>> {
    let bound = nest_rs_database::current_unit_of_work()?;
    let bound: &dyn std::any::Any = &*bound;
    bound
        .downcast_ref::<crate::SeaOrmUnitOfWork>()
        .map(crate::SeaOrmUnitOfWork::unique_keys)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn column(schema: &str, root: &str, table: &str, index: &str, name: &str) -> KeyColumn {
        KeyColumn {
            schema: schema.to_owned(),
            root: root.to_owned(),
            table: table.to_owned(),
            index: index.to_owned(),
            column: name.to_owned(),
        }
    }

    /// The keys a test names, each as `(table, index, columns)` in `public`.
    pub(crate) fn keys(of: &[(&str, &str, &[&str])]) -> UniqueKeys {
        UniqueKeys::from_columns(of.iter().flat_map(|(table, index, columns)| {
            columns
                .iter()
                .map(|name| column("public", table, table, index, name))
        }))
    }

    #[test]
    fn a_key_keeps_its_columns_in_the_order_the_catalog_reads_them() {
        let keys = keys(&[
            ("users", "users_email_key", &["email"]),
            ("users", "users_org_slug_live", &["org_id", "slug"]),
        ]);
        assert_eq!(
            keys.columns("users", "users", "users_email_key"),
            Some(&["email".to_owned()][..]),
        );
        assert_eq!(
            keys.columns("users", "users", "users_org_slug_live"),
            Some(&["org_id".to_owned(), "slug".to_owned()][..]),
        );
        assert_eq!(keys.columns("users", "users", "users_pkey"), None);
        assert_eq!(keys.columns("orgs", "orgs", "users_email_key"), None);
    }

    #[test]
    fn a_partitions_key_is_read_under_the_table_the_entity_writes() {
        let keys = UniqueKeys::from_columns([column(
            "public",
            "users",
            "users_0",
            "users_0_email_key",
            "email",
        )]);
        assert_eq!(
            keys.columns("users", "users_0", "users_0_email_key"),
            Some(&["email".to_owned()][..]),
        );
        assert_eq!(
            keys.columns("orgs", "users_0", "users_0_email_key"),
            None,
            "a hook's insert on another table's partition names nothing",
        );
    }

    #[test]
    fn a_table_and_index_two_schemas_share_name_nothing() {
        let (table, index) = ("users", "users_email_key");
        let keys = UniqueKeys::from_columns([
            column("public", table, table, index, "email"),
            column("archive", table, table, index, "login"),
        ]);
        assert_eq!(keys.columns(table, table, index), None);
    }

    inventory::submit! {
        UniqueKeyRegistration {
            table: || "unique_key_probes",
            schema: || None,
        }
    }

    /// A pool opened lazily holds no connection at boot: the read sends
    /// nothing, waits on no server, here one that does not exist, and says so.
    #[tokio::test]
    async fn a_pool_holding_no_open_connection_reads_no_key_and_waits_on_nothing() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut options = sea_orm::ConnectOptions::new("postgres://nestrs@127.0.0.1:1/nowhere");
        options
            .connect_lazy(true)
            .acquire_timeout(Duration::from_secs(600));
        let pool = sea_orm::Database::connect(options)
            .await
            .expect("a lazy pool dials nothing");

        let keys = UniqueKeys::read(&pool, Duration::from_secs(16))
            .await
            .expect("nothing is read, so nothing fails");
        assert!(keys.0.is_empty());
        let event = logs.expect_one(
            crate::target::ORM,
            "unique keys not read: a unique conflict names no field",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("reason").as_deref(), Some("no_open_connection"));
    }

    #[tokio::test]
    async fn a_connection_that_is_no_postgres_pool_reads_no_key() {
        let keys = UniqueKeys::read(&DatabaseConnection::default(), Duration::from_secs(16))
            .await
            .expect("a disconnected connection reads nothing");
        assert!(keys.0.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_unanswered_past_its_net_fails_saying_so() {
        let net = Duration::from_secs(26);
        let started = tokio::time::Instant::now();
        let refused = answered_within(net, std::future::pending::<Result<(), sqlx::Error>>())
            .await
            .expect_err("nothing answers");
        assert_eq!(started.elapsed(), net);
        assert!(
            refused.to_string().contains("no answer within 26s"),
            "{refused}"
        );
    }

    async fn unwinding() -> Result<(), sqlx::Error> {
        panic!("the driver unwound")
    }

    #[tokio::test]
    async fn a_read_that_unwinds_fails_and_is_filed_once() {
        let logs = nest_rs_testing::LogCapture::install();
        let refused = answered_within(Duration::from_secs(26), unwinding())
            .await
            .expect_err("the read unwound");
        assert!(
            refused.to_string().contains("the read unwound"),
            "{refused}"
        );
        let event = logs.expect_one(crate::target::ORM, "the read of the unique keys panicked");
        assert_eq!(event.level, "error");
    }

    #[test]
    fn outside_every_unit_no_key_is_current() {
        assert!(current().is_none());
    }
}
