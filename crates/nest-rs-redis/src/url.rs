//! [`RedisUrl`] — `<PREFIX>_REDIS__URL` read apart: its scheme declares the
//! topology and the encryption, its hosts and settings where to find Valkey.
//!
//! A URL naming several hosts follows the grammar fred's `Config::from_url`
//! documents — the authority's host, then each `node` parameter's — so it stays
//! an RFC 3986 URL. A standalone URL is the `redis` client's own.

use std::fmt;

use redis::{ConnectionAddr, ConnectionInfo, ErrorKind, IntoConnectionInfo, RedisConnectionInfo};

use crate::RedisError;

/// Where Valkey listens unless the URL says otherwise.
const NODE_PORT: u16 = 6379;

/// Where Sentinel listens unless the URL says otherwise.
const SENTINEL_PORT: u16 = 26379;

/// Each further sentinel, or each further node of a Cluster.
const NODE: &str = "node";

/// The name the sentinels monitor the primary under.
const SERVICE_NAME: &str = "sentinelServiceName";

/// The user the sentinels authenticate, when they hold users of their own.
const SENTINEL_USERNAME: &str = "sentinelUsername";

/// The password the sentinels authenticate.
const SENTINEL_PASSWORD: &str = "sentinelPassword";

/// Every parameter only a URL naming several hosts takes.
const SEVERAL_HOSTS: [&str; 4] = [NODE, SERVICE_NAME, SENTINEL_USERNAME, SENTINEL_PASSWORD];

/// A URL read apart into the topology its scheme declares.
pub(crate) enum RedisUrl {
    /// One server.
    Standalone(ConnectionInfo),
    /// A primary the sentinels name.
    Sentinel(SentinelUrl),
    /// The nodes of a Cluster.
    Cluster(ClusterUrl),
}

/// A host the URL names — the authority's, or a `node` parameter's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeAddr {
    pub(crate) host: String,
    pub(crate) port: u16,
}

impl fmt::Display for NodeAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// What a `redis-sentinel://` or `rediss-sentinel://` URL names.
pub(crate) struct SentinelUrl {
    /// Every sentinel, in the order the URL names them.
    pub(crate) sentinels: Vec<NodeAddr>,
    /// The name the sentinels monitor the primary under.
    pub(crate) service_name: String,
    /// How a sentinel is authenticated.
    pub(crate) sentinel_settings: RedisConnectionInfo,
    /// How the primary is authenticated, and the database it serves.
    pub(crate) primary_settings: RedisConnectionInfo,
    /// Whether every connection is TLS — to the sentinels and the primary.
    pub(crate) tls: bool,
}

/// What a `redis-cluster://` or `rediss-cluster://` URL names.
pub(crate) struct ClusterUrl {
    /// The nodes the topology is first read from.
    pub(crate) seeds: Vec<NodeAddr>,
    /// How every node is authenticated, and the database each serves.
    pub(crate) settings: RedisConnectionInfo,
    /// Whether every connection is TLS.
    pub(crate) tls: bool,
}

/// The topologies a URL naming several hosts declares.
#[derive(Clone, Copy)]
enum Several {
    Sentinel,
    Cluster,
}

impl Several {
    fn of(scheme: &str) -> Option<(Self, bool)> {
        match scheme {
            "redis-sentinel" => Some((Self::Sentinel, false)),
            "rediss-sentinel" => Some((Self::Sentinel, true)),
            "redis-cluster" => Some((Self::Cluster, false)),
            "rediss-cluster" => Some((Self::Cluster, true)),
            _ => None,
        }
    }

    fn default_port(self) -> u16 {
        match self {
            Self::Sentinel => SENTINEL_PORT,
            Self::Cluster => NODE_PORT,
        }
    }

    /// The parameters this topology's URL takes, as a refusal lists them.
    fn parameters(self) -> String {
        match self {
            Self::Sentinel => format!(
                "`{NODE}`, `{SERVICE_NAME}`, `{SENTINEL_USERNAME}` and `{SENTINEL_PASSWORD}`"
            ),
            Self::Cluster => format!("`{NODE}` alone"),
        }
    }
}

