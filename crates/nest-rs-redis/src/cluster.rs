//! [`ClusterLink`] — the link to a Cluster's nodes: `redis`'s cluster
//! connection, which reads the slots from the seeds the URL names, sends each
//! command to the node serving its keys, follows `MOVED` and `ASK`, and reads
//! the slots again once a failover moved them.
//!
//! Each node is reached through [`ClusterNode`], so a node's connection
//! announces no client library — `CLIENT SETINFO`, which an ACL user confined to
//! a binding's commands is refused — and a node refusing a TLS connection is
//! said once.

use std::sync::Arc;
use std::time::Duration;

use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::cluster::{ClusterClient, ClusterClientBuilder};
use redis::cluster_async::{ClusterConnection, Connect};
use redis::cluster_routing::{
    MultipleNodeRoutingInfo, ResponsePolicy, Route, RoutingInfo, SingleNodeRoutingInfo, Slot,
    SlotAddr,
};
use redis::{
    AsyncConnectionConfig, Cmd, IntoConnectionInfo, Pipeline, RedisConnectionInfo, RedisFuture,
    TlsMode, Value,
};

use crate::connection::{
    Attempt, FIRST_RETRY_BACKOFF, MAX_RETRY_BACKOFF, answered, classify, dial, node_addr,
    node_client, socket, within_budget,
};
use crate::error::RedisError;
use crate::script::RedisScript;
use crate::url::{ClusterUrl, NodeAddr, listed};
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
        let listed = listed(&url.seeds);
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
            .map_err(|gave_up| gave_up.into_error(&plan.listed, budget))?;
        Ok(Arc::new(Self { plan, connection }))
    }

    /// A link of its own to the same Cluster, for a command that blocks: each
    /// node's answer may take `response`.
    pub(crate) async fn dedicated(
        &self,
        response: Duration,
    ) -> Result<Arc<Self>, redis::RedisError> {
        let client = self.plan.client(response)?;
        let connection = answered(
            self.plan.budget,
            client.get_async_generic_connection::<ClusterNode>(),
        )
        .await?;
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
        answered(budget, self.connection.clone().req_packed_command(cmd)).await
    }

    /// Send `count` replies' worth of `pipeline` from `offset` within `budget`.
    pub(crate) async fn send_pipeline(
        &self,
        pipeline: &Pipeline,
        offset: usize,
        count: usize,
        budget: Duration,
    ) -> Result<Vec<Value>, redis::RedisError> {
        answered(
            budget,
            self.connection
                .clone()
                .req_packed_commands(pipeline, offset, count),
        )
        .await
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
        answered(
            budget,
            self.connection
                .clone()
                .route_command(script.load_cmd(), routing),
        )
        .await
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
    /// [`answered`] holds every command to whatever they do.
    fn client(&self, response: Duration) -> Result<ClusterClient, redis::RedisError> {
        let seeds = self
            .seeds
            .iter()
            .map(|seed| node_addr(seed, self.certificates.is_some()).into_connection_info())
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
            let client = node_client(
                seed,
                self.settings.clone(),
                self.certificates.as_ref(),
                self.budget,
            )
            .map_err(Attempt::Refused)?;
            let proof = async {
                let mut node = dial(&client, self.budget).await?;
                redis::cmd("CLUSTER")
                    .arg("SLOTS")
                    .query_async::<Value>(&mut node)
                    .await
            };
            match answered(self.budget, proof).await {
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
            tls::observe_refusal(&endpoint, &opened);
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
