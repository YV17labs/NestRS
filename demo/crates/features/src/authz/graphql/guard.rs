use nest_rs_core::injectable;

/// Marker provider owning the GraphQL principal forward — it carries no
/// logic of its own. Authentication and ability-building run in-band per
/// operation (`AppGraphqlGuard`); this marker's reachability is what gates
/// `forward_principal!(Claims, GraphqlAuthnGuard)`: omit `AuthzGraphqlModule`
/// and the principal is never seeded into resolver context.
#[injectable]
#[derive(Default)]
pub struct GraphqlAuthnGuard;
