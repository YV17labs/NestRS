//! GraphQL's reading of `nest_rs_core::Ctx<T>`: a `#[query]`, `#[mutation]`,
//! `#[subscription]`, `#[entity]` or `#[field_resolver]` parameter reading what
//! a guard attached — the request's guards for every field it carries, the
//! field's own for its body — and no argument of the schema.
//!
//! ```
//! use nest_rs_core::{AmbientValue, Ctx, Layer, UnitContext, injectable};
//! use nest_rs_graphql::{operations, resolver};
//! use nest_rs_guards::{Denial, Guard, async_trait};
//! use nest_rs_http::HttpView;
//! # use nest_rs_core::module;
//! # use nest_rs_graphql::GraphqlModule;
//! # use nest_rs_guards::guard;
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
//!     async fn me(&self, caller: Ctx<Caller>) -> String {
//!         caller.into_inner().0
//!     }
//! }
//! # #[module(imports = [GraphqlModule::for_root(None)], providers = [NameGuard, MeResolver])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::builder()
//! #     .module::<AppModule>()
//! #     .use_guards_global([guard::<NameGuard>()])
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

use nest_rs_core::{AmbientValue, Ctx, Failure};

use crate::error::FieldFailure;

/// The `Ctx<T>` an operation takes, read in its tail once its guards passed; a
/// miss answers the opaque internal error, the type filed at `error`, never
/// answered.
pub fn ctx_parameter<T: AmbientValue + Clone>() -> Result<Ctx<T>, FieldFailure> {
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

        let Err(missed) = ctx_parameter::<Hidden>() else {
            panic!("no guard attached a value here");
        };
        let answered = missed.into_answer();

        assert_eq!(answered.message, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
        let said = serde_json::to_string(&answered.extensions).expect("extensions serialize");
        assert!(!said.contains("Hidden"), "{said}");
        let line = logs.expect_one(crate::TARGET, "graphql operation failed");
        assert!(
            line.field("error")
                .is_some_and(|error| error.contains("ctx::tests::Hidden")),
            "{:?}",
            line.fields,
        );
    }
}
