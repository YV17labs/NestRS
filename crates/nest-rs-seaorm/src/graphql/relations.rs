//! Cross-entity bridges that let one `#[expose]` macro emit a field resolver
//! pointing at another entity's loader **without knowing its service name**.

use nest_rs_graphql::async_graphql::OutputType;
use nest_rs_graphql::async_graphql::connection::{Connection, Edge};

/// The dataloader key for **one parent's page** of an auto-resolved `has_many`.
///
/// The window travels in the key: keyed by parent alone, two sibling selections
/// asking different windows of one relation would both get the first one's page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RelationKey<K> {
    /// The parent row's primary key — the child's foreign-key value.
    pub parent: K,
    /// Page size, already clamped by [`clamp_page_size`](crate::clamp_page_size).
    pub first: u64,
    /// Exclusive cursor: the last child key of the previous page.
    pub after: Option<uuid::Uuid>,
}

/// One parent's page of children, as the loader hands it back: each child
/// beside the cursor addressing it, plus whether a further page exists.
///
/// Not a `Connection`: a `Loader::Value` must be `Clone` and a `Connection` is not.
#[derive(Clone, Debug)]
pub struct RelationPage<T> {
    /// `(cursor, node)` in primary-key order.
    pub edges: Vec<(String, T)>,
    /// Whether a further page exists — an over-fetched row, not a count.
    pub has_next_page: bool,
}

impl<T> Default for RelationPage<T> {
    /// The empty page a parent with no children resolves to; hand-written
    /// because `T` need not be `Default`.
    fn default() -> Self {
        Self {
            edges: Vec::new(),
            has_next_page: false,
        }
    }
}

impl<T: OutputType> RelationPage<T> {
    /// The Relay connection this page ships as; `has_previous_page` is the
    /// caller's `after.is_some()`.
    pub fn into_connection(self, has_previous_page: bool) -> Connection<String, T> {
        let mut connection = Connection::new(has_previous_page, self.has_next_page);
        connection.edges.extend(
            self.edges
                .into_iter()
                .map(|(cursor, node)| Edge::new(cursor, node)),
        );
        connection
    }
}

/// "This entity exposes a primary-key loader and a wire DTO." Implemented
/// automatically by `#[expose]` on every entity that declares a `service = …`.
pub trait PkLoadable {
    /// The `#[dataloader]`-generated primary-key loader (`<Service>ById`).
    type Loader: Send + Sync + 'static;
    /// The GraphQL output type `#[expose(name = "…")]` emits for this entity.
    type Wire: OutputType + Send + Sync + 'static;
}

/// The `Via` a `HasMany` takes when it names no column: "the one foreign key
/// this child has to that parent".
///
/// A child with **two** `belongs_to` at one parent gets no impl for it, so the
/// parent must name the key with `#[expose(via = "…")]`.
pub struct SoleForeignKey;

/// "This entity is the child of `Parent`, fetched in batches by its
/// foreign-key column." Implemented by `#[expose]` on the entity owning the FK.
///
/// `Via` names *which* foreign key: one `By<PascalColumn>` marker per
/// `belongs_to`, plus [`SoleForeignKey`] when the child points at that parent
/// exactly once.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not reachable as a `has_many` child of `{Parent}` through `{Via}`",
    label = "no foreign key selected",
    note = "an auto-resolved `has_many` reaches its children through the child's own `#[sea_orm(belongs_to, from = \"…\")]`",
    note = "two `belongs_to` columns pointing at one parent leave no default — name the one this relation follows with `#[expose(via = \"<fk_column>\")]` on the `HasMany` field"
)]
pub trait RelatedTo<Parent: ?Sized, Via: ?Sized = SoleForeignKey> {
    /// The FK loader that batches this child by its foreign-key column.
    type Loader: Send + Sync + 'static;
    /// The GraphQL output type for this child entity.
    type Wire: OutputType + Send + Sync + 'static;
}
