//! GraphQL's reading of `nest_rs_core::Ambient<T>`: a `#[query]`, `#[mutation]`,
//! `#[subscription]`, `#[entity]` or `#[field_resolver]` parameter reading what
//! a guard attached — the request's guards for every field it carries, the
//! field's own for its body — and no argument of the schema.
//!
//! ```
//! use nest_rs_core::{Ambient, AmbientValue, Layer, UnitContext, async_trait, injectable};
//! use nest_rs_graphql::{operations, resolver};
//! use nest_rs_guards::{Denial, Guard};
//! use nest_rs_http::HttpView;
//! # use nest_rs_core::module;
//! # use nest_rs_graphql::GraphqlModule;
//! # use nest_rs_guards::GlobalGuards;
//!
//! #[derive(Clone)]
//! struct Caller(String);
//!
//! impl AmbientValue for Caller {}
//!
//! /// Attaches the caller the way an authentication guard does.
//! #[injectable]
//! #[derive(Layer)]
//! struct NameGuard;
//!
//! #[async_trait]
//! impl Guard for NameGuard {
//!     async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
//!         let name = HttpView::of(cx)?.request().header("x-name").unwrap_or("anonymous");
//!         let caller = Caller(name.to_owned());
//!         cx.attach(caller);
//!         Ok(())
//!     }
//!
//!     async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
//!         Ok(())
//!     }
//! }
//!
//! #[resolver]
//! struct MeResolver;
//!
//! #[operations]
//! impl MeResolver {
//!     #[query]
//!     #[public]
//!     async fn me(&self, caller: Ambient<Caller>) -> String {
//!         caller.into_inner().0
//!     }
//! }
//! # #[module(
//! #     imports = [GraphqlModule],
//! #     providers = [MeResolver, GlobalGuards<(NameGuard,)>],
//! # )]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::builder()
//! #     .module::<AppModule>()
//! #     .build()
//! #     .await?;
//!
//! let resp = app
//!     .http()
//!     .post("/graphql")
//!     .header("x-name", "ada")
//!     .body_json(&serde_json::json!({ "query": "{ me }" }))
//!     .send()
//!     .await;
//! resp.assert_json(serde_json::json!({ "data": { "me": "ada" } })).await;
//! # Ok(())
//! # }
//! ```

use nest_rs_core::{Ambient, AmbientValue, Failure};

use crate::error::FieldFailure;

/// The `Ambient<T>` an operation takes, read in its tail once its guards passed; a
/// miss answers the opaque internal error, the type filed at `error`, never
/// answered.
pub fn ambient_parameter<T: AmbientValue + Clone>() -> Result<Ambient<T>, FieldFailure> {
    nest_rs_core::__private::attached::<T>()
        .map_err(|missing| FieldFailure::unanswered(Failure::from(missing)))
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    #[derive(Clone)]
    struct Hidden;

    impl AmbientValue for Hidden {}

    #[test]
    fn a_miss_answers_no_type_name_and_files_it() {
        let logs = LogCapture::install();

        let Err(missed) = ambient_parameter::<Hidden>() else {
            panic!("no guard attached a value here");
        };
        let answered = missed.into_answer();

        assert_eq!(answered.message, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
        let said = serde_json::to_string(&answered.extensions).expect("extensions serialize");
        assert!(!said.contains("Hidden"), "{said}");
        let line = logs.expect_one(crate::TARGET, "graphql operation failed");
        assert!(
            line.field("error")
                .is_some_and(|error| error.contains("no `Hidden` is ambient")),
            "{:?}",
            line.fields,
        );
    }
}
