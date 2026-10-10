//! The field half of `#[expose]` refuses a repeated key as the struct half does
//! — within one attribute and across two on the same field. `via` decides which
//! of the child's foreign keys a `HasMany` follows, so `via = "author_id", via =
//! "reviewer_id"` compiled and silently loaded the rows `reviewer_id` links; a
//! repeated `complexity` silently took the last query-cost limit.

mod via_twice {
    use nest_rs::seaorm::expose;

    #[expose(name = "Thing")]
    pub struct Model {
        #[expose(via = "author_id", via = "reviewer_id")]
        pub name: String,
    }
}

mod complexity_across_two_attributes {
    use nest_rs::seaorm::expose;

    #[expose(name = "Thing")]
    pub struct Model {
        #[expose(complexity = 1)]
        #[expose(complexity = 99)]
        pub name: String,
    }
}

fn main() {}
