//! [`ClusterLink`] — the link to a Cluster's nodes: `redis`'s cluster
//! connection, which reads the slots from the seeds the URL names, sends each
//! command to the node serving its keys, follows `MOVED` and `ASK`, and reads
//! the slots again once a failover moved them.
//!
//! Each node is reached through [`ClusterNode`], so a node's connection
//! announces no client library — `CLIENT SETINFO`, which an ACL user confined to
//! a binding's commands is refused — and a node refusing a TLS connection is
//! said once.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::cluster::{ClusterClient, ClusterClientBuilder};
use redis::cluster_async::{ClusterConnection, Connect};
use redis::cluster_routing::{
    MultipleNodeRoutingInfo, ResponsePolicy, Route, RoutingInfo, SingleNodeRoutingInfo, Slot,
    SlotAddr,
};
use redis::{
    AsyncConnectionConfig, Cmd, ConnectionAddr, IntoConnectionInfo, Pipeline, RedisConnectionInfo,
    RedisFuture, TlsMode, Value,
};

use crate::connection::{
    Attempt, FIRST_RETRY_BACKOFF, GaveUp, MAX_RETRY_BACKOFF, bounded, classify, connection_config,
    socket, spent, within_budget,
};
use crate::error::RedisError;
use crate::script::RedisScript;
use crate::url::{ClusterUrl, NodeAddr};
use crate::{RedisTls, RedisTopology, tls};

/// The link to a Cluster's nodes.
pub(crate) struct ClusterLink {
    plan: Arc<Plan>,
    connection: ClusterConnection<ClusterNode>,
}

/// What opening a cluster connection needs, shared by a link and every link
/// it dedicates.
struct Plan {
    seeds: Vec<NodeAddr>,
    /// Every seed's address, as an error names them.
    listed: String,
    settings: RedisConnectionInfo,
    certificates: Option<redis::TlsCertificates>,
    budget: Duration,
}

impl ClusterLink {
    /// Open the link to the Cluster `url`'s seeds belong to, proved within
    /// `budget`: a seed answers `CLUSTER SLOTS`, and every primary a `PING`.
    pub(crate) async fn connect(
        url: ClusterUrl,
        tls: &RedisTls,
        budget: Duration,
    ) -> Result<Arc<Self>, RedisError> {
        let listed = url
            .seeds
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let certificates = tls::material(tls, url.tls, &listed)?;
        let plan = Arc::new(Plan {
            seeds: url.seeds,
            listed,
            settings: url.settings,
            certificates,
            budget,
        });
        let client = plan
            .client(budget)
            .map_err(|source| RedisError::InvalidUrl {
                endpoint: plan.listed.clone(),
                source,
            })?;
        let connection = within_budget(budget, &plan.listed, |_| plan.prove(&client))
            .await
            .map_err(|gave_up| match gave_up {
                GaveUp::Refused(refusal) => refusal,
                GaveUp::Spent { attempts, last } => {
                    spent(plan.listed.clone(), budget, attempts, last)
                }
            })?;
        Ok(Arc::new(Self { plan, connection }))
    }

    /// A link of its own to the same Cluster, for a command that blocks: each
    /// node's answer may take `response`.
    pub(crate) async fn dedicated(
        &self,
        response: Duration,
    ) -> Result<Arc<Self>, redis::RedisError> {
        let client = self.plan.client(response)?;
        let connection = bounded(
            self.plan.budget,
            client.get_async_generic_connection::<ClusterNode>(),
        )
        .await??;
        Ok(Arc::new(Self {
            plan: Arc::clone(&self.plan),
            connection,
        }))
    }

    /// Send `cmd` to the node its keys sit on, answered or failed within
    /// `budget`, redirections and a slot refresh included.
    pub(crate) async fn send(
        &self,
        cmd: &Cmd,
        budget: Duration,
    ) -> Result<Value, redis::RedisError> {
        bounded(budget, self.connection.clone().req_packed_command(cmd)).await?
    }

    /// Send `count` replies' worth of `pipeline` from `offset` within `budget`.
    pub(crate) async fn send_pipeline(
        &self,
        pipeline: &Pipeline,
        offset: usize,
        count: usize,
        budget: Duration,
    ) -> Result<Vec<Value>, redis::RedisError> {
        bounded(
            budget,
            self.connection
                .clone()
                .req_packed_commands(pipeline, offset, count),
        )
        .await?
    }

