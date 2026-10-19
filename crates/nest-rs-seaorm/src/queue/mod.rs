//! The queue port's outbox, kept in the app's Postgres (feature `queue`): the
//! store [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule) binds, and the
//! migration creating its table.

mod migration;
mod outbox;

pub use migration::OutboxMigration;
pub use outbox::SeaOrmOutbox;

#[cfg(test)]
pub(crate) use outbox::tests::armed;
pub(crate) use outbox::{
    KeepingPool, collect, insert, register, removal_of_job, removal_of_key, removed,
};
