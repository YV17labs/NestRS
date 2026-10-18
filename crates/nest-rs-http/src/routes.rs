//! [`HttpRoutes`], the route table every controller and surface registers on,
//! which the transport reads whole: each address, each method, and who
//! registered it.

use std::collections::HashMap;
use std::sync::Arc;

use http::Method;

use crate::endpoint::{BoxEndpoint, Endpoint};
use crate::error::RouteError;
use crate::fallback::is_under;
use crate::poem_adapter::PoemLeaf;
use crate::route_template::RouteTemplate;
use crate::transport::join_path;
use crate::versioning::route_matches;

/// The route table being assembled, in nestrs's route grammar (`/users/{id}`,
/// `/@{handle}`, `/files/{*rest}`), under the global prefix.
///
/// A registration the table refuses — outside the grammar, a second owner of
/// one address, a path a self-mount did not declare — is recorded and fails
/// the boot once the mount returns, naming the registrant: a mount refuses, it
/// never panics.
///
/// ```
/// use nest_rs_core::Container;
/// use nest_rs_http::{HttpRoutes, Method, Request, Response, endpoint_fn};
///
/// fn mount(_container: &Container, routes: &mut HttpRoutes) {
///     routes.route(
///         Method::GET,
///         "/probes/{name}",
///         endpoint_fn(|req: Request| async move {
///             let name = req.path_param("name").unwrap_or_default().to_owned();
///             Ok(Response::builder().body(name))
///         }),
///     );
/// }
/// ```
pub struct HttpRoutes {
    prefix: Option<String>,
    registrant: Registrant,
    addresses: Vec<Address>,
    /// Where each address sits in `addresses`, by its identity.
    by_identity: HashMap<String, usize>,
    /// Every template a self-mount registered, without the global prefix.
    self_mounted: Vec<String>,
    refused: Option<RouteError>,
}

/// Who registers, while its mount runs.
struct Registrant {
    name: Arc<str>,
    /// The templates a self-mount declared; `None` for a controller.
    declared: Option<Vec<RouteTemplate>>,
    /// What each entry is put behind: the guard pool, for a self-mount gated
    /// at its edge.
    wrap: Option<Wrap>,
}

/// An endpoint put behind something.
pub(crate) type Wrap = Arc<dyn Fn(BoxEndpoint) -> BoxEndpoint + Send + Sync>;

/// What one registration answers with.
pub(crate) enum Leaf {
    /// A nestrs endpoint.
    Endpoint(BoxEndpoint),
    /// An endpoint still written on poem, which reads poem's own route
    /// parameters; removed once every registrant speaks the vocabulary.
    Poem(PoemLeaf),
}

impl Leaf {
    fn into_endpoint(self) -> BoxEndpoint {
        match self {
            Self::Endpoint(endpoint) => endpoint,
            Self::Poem(leaf) => leaf.into_endpoint(),
        }
    }
}

/// One address of the table: its template, who registered it, and what
/// answers there.
pub(crate) struct Address {
    pub(crate) template: RouteTemplate,
    pub(crate) owner: Arc<str>,
    pub(crate) dispatch: Dispatch,
}

impl Address {
    /// The methods this address answers by name; none for an entry answering
    /// every method.
    pub(crate) fn methods(&self) -> impl Iterator<Item = &Method> {
        let named: &[(Method, Leaf)] = match &self.dispatch {
            Dispatch::Any(_) => &[],
            Dispatch::Methods(methods) => methods,
        };
        named.iter().map(|(method, _)| method)
    }
}

/// What answers at one address.
pub(crate) enum Dispatch {
    /// One entry for every method: it dispatches on the method itself.
    Any(Leaf),
    /// One entry per method, in registration order.
    Methods(Vec<(Method, Leaf)>),
}

impl HttpRoutes {
    /// `endpoint` answers `method` at `template`. The table owns the method
    /// dispatch: another method at that address answers `405` with `Allow`,
    /// and `HEAD` is served by `GET` when not registered (RFC 9110 §9.3.2,
    /// §15.5.6).
    pub fn route(&mut self, method: Method, template: &str, endpoint: impl Endpoint) -> &mut Self {
        self.register(
            Some(method),
            template,
            Leaf::Endpoint(BoxEndpoint::new(endpoint)),
        );
        self
    }

    /// `endpoint` answers every method at `template`: a protocol that
    /// dispatches on the method itself (MCP).
    pub fn route_any(&mut self, template: &str, endpoint: impl Endpoint) -> &mut Self {
        self.register(None, template, Leaf::Endpoint(BoxEndpoint::new(endpoint)));
        self
    }

    /// [`route`](Self::route) for an endpoint still written on poem.
    #[doc(hidden)]
    pub fn route_poem<E>(&mut self, method: Method, template: &str, endpoint: E) -> &mut Self
    where
        E: poem::IntoEndpoint,
        E::Endpoint: 'static,
    {
        self.register(Some(method), template, Leaf::Poem(PoemLeaf::new(endpoint)));
        self
    }

