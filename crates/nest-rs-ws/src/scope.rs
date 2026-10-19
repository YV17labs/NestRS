//! Request-scoped providers on WS messages — the WS mirror of
//! [`nest_rs_http::Scoped<T>`]: a `#[subscribe_message]` parameter resolving an
//! `#[injectable(scope = request)]` provider from the scope each message runs
//! in.
//!
//! ```
//! # use std::sync::atomic::{AtomicU64, Ordering};
//! # use std::sync::Arc;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_ws::{Scoped, WsModule, gateway, messages};
//! #
//! # #[injectable]
//! # #[derive(Default)]
//! # struct Counter {
//! #     next: AtomicU64,
//! # }
//! #
//! # #[injectable(scope = request)]
//! # struct RequestSeq {
//! #     #[inject]
//! #     counter: Arc<Counter>,
//! # }
//! #
//! # impl RequestSeq {
//! #     fn value(&self) -> u64 {
//! #         self.counter.next.fetch_add(1, Ordering::SeqCst)
//! #     }
//! # }
//! #
//! # #[gateway(path = "/ws")]
//! # #[derive(Default)]
//! # struct SeqGateway;
//! #
//! # #[messages]
//! # impl SeqGateway {
//! #[subscribe_message("whoami")]
//! #[public]
//! async fn whoami(&self, seq: Scoped<RequestSeq>) -> u64 {
//!     seq.value()
//! }
//! # }
//! #
//! # #[module(imports = [WsModule], providers = [Counter, RequestSeq, SeqGateway])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::builder().module::<AppModule>().build_ws().await?;
//! # let mut socket = app.socket("/ws").connect().await;
//! # socket.send("whoami", serde_json::Value::Null).await;
//!
//! assert_eq!(socket.next_envelope().await["data"], 0);
//! # app.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! Scope is **per message**, so an `#[injectable(scope = request)]` provider is
//! rebuilt for each message and shared within that one dispatch; with none in
//! the app, every message shares one scope, which has nothing to build.

use std::any::{TypeId, type_name};
use std::ops::Deref;
use std::sync::Arc;

use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, ResolvesProvider};

use crate::envelope::{WsError, WsReply};

/// Resolves a provider of type `T` from the current WS message's
/// [`RequestScope`](nest_rs_core::RequestScope), declared as a
/// `#[subscribe_message]` parameter — the per-message mirror of
/// [`nest_rs_http::Scoped<T>`].
///
/// The provider is part of the boot's access contract
/// ([`ResolvesProvider`]). Should it still be missing at a message, the
/// message answers the opaque error frame and the type is filed at `error`,
/// never answered.
///
/// ```
/// use std::any::TypeId;
///
/// use nest_rs_core::ResolvesProvider;
/// use nest_rs_ws::Scoped;
///
/// struct Cart;
///
/// assert_eq!(Scoped::<Cart>::provider().0, TypeId::of::<Cart>());
/// ```
pub struct Scoped<T>(pub Arc<T>);

impl<T> Scoped<T> {
    /// Take the resolved provider handle out of the wrapper.
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T> Deref for Scoped<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: 'static> ResolvesProvider for Scoped<T> {
    fn provider() -> (TypeId, &'static str) {
        (TypeId::of::<T>(), type_name::<T>())
    }
}

/// `T` from the running message's scope, for the `Scoped<T>` parameter
/// `#[messages]` reads; a miss answers the opaque frame, the type filed at
/// `error`.
pub fn scoped_parameter<T: Send + Sync + 'static>() -> Result<Scoped<T>, WsReply> {
    let Some(scope) = nest_rs_core::current_request_scope() else {
        tracing::error!(
            target: crate::TARGET,
            provider = type_name::<T>(),
            "request scope not installed — a message's handler ran off its dispatch",
        );
        return Err(opaque());
    };
    match scope.get::<T>() {
        Some(provider) => Ok(Scoped(provider)),
        None => {
            tracing::error!(
                target: crate::TARGET,
                provider = type_name::<T>(),
                "no provider for a request-scoped parameter — add it to a module's providers",
            );
            Err(opaque())
        }
    }
}

