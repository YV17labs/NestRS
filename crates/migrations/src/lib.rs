mod m20260526_000000_create_org;
mod m20260526_000001_create_user;
mod m20260609_000000_create_post;
mod m20260610_000000_add_post_org_author;
mod migrator;

pub use migrator::{Migrator, migrate};