    /// [`route_any`](Self::route_any) for an endpoint still written on poem.
    #[doc(hidden)]
    pub fn route_any_poem<E>(&mut self, template: &str, endpoint: E) -> &mut Self
    where
        E: poem::IntoEndpoint,
        E::Endpoint: 'static,
    {
        self.register(None, template, Leaf::Poem(PoemLeaf::new(endpoint)));
        self
    }

    /// An empty table whose every template is registered under `prefix`, a
    /// canonical literal path.
    pub(crate) fn new(prefix: Option<&str>) -> Self {
        Self {
            prefix: prefix.map(str::to_owned),
            registrant: Registrant {
                name: Arc::from("the transport"),
                declared: None,
                wrap: None,
            },
            addresses: Vec::new(),
            by_identity: HashMap::new(),
            self_mounted: Vec::new(),
            refused: None,
        }
    }

    /// What follows is registered by the controller `name`.
    pub(crate) fn for_controller(&mut self, name: &str) -> &mut Self {
        self.registrant = Registrant {
            name: Arc::from(name),
            declared: None,
            wrap: None,
        };
        self
    }

    /// What follows is registered by the self-mount `name`, at the paths it
    /// `declared`, each entry put behind `wrap`.
    pub(crate) fn for_self_mount(
        &mut self,
        name: String,
        declared: Vec<RouteTemplate>,
        wrap: Option<Wrap>,
    ) -> &mut Self {
        self.registrant = Registrant {
            name: Arc::from(name),
            declared: Some(declared),
            wrap,
        };
        self
    }

    /// The first registration refused since the last call.
    pub(crate) fn settle(&mut self) -> Result<(), RouteError> {
        self.refused.take().map_or(Ok(()), Err)
    }