impl RedisUrl {
    /// `raw` read apart, or refused naming what to change — never quoting it.
    pub(crate) fn parse(raw: &str) -> Result<Self, RedisError> {
        match scheme(raw).as_deref().and_then(Several::of) {
            Some((several, tls)) => parse_several(raw, several, tls),
            None => parse_standalone(raw),
        }
    }
}

/// A URL the `redis` client reads on its own, refusing what this crate does not
/// speak: RESP3, a skipped verification, a host no certificate can name, and a
/// parameter that names several hosts.
fn parse_standalone(raw: &str) -> Result<RedisUrl, RedisError> {
    let info = raw
        .into_connection_info()
        .map_err(|source| invalid(raw, source))?;
    if ::url::Url::parse(raw).is_ok_and(|url| {
        url.query_pairs()
            .any(|(key, _)| SEVERAL_HOSTS.contains(&key.as_ref()))
    }) {
        return Err(invalid(
            raw,
            refusal(
                "a URL of one server names no other host",
                "name a Sentinel deployment with rediss-sentinel://, or a Cluster's nodes with \
                 rediss-cluster://"
                    .to_owned(),
            ),
        ));
    }
    if info.redis_settings().protocol() != redis::ProtocolVersion::RESP2 {
        return Err(RedisError::UnsupportedProtocol {
            endpoint: address(raw),
        });
    }
    if let ConnectionAddr::TcpTls { host, insecure, .. } = info.addr() {
        if *insecure {
            return Err(RedisError::UnverifiedTls {
                endpoint: address(raw),
            });
        }
        nameable(raw, host)?;
    }
    let settings = info.redis_settings().clone().set_skip_set_lib_name();
    Ok(RedisUrl::Standalone(info.set_redis_settings(settings)))
}

/// A URL naming several hosts, read as the scheme declares.
fn parse_several(raw: &str, several: Several, tls: bool) -> Result<RedisUrl, RedisError> {
    let url = ::url::Url::parse(raw)
        .map_err(|error| invalid(raw, refusal("the URL does not parse", error.to_string())))?;
    match url.fragment() {
        None => {}
        Some("insecure") => {
            return Err(RedisError::UnverifiedTls {
                endpoint: address(raw),
            });
        }
        Some(_) => {
            return Err(invalid(
                raw,
                refusal(
                    "the URL carries a fragment",
                    "a URL naming several hosts reads none".to_owned(),
                ),
            ));
        }
    }
    let first = authority(&url, several.default_port()).ok_or_else(|| {
        invalid(
            raw,
            refusal(
                "the URL names no host",
                "write the first host after `//`".to_owned(),
            ),
        )
    })?;
    let settings = settings(&url).map_err(|source| invalid(raw, source))?;
    let mut hosts = vec![first];
    let (mut service_name, mut username, mut password) = (None, None, None);
    for (key, value) in url.query_pairs() {
        match (key.as_ref(), several) {
            (NODE, _) => hosts.push(node(&value, several.default_port()).ok_or_else(|| {
                invalid(
                    raw,
                    refusal(
                        "a `node` is a host and its port",
                        "write node=host or node=host:port, with no credentials, path or query"
                            .to_owned(),
                    ),
                )
            })?),
            (SERVICE_NAME, Several::Sentinel) => once(raw, &mut service_name, SERVICE_NAME, value)?,
            (SENTINEL_USERNAME, Several::Sentinel) => {
                once(raw, &mut username, SENTINEL_USERNAME, value)?;
            }
            (SENTINEL_PASSWORD, Several::Sentinel) => {
                once(raw, &mut password, SENTINEL_PASSWORD, value)?;
            }
            (other, _) => {
                return Err(invalid(
                    raw,
                    refusal(
                        "the URL names a parameter it does not take",
                        format!("`{other}`: {} takes {}", url.scheme(), several.parameters()),
                    ),
                ));
            }
        }
    }
    if tls {
        for host in &hosts {
            nameable(raw, &host.host)?;
        }
    }
    match several {
        Several::Cluster => Ok(RedisUrl::Cluster(ClusterUrl {
            seeds: hosts,
            settings,
            tls,
        })),
        Several::Sentinel => {
            let service_name = service_name
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    invalid(
                        raw,
                        refusal(
                            "a Sentinel URL names its primary",
                            format!(
                                "set `{SERVICE_NAME}` to the name the sentinels monitor it under"
                            ),
                        ),
                    )
                })?;
            let mut sentinel_settings = RedisConnectionInfo::default().set_skip_set_lib_name();
            match (username, password) {
                (Some(_), None) => {
                    return Err(invalid(
                        raw,
                        refusal(
                            "a sentinel user is authenticated with its password",
                            format!("set `{SENTINEL_PASSWORD}` beside `{SENTINEL_USERNAME}`"),
                        ),
                    ));
                }
                (username, Some(password)) => {
                    if let Some(username) = username {
                        sentinel_settings = sentinel_settings.set_username(username);
                    }
                    sentinel_settings = sentinel_settings.set_password(password);
                }
                (None, None) => {}
            }
            Ok(RedisUrl::Sentinel(SentinelUrl {
                sentinels: hosts,
                service_name,
                sentinel_settings,
                primary_settings: settings,
                tls,
            }))
        }
    }
}

