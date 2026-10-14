//! [`GraphqlView`], the GraphQL edge's view of the field it dispatches.

use async_graphql::parser::types::OperationType;
use nest_rs_core::UnitView;

/// The GraphQL edge's view of one field a guard checks: which field, under
/// which kind of operation, in which role.
///
/// A guard reaches it through `UnitContext::switch_to` in
/// `Guard::check`. The field's arguments are not on it: refusing a payload is
/// a pipe's job.
#[derive(Clone, Copy, Debug)]
pub struct GraphqlView {
    field: &'static str,
    operation: OperationType,
    role: FieldRole,
}

impl UnitView for GraphqlView {}

impl GraphqlView {
    /// The view of `field`, declared at its site, run under `operation`.
    pub(crate) fn new(field: &'static str, operation: OperationType, role: FieldRole) -> Self {
        Self {
            field,
            operation,
            role,
        }
    }

    /// The field, as its site declares it (`listUsers`).
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// The kind of operation the field runs under.
    pub fn operation(&self) -> OperationType {
        self.operation
    }

    /// The role the field plays in its operation.
    pub fn role(&self) -> FieldRole {
        self.role
    }
}

/// The role a field plays in the operation that resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldRole {
    /// A `#[query]`, `#[mutation]` or `#[subscription]`: a field of the root
    /// type the operation selects.
    RootField,
    /// An `#[entity]`, resolved from a reference a federation router sent.
    Entity,
    /// A federation root field, `_service` or `_entities`, which no resolver
    /// declares.
    Federation,
    /// A `#[field_resolver]`, resolved under a root field's result once per
    /// parent: a field of a unit, never a unit of its own.
    Nested,
}