    /// Cache `script` on the primary serving `slot`, or on every primary when
    /// the call names no key — never on every node, so a replica that is down
    /// fails no load.
    pub(crate) async fn load(
        &self,
        script: &RedisScript,
        slot: Option<Slot>,
        budget: Duration,
    ) -> Result<(), redis::RedisError> {
        let routing = match slot {
            Some(slot) => RoutingInfo::SingleNode(SingleNodeRoutingInfo::SpecificNode(
                Route::with_slot(slot, SlotAddr::Master),
            )),
            None => RoutingInfo::MultiNode((
                MultipleNodeRoutingInfo::AllMasters,
                Some(ResponsePolicy::AllSucceeded),
            )),
        };
        bounded(
            budget,
            self.connection
                .clone()
                .route_command(script.load_cmd(), routing),
        )
        .await?
        .map(drop)
    }

    /// The database the URL selects on every node.
    pub(crate) fn db(&self) -> i64 {
        self.plan.settings.db()
    }
}

impl Plan {
    /// The cluster client every connection is opened from, each node's answer
    /// bounded by `response`. Its retries fit the budget, which
    /// [`bounded`] holds every command to whatever they do.
    fn client(&self, response: Duration) -> Result<ClusterClient, redis::RedisError> {
        let seeds = self
            .seeds
            .iter()
            .map(|seed| addr(seed, self.certificates.is_some()).into_connection_info())
            .collect::<Result<Vec<_>, _>>()?;
        let mut builder = ClusterClientBuilder::new(seeds)
            .connection_timeout(self.budget)
            .response_timeout(response)
            .overall_response_timeout(None)
            .min_retry_wait(millis(FIRST_RETRY_BACKOFF))
            .max_retry_wait(millis(MAX_RETRY_BACKOFF))
            .tcp_settings(socket(self.budget))
            .use_protocol(redis::ProtocolVersion::RESP2);
        if let Some(certificates) = &self.certificates {
            builder = builder.tls(TlsMode::Secure).certs(certificates.clone());
        }
        if let Some(username) = self.settings.username() {
            builder = builder.username(username);
        }
        if let Some(password) = self.settings.password() {
            builder = builder.password(password);
        }
        if self.settings.db() != 0 {
            builder = builder.database_id(self.settings.db());
        }
        builder.build()
    }

    /// One boot attempt: a seed proved a Cluster node with `CLUSTER SLOTS`,
    /// then the cluster connection, every primary answering a `PING`.
    ///
    /// The seed's proof runs on a connection of its own: `redis` reports a
    /// failed cluster connection by its text alone, so a refusal every attempt
    /// would repeat is read here, where its cause is still typed.
    async fn prove(
        &self,
        client: &ClusterClient,
    ) -> Result<ClusterConnection<ClusterNode>, Attempt<redis::RedisError>> {
        self.prove_a_seed().await?;
        let connection = client
            .get_async_generic_connection::<ClusterNode>()
            .await
            .map_err(Attempt::Failed)?;
        redis::cmd("PING")
            .query_async::<()>(&mut connection.clone())
            .await
            .map_err(|error| classify(error, &self.listed, self.settings.db()))?;
        Ok(connection)
    }

    /// The first seed that answers decides; one that cannot be reached lets
    /// the next decide.
    async fn prove_a_seed(&self) -> Result<(), Attempt<redis::RedisError>> {
        let mut last = None;
        for seed in &self.seeds {
            let endpoint = seed.to_string();
            let client = self.seed_client(seed).map_err(Attempt::Refused)?;
            let proof = async {
                let mut node = client
                    .get_multiplexed_async_connection_with_config(&connection_config(self.budget))
                    .await?;
                redis::cmd("CLUSTER")
                    .arg("SLOTS")
                    .query_async::<Value>(&mut node)
                    .await
            };
            match bounded(self.budget, proof)
                .await
                .and_then(std::convert::identity)
            {
                Ok(_) => return Ok(()),
                Err(error) if cluster_disabled(&error) => {
                    return Err(Attempt::Refused(RedisError::TopologyMismatch {
                        endpoint,
                        declared: RedisTopology::Cluster,
                        source: error,
                    }));
                }
                Err(error) => match classify(error, &endpoint, self.settings.db()) {
                    refused @ Attempt::Refused(_) => return Err(refused),
                    Attempt::Failed(error) => last = Some(error),
                },
            }
        }
        Err(Attempt::Failed(last.unwrap_or_else(|| {
            redis::RedisError::from((
                redis::ErrorKind::InvalidClientConfig,
                "the URL names no seed",
            ))
        })))
    }