/// The hosts a URL names, as a diagnostic lists them.
pub(crate) fn listed(hosts: &[NodeAddr]) -> String {
    hosts
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The scheme of `raw`, lowercased, when it has one.
fn scheme(raw: &str) -> Option<String> {
    raw.split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
}

/// The authority's host and port.
fn authority(url: &::url::Url, default_port: u16) -> Option<NodeAddr> {
    let host = match url.host()? {
        ::url::Host::Domain(domain) if !domain.is_empty() => domain.to_owned(),
        ::url::Host::Domain(_) => return None,
        ::url::Host::Ipv4(address) => address.to_string(),
        ::url::Host::Ipv6(address) => address.to_string(),
    };
    Some(NodeAddr {
        host,
        port: url.port().unwrap_or(default_port),
    })
}

/// A `node` parameter's host and port: nothing else may ride in it.
fn node(value: &str, default_port: u16) -> Option<NodeAddr> {
    let url = ::url::Url::parse(&format!("node://{value}")).ok()?;
    let bare = url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none();
    bare.then(|| authority(&url, default_port)).flatten()
}

/// The credentials and the database a URL naming several hosts gives every
/// data node, read by the `redis` client as it reads a URL of one server.
fn settings(url: &::url::Url) -> Result<RedisConnectionInfo, redis::RedisError> {
    let userinfo = match (url.username(), url.password()) {
        ("", None) => String::new(),
        (username, None) => format!("{username}@"),
        (username, Some(password)) => format!("{username}:{password}@"),
    };
    let info = format!("redis://{userinfo}localhost{}", url.path()).into_connection_info()?;
    Ok(info.redis_settings().clone().set_skip_set_lib_name())
}

/// Keep `value` for `key`, refusing a second one.
fn once(
    raw: &str,
    kept: &mut Option<String>,
    key: &str,
    value: std::borrow::Cow<'_, str>,
) -> Result<(), RedisError> {
    if kept.replace(value.into_owned()).is_some() {
        return Err(invalid(
            raw,
            refusal(
                "a parameter is given more than once",
                format!("`{key}` is given more than once"),
            ),
        ));
    }
    Ok(())
}

/// Refuse a TLS host no certificate can name, as the handshake parses it.
fn nameable(raw: &str, host: &str) -> Result<(), RedisError> {
    rustls::pki_types::ServerName::try_from(host)
        .map(|_| ())
        .map_err(|error| invalid(raw, error.into()))
}

fn refusal(description: &'static str, detail: String) -> redis::RedisError {
    redis::RedisError::from((ErrorKind::InvalidClientConfig, description, detail))
}

fn invalid(raw: &str, source: redis::RedisError) -> RedisError {
    RedisError::InvalidUrl {
        endpoint: address(raw),
        source,
    }
}

/// The addresses a URL names, for a diagnostic: the hosts the client dials,
/// never the URL, which may carry a password. A URL that cannot be read apart
/// is named by its scheme alone, and by nothing at all when what precedes `://`
/// is not a scheme and may be a credential.
pub(crate) fn address(raw: &str) -> String {
    let scheme = scheme(raw);
    if let Some((several, _)) = scheme.as_deref().and_then(Several::of) {
        return hosts(raw, several).map_or_else(
            || format!("{}://<unparseable>", scheme.unwrap_or_default()),
            |hosts| listed(&hosts),
        );
    }
    match raw.into_connection_info() {
        Ok(info) => info.addr().to_string(),
        Err(_) => match raw.split_once("://") {
            Some((scheme, _)) if is_scheme(scheme) => format!("{scheme}://<unparseable>"),
            _ => "<unparseable>".to_owned(),
        },
    }
}

/// Every host a URL naming several hosts dials, or `None` when one does not
/// read.
fn hosts(raw: &str, several: Several) -> Option<Vec<NodeAddr>> {
    let url = ::url::Url::parse(raw).ok()?;
    let mut hosts = vec![authority(&url, several.default_port())?];
    for (key, value) in url.query_pairs() {
        if key == NODE {
            hosts.push(node(&value, several.default_port())?);
        }
    }
    Some(hosts)
}

/// RFC 3986 §3.1: a letter, then letters, digits, `+`, `-` or `.`.
fn is_scheme(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RedisTopology;

    trait Declared {
        fn topology(&self) -> RedisTopology;
    }

    impl Declared for RedisUrl {
        fn topology(&self) -> RedisTopology {
            match self {
                RedisUrl::Standalone(_) => RedisTopology::Standalone,
                RedisUrl::Sentinel(_) => RedisTopology::Sentinel,
                RedisUrl::Cluster(_) => RedisTopology::Cluster,
            }
        }
    }

    fn parsed(raw: &str) -> RedisUrl {
        RedisUrl::parse(raw).unwrap_or_else(|error| panic!("{raw} parses: {error}"))
    }

    fn sentinel(raw: &str) -> SentinelUrl {
        match parsed(raw) {
            RedisUrl::Sentinel(sentinel) => sentinel,
            other => panic!("{raw} is a Sentinel URL, read as {}", other.topology()),
        }
    }

    fn cluster(raw: &str) -> ClusterUrl {
        match parsed(raw) {
            RedisUrl::Cluster(cluster) => cluster,
            other => panic!("{raw} is a Cluster URL, read as {}", other.topology()),
        }
    }

    fn node(host: &str, port: u16) -> NodeAddr {
        NodeAddr {
            host: host.to_owned(),
            port,
        }
    }

    /// The refusal of `raw`, rendered with its cause, which must not quote
    /// anything after the scheme.
    fn refused(raw: &str) -> (RedisError, String) {
        let Err(error) = RedisUrl::parse(raw) else {
            panic!("{raw} is refused")
        };
        let rendered = nest_rs_core::error_message(&error);
        (error, rendered)
    }

    #[test]
    fn the_scheme_declares_the_topology() {
        for (raw, topology) in [
            ("redis://valkey:6379/2", RedisTopology::Standalone),
            ("rediss://valkey:6379/2", RedisTopology::Standalone),
            ("redis+unix:///run/valkey.sock", RedisTopology::Standalone),
            (
                "rediss-sentinel://s1:26379?sentinelServiceName=primary",
                RedisTopology::Sentinel,
            ),
            (
                "redis-sentinel://s1?sentinelServiceName=primary",
                RedisTopology::Sentinel,
            ),
            ("rediss-cluster://n1:7000", RedisTopology::Cluster),
            ("redis-cluster://n1:7000", RedisTopology::Cluster),
            ("REDISS-CLUSTER://n1:7000", RedisTopology::Cluster),
        ] {
            assert_eq!(parsed(raw).topology(), topology, "{raw}");
        }
    }

    #[test]
    fn a_sentinel_url_names_every_sentinel_the_primary_and_both_credentials() {
        let url = sentinel(
            "rediss-sentinel://app:p%40ss@s1:26379/2?node=s2&node=s3:26380&node=[::1]:26381\
             &sentinelServiceName=nestrs&sentinelUsername=watch&sentinelPassword=p%2Bw",
        );
        assert_eq!(
            url.sentinels,
            [
                node("s1", 26379),
                node("s2", 26379),
                node("s3", 26380),
                node("::1", 26381)
            ]
        );
        assert_eq!(url.service_name, "nestrs");
        assert!(url.tls);
        assert_eq!(url.primary_settings.username(), Some("app"));
        assert_eq!(url.primary_settings.password(), Some("p@ss"));
        assert_eq!(url.primary_settings.db(), 2);
        assert_eq!(url.sentinel_settings.username(), Some("watch"));
        assert_eq!(url.sentinel_settings.password(), Some("p+w"));
        assert_eq!(url.sentinel_settings.db(), 0, "a sentinel has no database");
    }

    #[test]
    fn a_sentinel_without_credentials_of_its_own_is_dialled_with_none() {
        let url = sentinel("redis-sentinel://app:pw@s1?sentinelServiceName=nestrs");
        assert_eq!(url.sentinels, [node("s1", 26379)], "Sentinel's own port");
        assert!(!url.tls);
        assert_eq!(url.sentinel_settings.username(), None);
        assert_eq!(url.sentinel_settings.password(), None);
        assert_eq!(url.primary_settings.password(), Some("pw"));
        assert_eq!(url.primary_settings.db(), 0);
    }

    #[test]
    fn a_password_alone_authenticates_a_sentinel_as_its_default_user() {
        let url = sentinel("rediss-sentinel://s1?sentinelServiceName=nestrs&sentinelPassword=pw");
        assert_eq!(url.sentinel_settings.username(), None);
        assert_eq!(url.sentinel_settings.password(), Some("pw"));
    }

    #[test]
    fn a_cluster_url_names_its_seeds_and_the_database_every_node_selects() {
        let url = cluster("rediss-cluster://app:pw@n1:7000/3?node=n2:7001&node=n3");
        assert_eq!(
            url.seeds,
            [node("n1", 7000), node("n2", 7001), node("n3", 6379)]
        );
        assert!(url.tls);
        assert_eq!(url.settings.username(), Some("app"));
        assert_eq!(url.settings.password(), Some("pw"));
        assert_eq!(url.settings.db(), 3);
        assert!(!cluster("redis-cluster://n1").tls);
        assert_eq!(cluster("redis-cluster://n1").seeds, [node("n1", 6379)]);
    }

    #[test]
    fn no_node_announces_the_client_library() {
        assert!(cluster("rediss-cluster://n1").settings.skip_set_lib_name());
        let url = sentinel("rediss-sentinel://s1?sentinelServiceName=nestrs");
        assert!(url.primary_settings.skip_set_lib_name());
        assert!(url.sentinel_settings.skip_set_lib_name());
    }

    #[test]
    fn a_parameter_the_grammar_does_not_take_is_refused_by_its_name_alone() {
        for (raw, named) in [
            ("rediss-cluster://n1?protocol=resp3", "protocol"),
            (
                "rediss-cluster://n1?sentinelServiceName=s3cr3t",
                "sentinelServiceName",
            ),
            (
                "rediss-cluster://n1?sentinelPassword=s3cr3t",
                "sentinelPassword",
            ),
            (
                "rediss-sentinel://s1?sentinelServiceName=m&sentinelPasword=s3cr3t",
                "sentinelPasword",
            ),
        ] {
            let (error, rendered) = refused(raw);
            assert!(
                matches!(error, RedisError::InvalidUrl { .. }),
                "{raw}: {rendered}"
            );
            assert!(
                rendered.contains(&format!("`{named}`")),
                "{raw}: {rendered}"
            );
            assert!(!rendered.contains("s3cr3t"), "{raw}: {rendered}");
        }
    }

    #[test]
    fn a_standalone_url_naming_several_hosts_is_sent_to_the_topology_schemes() {
        for raw in [
            "rediss://valkey:6379?node=valkey-2:6379",
            "rediss://valkey:6379?sentinelServiceName=nestrs",
        ] {
            let (error, rendered) = refused(raw);
            assert!(
                matches!(error, RedisError::InvalidUrl { .. }),
                "{raw}: {rendered}"
            );
            assert!(
                rendered.contains("rediss-sentinel://") && rendered.contains("rediss-cluster://"),
                "{raw}: {rendered}"
            );
        }
    }

    #[test]
    fn a_sentinel_url_names_its_primary_once_and_its_credentials_whole() {
        for (raw, says) in [
            ("rediss-sentinel://s1", "sentinelServiceName"),
            (
                "rediss-sentinel://s1?sentinelServiceName=",
                "sentinelServiceName",
            ),
            (
                "rediss-sentinel://s1?sentinelServiceName=a&sentinelServiceName=b",
                "once",
            ),
            (
                "rediss-sentinel://s1?sentinelServiceName=a&sentinelUsername=watch",
                "sentinelPassword",
            ),
        ] {
            let (error, rendered) = refused(raw);
            assert!(
                matches!(error, RedisError::InvalidUrl { .. }),
                "{raw}: {rendered}"
            );
            assert!(rendered.contains(says), "{raw}: {rendered}");
        }
    }

    #[test]
    fn a_node_is_a_host_and_a_port_and_nothing_else() {
        for raw in [
            "rediss-cluster://n1?node=app:s3cr3t@n2:7001",
            "rediss-cluster://n1?node=n2:7001/2",
            "rediss-cluster://n1?node=n2:notaport",
            "rediss-cluster://n1?node=",
        ] {
            let (error, rendered) = refused(raw);
            assert!(
                matches!(error, RedisError::InvalidUrl { .. }),
                "{raw}: {rendered}"
            );
            assert!(rendered.contains("`node`"), "{raw}: {rendered}");
            assert!(!rendered.contains("s3cr3t"), "{raw}: {rendered}");
        }
    }

    #[test]
    fn a_url_that_does_not_name_a_reachable_host_and_database_is_invalid() {
        for raw in [
            "rediss-cluster:///2",
            "rediss-cluster://n1/notadatabase",
            "rediss-cluster://-leading:7000",
            "rediss-sentinel://s1?node=-leading&sentinelServiceName=nestrs",
            "rediss-cluster://n1#primary",
        ] {
            let (error, rendered) = refused(raw);
            assert!(
                matches!(error, RedisError::InvalidUrl { .. }),
                "{raw}: {rendered}"
            );
        }
    }

    #[test]
    fn verification_is_never_skipped_on_any_topology() {
        for raw in [
            "rediss://valkey:6379/#insecure",
            "rediss-sentinel://s1?sentinelServiceName=nestrs#insecure",
            "rediss-cluster://n1:7000#insecure",
        ] {
            let (error, rendered) = refused(raw);
            assert!(
                matches!(error, RedisError::UnverifiedTls { .. }),
                "{raw}: {rendered}"
            );
        }
    }

    #[test]
    fn resp3_is_refused_on_a_standalone_url() {
        let (error, rendered) = refused("redis://valkey:6379/?protocol=resp3");
        assert!(
            matches!(error, RedisError::UnsupportedProtocol { .. }),
            "{rendered}"
        );
    }

    #[test]
    fn the_endpoint_is_the_address_the_client_dials_and_never_a_credential() {
        for (url, shown) in [
            (
                "redis://alice:s3cr3t@redis.internal:6379/2",
                "redis.internal:6379",
            ),
            ("redis://127.0.0.1:6379/0", "127.0.0.1:6379"),
            (
                "redis+unix:///tmp/redis.sock?pass=s3cr3t",
                "/tmp/redis.sock",
            ),
            ("redis://user:s3cr3t@[::bad-host/", "redis://<unparseable>"),
            ("not-a-url", "<unparseable>"),
            (
                "rediss-sentinel://app:s3cr3t@s1:26379/0?node=s2:26380&sentinelServiceName=nestrs\
                 &sentinelPassword=s3cr3t",
                "s1:26379, s2:26380",
            ),
            (
                "rediss-cluster://app:s3cr3t@n1/0?node=n2:7001",
                "n1:6379, n2:7001",
            ),
            (
                "rediss-cluster://app:s3cr3t@n1?node=app:s3cr3t@n2",
                "rediss-cluster://<unparseable>",
            ),
        ] {
            assert_eq!(address(url), shown, "{url}");
        }
        for url in [
            "redis://alice:p@s3cr3t@redis:6379",
            "redis://alice:pa/s3cr3t@127.0.0.1:9/",
            "redis://alice:pa?s3cr3t@127.0.0.1:9/",
            "unix:/tmp/redis.sock?pass=s3cr3t",
            "redis:/alice:s3cr3t@127.0.0.1:9/0",
            "redis:\\\\alice:s3cr3t@127.0.0.1:9/0",
            "s3cr3t@redis:6379",
            "alice:s3cr3t@redis:6379/0",
            "s3cr3t@redis://redis:6379",
            "rediss-sentinel://alice:pa/s3cr3t@s1?sentinelServiceName=x",
        ] {
            let shown = address(url);
            assert!(!shown.contains("s3cr3t"), "{url} showed {shown}");
        }
    }
}