/// What a miss answers: the client learns nothing of the wiring.
fn opaque() -> WsReply {
    WsReply::Error(WsError::new(OPAQUE_CLIENT_MESSAGE))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use nest_rs_core::__private::{ParamProbe, ParamProbeFallback as _, ParamProbeResolves as _};
    use nest_rs_core::{Container, Correlation, RequestScope};
    use nest_rs_testing::LogCapture;

    use super::*;

    struct Probe(u64);

    fn scoped_container() -> Container {
        let counter = Arc::new(AtomicU64::new(0));
        Container::builder()
            .provide_scoped::<Probe, _>(move |_| Probe(counter.fetch_add(1, Ordering::SeqCst)))
            .build()
            .expect("the container builds")
    }

    fn resolved() -> u64 {
        match scoped_parameter::<Probe>() {
            Ok(probe) => probe.0.0,
            Err(_) => panic!("the scope resolves the probe"),
        }
    }

    #[tokio::test]
    async fn one_scope_shares_one_instance_within_a_message() {
        let scope = Arc::new(RequestScope::new(scoped_container()));
        nest_rs_core::with_request_scope(Some(scope), Correlation::minted(None), async {
            assert_eq!(resolved(), resolved());
        })
        .await;
    }

    #[tokio::test]
    async fn separate_messages_build_distinct_instances() {
        let container = scoped_container();
        let first = nest_rs_core::with_request_scope(
            Some(Arc::new(RequestScope::new(container.clone()))),
            Correlation::minted(None),
            async { resolved() },
        )
        .await;
        let second = nest_rs_core::with_request_scope(
            Some(Arc::new(RequestScope::new(container))),
            Correlation::minted(None),
            async { resolved() },
        )
        .await;
        assert_ne!(
            first, second,
            "each WS message builds its own request-scoped instance",
        );
    }

    /// Stands for a provider a reachable module does not offer.
    struct Hidden;

    /// A miss — no scope, or no provider — tells the client nothing of the
    /// wiring and files the type for the operator.
    #[tokio::test]
    async fn a_miss_answers_opaquely_and_files_the_type() {
        for scope in [
            None,
            Some(Arc::new(RequestScope::new(Container::default()))),
        ] {
            let logs = LogCapture::install();
            let answered =
                nest_rs_core::with_request_scope(scope, Correlation::minted(None), async {
                    scoped_parameter::<Hidden>()
                })
                .await;
            let Err(WsReply::Error(frame)) = answered else {
                panic!("a miss answers an error frame");
            };
            let said = serde_json::to_string(&frame).expect("serializes");
            assert!(!said.contains("Hidden"), "the frame names the type: {said}");
            assert_eq!(frame.error, OPAQUE_CLIENT_MESSAGE);
            let filed = logs
                .events()
                .into_iter()
                .find(|event| event.target == crate::TARGET && event.level == "error")
                .expect("the miss is filed");
            assert!(
                filed
                    .field("provider")
                    .is_some_and(|provider| provider.ends_with("::Hidden")),
                "{filed:#?}"
            );
        }
    }

    /// The probe `#[messages]` asks per parameter, written once at the
    /// reference depth its expansion calls it.
    macro_rules! probe {
        ($ty:ty) => {
            (&&ParamProbe::<$ty>::new()).resolved_provider()
        };
    }

    #[test]
    fn a_scoped_parameter_declares_its_provider_and_a_payload_none() {
        assert_eq!(
            probe!(Scoped<Probe>).map(|(id, _)| id),
            Some(TypeId::of::<Probe>())
        );
        assert_eq!(probe!(String), None);
        assert_eq!(probe!(&Scoped<Probe>), None, "a borrow resolves nothing");
    }
}