    /// A client of `seed` alone, as every node is reached: its credentials,
    /// its database, its TLS.
    fn seed_client(&self, seed: &NodeAddr) -> Result<redis::Client, RedisError> {
        let endpoint = seed.to_string();
        let info = addr(seed, self.certificates.is_some())
            .into_connection_info()
            .map_err(|source| RedisError::InvalidUrl {
                endpoint: endpoint.clone(),
                source,
            })?
            .set_redis_settings(self.settings.clone())
            .set_tcp_settings(socket(self.budget));
        match &self.certificates {
            None => redis::Client::open(info)
                .map_err(|source| RedisError::InvalidUrl { endpoint, source }),
            Some(certificates) => redis::Client::build_with_tls(info, certificates.clone())
                .map_err(|source| RedisError::TlsRefused {
                    endpoint,
                    reason: tls::unusable_material(),
                    source: Some(source),
                }),
        }
    }
}

/// `node`'s address, encrypted or not.
fn addr(node: &NodeAddr, encrypted: bool) -> ConnectionAddr {
    if encrypted {
        ConnectionAddr::TcpTls {
            host: node.host.clone(),
            port: node.port,
            insecure: false,
            tls_params: None,
        }
    } else {
        ConnectionAddr::Tcp(node.host.clone(), node.port)
    }
}

/// Whether a server answered that its cluster support is off.
fn cluster_disabled(error: &redis::RedisError) -> bool {
    error
        .detail()
        .is_some_and(|detail| detail.contains("cluster support disabled"))
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// One Cluster node's connection, opened announcing no client library.
///
/// `redis` gives a Cluster no setting to skip `CLIENT SETINFO` on its nodes;
/// its generic connection is the one seam reaching each node's handshake.
#[derive(Clone)]
pub(crate) struct ClusterNode(MultiplexedConnection);

impl Connect for ClusterNode {
    fn connect_with_config<'a, T>(info: T, config: AsyncConnectionConfig) -> RedisFuture<'a, Self>
    where
        T: IntoConnectionInfo + Send + 'a,
    {
        Box::pin(async move {
            let info = info.into_connection_info()?;
            let endpoint = info.addr().to_string();
            let settings = info.redis_settings().clone().set_skip_set_lib_name();
            let opened = redis::Client::open(info.set_redis_settings(settings))?
                .get_multiplexed_async_connection_with_config(&config)
                .await;
            observe(&endpoint, &opened);
            opened.map(Self)
        })
    }
}

impl ConnectionLike for ClusterNode {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        self.0.req_packed_command(cmd)
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        pipeline: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        self.0.req_packed_commands(pipeline, offset, count)
    }

    fn get_db(&self) -> i64 {
        self.0.get_db()
    }
}

/// The nodes refusing TLS, each said once until a connection to it opens.
static REFUSING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Say a node's TLS refusal once, and forget it once the node opens again.
fn observe<T>(endpoint: &str, opened: &Result<T, redis::RedisError>) {
    let mut refusing = REFUSING.lock().unwrap_or_else(PoisonError::into_inner);
    match opened {
        Ok(_) => {
            refusing.remove(endpoint);
        }
        Err(error) if tls::negotiation_failed(error) => {
            if refusing.insert(endpoint.to_owned()) {
                tracing::warn!(
                    target: crate::TARGET,
                    endpoint = %endpoint,
                    reason = %tls::remedy(error),
                    error = %nest_rs_core::error_message(error),
                    "redis refused a reopened tls connection",
                );
            }
        }
        Err(_) => {}
    }
}