    /// The templates the self-mounts registered, as written (no global
    /// prefix), each once: the addresses a version selector serves as sent.
    pub(crate) fn take_self_mounted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.self_mounted)
    }

    /// The table, compiled.
    pub(crate) fn into_addresses(self) -> Vec<Address> {
        self.addresses
    }

    fn register(&mut self, method: Option<Method>, template: &str, leaf: Leaf) {
        if self.refused.is_some() {
            return;
        }
        if let Err(refusal) = self.try_register(method, template, leaf) {
            self.refused = Some(refusal);
        }
    }

    fn try_register(
        &mut self,
        method: Option<Method>,
        template: &str,
        leaf: Leaf,
    ) -> Result<(), RouteError> {
        let owner = Arc::clone(&self.registrant.name);
        let refused = |refused| RouteError::Template {
            owner: owner.to_string(),
            refused,
        };
        let written = RouteTemplate::parse(template).map_err(refused)?;
        if let Some(declared) = &self.registrant.declared
            && !declared.iter().any(|path| declares(path, &written))
        {
            return Err(RouteError::Undeclared {
                owner: owner.to_string(),
                template: written.as_str().to_owned(),
                declared: declared
                    .iter()
                    .map(|path| format!("{:?}", path.as_str()))
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        let self_mounted = self
            .registrant
            .declared
            .is_some()
            .then(|| written.as_str().to_owned());
        let template = match &self.prefix {
            Some(prefix) => {
                RouteTemplate::parse(&join_path(prefix, written.as_str())).map_err(refused)?
            }
            None => written,
        };
        let leaf = match &self.registrant.wrap {
            Some(wrap) => Leaf::Endpoint(wrap(leaf.into_endpoint())),
            None => leaf,
        };
        let Some(&at) = self.by_identity.get(template.identity()) else {
            self.by_identity
                .insert(template.identity().to_owned(), self.addresses.len());
            self.self_mounted.extend(self_mounted);
            self.addresses.push(Address {
                template,
                owner,
                dispatch: match method {
                    Some(method) => Dispatch::Methods(vec![(method, leaf)]),
                    None => Dispatch::Any(leaf),
                },
            });
            return Ok(());
        };
        let address = &mut self.addresses[at];
        if address.template.as_str() != template.as_str() {
            return Err(RouteError::OneAddress {
                first: address.owner.to_string(),
                first_template: address.template.as_str().to_owned(),
                second: owner.to_string(),
                second_template: template.as_str().to_owned(),
            });
        }
        if address.owner != owner {
            return Err(RouteError::SharedAddress {
                template: template.as_str().to_owned(),
                first: address.owner.to_string(),
                second: owner.to_string(),
            });
        }
        match (&mut address.dispatch, method) {
            (Dispatch::Methods(entries), Some(method))
                if !entries.iter().any(|(listed, _)| *listed == method) =>
            {
                entries.push((method, leaf));
                Ok(())
            }
            (_, method) => Err(RouteError::Twice {
                owner: owner.to_string(),
                method: method.map_or_else(
                    || "every method".to_owned(),
                    |method| method.as_str().to_owned(),
                ),
                template: template.as_str().to_owned(),
            }),
        }
    }
}

/// Whether a self-mount that declared `path` may register `template`: the
/// same address, one below it, or one its catch-all takes.
fn declares(path: &RouteTemplate, template: &RouteTemplate) -> bool {
    path.identity() == template.identity()
        || is_under(template.identity(), path.identity())
        || (path.catch_all().is_some() && route_matches(template.as_str(), path.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::endpoint_fn;
    use crate::response::Response;

    fn ok() -> impl Endpoint {
        endpoint_fn(|_| async { Ok(Response::default()) })
    }

    fn template(written: &str) -> RouteTemplate {
        RouteTemplate::parse(written).unwrap()
    }

    fn templates(routes: HttpRoutes) -> Vec<String> {
        routes
            .into_addresses()
            .into_iter()
            .map(|address| address.template.as_str().to_owned())
            .collect()
    }

    #[test]
    fn the_global_prefix_is_applied_at_registration() {
        let mut routes = HttpRoutes::new(Some("/api"));
        routes.for_controller("UsersController");
        routes
            .route(Method::GET, "/users/{id}", ok())
            .route(Method::GET, "/", ok());
        routes.settle().unwrap();
        assert_eq!(templates(routes), ["/api/users/{id}", "/api"]);
    }

    #[test]
    fn methods_of_one_address_share_it_and_one_method_twice_is_refused() {
        let mut routes = HttpRoutes::new(None);
        routes.for_controller("UsersController");
        routes
            .route(Method::GET, "/users", ok())
            .route(Method::POST, "/users/", ok());
        routes.settle().unwrap();
        routes.route(Method::GET, "/users", ok());
        let refused = routes.settle().unwrap_err().to_string();
        assert!(
            refused.contains("UsersController") && refused.contains("for GET"),
            "{refused}"
        );

        routes
            .route_any("/any", ok())
            .route(Method::GET, "/any", ok());
        let refused = routes.settle().unwrap_err().to_string();
        assert!(refused.contains("for GET"), "{refused}");
        routes.route_any("/any", ok());
        let refused = routes.settle().unwrap_err().to_string();
        assert!(refused.contains("for every method"), "{refused}");
    }

    #[test]
    fn two_shapes_of_one_address_are_refused_naming_both() {
        let mut routes = HttpRoutes::new(None);
        routes.for_controller("ParcelsController");
        routes.route(Method::GET, "/parcels/{id}", ok());
        routes.for_controller("LockersController");
        routes.route(Method::DELETE, "/parcels/{key}", ok());
        let refused = routes.settle().unwrap_err().to_string();
        for named in [
            "ParcelsController",
            "\"/parcels/{id}\"",
            "LockersController",
            "\"/parcels/{key}\"",
        ] {
            assert!(refused.contains(named), "names {named}: {refused}");
        }
    }

    #[test]
    fn one_address_has_one_owner() {
        let mut routes = HttpRoutes::new(None);
        routes.for_controller("UsersController");
        routes.route(Method::GET, "/users", ok());
        routes.for_self_mount(
            "the ws endpoint Chat".into(),
            vec![template("/users")],
            None,
        );
        routes.route(Method::POST, "/users", ok());
        let refused = routes.settle().unwrap_err().to_string();
        assert!(
            refused.contains("UsersController") && refused.contains("the ws endpoint Chat"),
            "{refused}"
        );
    }

    #[test]
    fn a_self_mount_registers_what_it_declared_and_below() {
        let mut routes = HttpRoutes::new(Some("/api"));
        routes.for_self_mount(
            "the docs endpoint Docs".into(),
            vec![template("/docs"), template("/docs-json/{*version}")],
            None,
        );
        for path in ["/docs", "/docs/assets/{*file}", "/docs-json/v2"] {
            routes.route(Method::GET, path, ok());
            routes
                .settle()
                .unwrap_or_else(|refused| panic!("{path}: {refused}"));
        }
        routes.route(Method::POST, "/docs", ok());
        routes.settle().unwrap();
        assert_eq!(
            routes.take_self_mounted(),
            ["/docs", "/docs/assets/{*file}", "/docs-json/v2"],
            "what it registered, unprefixed, each address once"
        );
        routes.route(Method::GET, "/admin", ok());
        let refused = routes.settle().unwrap_err().to_string();
        assert!(
            refused.contains("the docs endpoint Docs")
                && refused.contains("\"/admin\"")
                && refused.contains("\"/docs\", \"/docs-json/{*version}\""),
            "{refused}"
        );
        routes.route(Method::GET, "/docsy", ok());
        assert!(
            routes.settle().is_err(),
            "a shared spelling is not a segment"
        );
    }

    #[test]
    fn a_template_out_of_the_grammar_is_refused_naming_its_registrant() {
        let mut routes = HttpRoutes::new(None);
        routes.for_controller("UsersController");
        routes.route(Method::GET, "/users/:id", ok());
        let refused = routes.settle().unwrap_err().to_string();
        assert!(
            refused.starts_with("UsersController mounts `/users/:id`"),
            "{refused}"
        );
    }

    #[test]
    fn the_first_refusal_is_kept() {
        let mut routes = HttpRoutes::new(None);
        routes.for_controller("UsersController");
        routes
            .route(Method::GET, "/a/:x", ok())
            .route(Method::GET, "/b/:y", ok());
        let refused = routes.settle().unwrap_err().to_string();
        assert!(refused.contains("/a/:x"), "{refused}");
        assert!(routes.settle().is_ok(), "settled once");
    }
}
