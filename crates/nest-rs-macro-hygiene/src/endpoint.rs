//! A package's own self-mount, through the umbrella's `http` alone: the site
//! `declare_endpoint` builds in a `static`, and the `HttpEndpointMeta` naming
//! it, as a package mounting a surface of its own writes them.

use nest_rs::core::{ContainerBuilder, Handler, Module, Posture, Registering};
use nest_rs::http::{HttpEndpointMeta, Method, Response, declare_endpoint, endpoint_fn};

/// A package's module, serving its status page beside the app's routes.
pub struct HygieneStatusModule;

static STATUS: Handler =
    declare_endpoint::<HygieneStatusModule>("HygieneStatusModule", "/status", Posture::Public);

impl Module for HygieneStatusModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(HttpEndpointMeta::new(
            &STATUS,
            "/status",
            "status",
            |_, routes| {
                routes.route(
                    Method::GET,
                    "/status",
                    endpoint_fn(|_req| async { Ok(Response::builder().body("up")) }),
                );
            },
        ))
    }
}
