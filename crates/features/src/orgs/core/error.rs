use sea_orm::DbErr;

/// `Display` is wire-safe; the inner `DbErr` stays for logs (same discipline
/// as [`crate::users::UserError`]).
#[derive(Debug, Clone, thiserror::Error)]
pub enum OrgError {
    #[error("database error")]
    Db(#[from] DbErr),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_display_is_wire_safe_constant() {
        let err = OrgError::Db(DbErr::Custom("SELECT * FROM org WHERE id = $1".into()));
        assert_eq!(err.to_string(), "database error");
    }
}
