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

use nest_rs_config::ClientTls;
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::cluster::{ClusterClient, ClusterClientBuilder};
use redis::cluster_async::{ClusterConnection, Connect};
use redis::cluster_routing::{
    MultipleNodeRoutingInfo, ResponsePolicy, Route, RoutingInfo, SingleNodeRoutingInfo, Slot,
    SlotAddr,
};
use redis::{
    AsyncConnectionConfig, Cmd, IntoConnectionInfo, Pipeline, RedisConnectionInfo, RedisFuture,
    ServerErrorKind, TlsMode, Value,
};

use crate::connection::{
    Attempt, FIRST_RETRY_BACKOFF, MAX_RETRY_BACKOFF, answered, classify, dial, node_addr,
    node_client, socket, within_budget,
};
use crate::error::RedisError;
use crate::script::RedisScript;
use crate::url::{ClusterUrl, NodeAddr, listed};
use crate::{RedisTopology, tls};

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
    /// `budget`: a seed says it serves a Cluster, and every primary answers a
    /// `PING`.
    pub(crate) async fn connect(
        url: ClusterUrl,
        tls: &ClientTls,
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
        let client = plan.client().map_err(|source| RedisError::InvalidUrl {
            endpoint: plan.listed.clone(),
            source,
        })?;
        let connection = within_budget(budget, &plan.listed, |_| plan.prove(&client))
            .await
            .map_err(|gave_up| gave_up.into_error(&plan.listed, budget))?;
        Ok(Arc::new(Self { plan, connection }))
    }

    /// A link of its own to the primary serving `key`'s slot, for a command
    /// that blocks on that key.
    pub(crate) async fn dedicated(&self, key: &str) -> Result<Arc<SlotLink>, redis::RedisError> {
        SlotLink::open(
            Arc::clone(&self.plan),
            self.connection.clone(),
            Slot::for_key(key),
        )
        .await
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
    /// A connection of its own to the node at `node`, with the URL's
    /// credentials, database and TLS.
    async fn dial(&self, node: &NodeAddr) -> Result<MultiplexedConnection, redis::RedisError> {
        let endpoint = node.to_string();
        let client = node_client(
            node,
            self.settings.clone(),
            self.certificates.as_ref(),
            self.budget,
        )
        .map_err(|refused| {
            redis::RedisError::from((
                redis::ErrorKind::InvalidClientConfig,
                "the Cluster names a node no client can reach",
                nest_rs_core::error_message(&refused),
            ))
        })?;
        let opened = dial(&client, self.budget).await;
        tls::observe_refusal(&endpoint, &opened);
        opened
    }

    /// The cluster client the link is opened from, each node's answer bounded
    /// by the budget. Its retries fit the budget, which [`answered`] holds
    /// every command to whatever they do.
    fn client(&self) -> Result<ClusterClient, redis::RedisError> {
        let seeds = self
            .seeds
            .iter()
            .map(|seed| node_addr(seed, self.certificates.is_some()).into_connection_info())
            .collect::<Result<Vec<_>, _>>()?;
        let mut builder = ClusterClientBuilder::new(seeds)
            .connection_timeout(self.budget)
            .response_timeout(self.budget)
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

    /// One boot attempt: a seed proved a Cluster node by its `HELLO`, then the
    /// cluster connection, every primary answering a `PING`.
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
                tls::hello(&client, &mut node, self.budget).await
            };
            match answered(self.budget, proof).await {
                Ok(hello) if hello.serves == RedisTopology::Cluster => return Ok(()),
                Ok(hello) => {
                    return Err(Attempt::Refused(RedisError::TopologyMismatch {
                        endpoint,
                        declared: RedisTopology::Cluster,
                        serves: hello.serves,
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

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The link to the primary serving one slot, for a command that blocks on a
/// key of it: one socket to that node alone, where the cluster connection would
/// hold one to every node.
///
/// **A redirection or a loss ends it rather than reopening it.** A `MOVED`, a
/// drop or a timeout fails the command, and the holder opens a link afresh,
/// which asks the Cluster again which node serves the slot. An `ASK` — the
/// slot being moved key by key — is followed for that command alone, as the
/// cluster connection follows it.
pub(crate) struct SlotLink {
    plan: Arc<Plan>,
    /// The cluster connection that names the slot's primary, and opens a link
    /// afresh.
    cluster: ClusterConnection<ClusterNode>,
    connection: MultiplexedConnection,
}

impl SlotLink {
    /// Open a connection to the primary the Cluster names for `slot` now.
    async fn open(
        plan: Arc<Plan>,
        mut cluster: ClusterConnection<ClusterNode>,
        slot: Slot,
    ) -> Result<Arc<Self>, redis::RedisError> {
        let routing = RoutingInfo::SingleNode(SingleNodeRoutingInfo::SpecificNode(
            Route::with_slot(slot, SlotAddr::Master),
        ));
        let slots = answered(
            plan.budget,
            cluster.route_command(redis::cmd("CLUSTER").arg("SLOTS").clone(), routing),
        )
        .await?;
        let primary = primary_of(&slots, slot).ok_or_else(|| {
            redis::RedisError::from((
                redis::ErrorKind::Server(ServerErrorKind::ClusterDown),
                "no primary serves the slot yet",
            ))
        })?;
        let connection = answered(plan.budget, plan.dial(&primary)).await?;
        Ok(Arc::new(Self {
            plan,
            cluster,
            connection,
        }))
    }

    /// A link of its own to the primary serving `key`'s slot.
    pub(crate) async fn dedicated(&self, key: &str) -> Result<Arc<Self>, redis::RedisError> {
        Self::open(
            Arc::clone(&self.plan),
            self.cluster.clone(),
            Slot::for_key(key),
        )
        .await
    }

    /// Send `cmd` to the slot's primary — or, while it answers `ASK`, to the
    /// node the slot's keys are moving to — answered or failed within
    /// `budget`.
    pub(crate) async fn send(
        &self,
        cmd: &Cmd,
        budget: Duration,
    ) -> Result<Value, redis::RedisError> {
        answered(budget, async {
            let reply = self.connection.clone().send_packed_command(cmd).await;
            match asked(&reply) {
                Some(target) => self.ask(&target, cmd).await,
                None => reply,
            }
        })
        .await
    }

    /// Send `count` replies' worth of `pipeline` from `offset` to the slot's
    /// primary within `budget`; an `ASK` reaches the caller.
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
                .send_packed_commands(pipeline, offset, count),
        )
        .await
    }

    /// The database the URL selects on every node.
    pub(crate) fn db(&self) -> i64 {
        self.plan.settings.db()
    }

    /// `cmd` sent once to `target` after `ASKING`, on a connection of its own.
    async fn ask(&self, target: &NodeAddr, cmd: &Cmd) -> Result<Value, redis::RedisError> {
        let mut connection = self.plan.dial(target).await?;
        let mut asking = redis::pipe();
        asking.cmd("ASKING").ignore().add_command(cmd.clone());
        let mut replies: Vec<Value> = asking.query_async(&mut connection).await?;
        replies.pop().ok_or_else(|| {
            redis::RedisError::from((
                redis::ErrorKind::UnexpectedReturnType,
                "the node a slot moves to answered nothing",
            ))
        })
    }
}

/// Where an `ASK` answer sends the command.
fn asked(reply: &Result<Value, redis::RedisError>) -> Option<NodeAddr> {
    let error = match reply {
        Ok(Value::ServerError(error)) => redis::RedisError::from(error.clone()),
        Err(error) if error.kind() == redis::ErrorKind::Server(ServerErrorKind::Ask) => {
            error.clone()
        }
        _ => return None,
    };
    if error.kind() != redis::ErrorKind::Server(ServerErrorKind::Ask) {
        return None;
    }
    let (addr, _) = error.redirect_node()?;
    let (host, port) = addr.rsplit_once(':')?;
    Some(NodeAddr {
        host: host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
        port: port.parse().ok()?,
    })
}

/// The primary `CLUSTER SLOTS` names for `slot`: each range is its first and
/// last slot, then its primary's host and port, then its replicas'.
fn primary_of(slots: &Value, slot: Slot) -> Option<NodeAddr> {
    let Value::Array(ranges) = slots else {
        return None;
    };
    ranges.iter().find_map(|range| {
        let Value::Array(range) = range else {
            return None;
        };
        let bound = |at: usize| match range.get(at) {
            Some(Value::Int(bound)) => u16::try_from(*bound).ok(),
            _ => None,
        };
        // `Slot` keeps its number to itself, so the range is matched slot by slot.
        if !(bound(0)?..=bound(1)?)
            .filter_map(Slot::new)
            .any(|served| served == slot)
        {
            return None;
        }
        let Some(Value::Array(primary)) = range.get(2) else {
            return None;
        };
        let host = match primary.first()? {
            Value::BulkString(host) => String::from_utf8(host.clone()).ok()?,
            _ => return None,
        };
        let port = match primary.get(1)? {
            Value::Int(port) => u16::try_from(*port).ok()?,
            _ => return None,
        };
        Some(NodeAddr { host, port })
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(text: &str) -> Value {
        Value::BulkString(text.as_bytes().to_vec())
    }

    fn range(first: i64, last: i64, primary: (&str, i64), replica: (&str, i64)) -> Value {
        Value::Array(vec![
            Value::Int(first),
            Value::Int(last),
            Value::Array(vec![
                bulk(primary.0),
                Value::Int(primary.1),
                bulk("primary-id"),
            ]),
            Value::Array(vec![
                bulk(replica.0),
                Value::Int(replica.1),
                bulk("replica-id"),
            ]),
        ])
    }

    #[test]
    fn the_primary_of_a_slot_is_read_off_the_range_holding_it() {
        let slots = Value::Array(vec![
            range(0, 5460, ("node-a", 7000), ("node-d", 7003)),
            range(5461, 10922, ("node-b", 7001), ("node-e", 7004)),
            range(10923, 16383, ("node-c", 7002), ("node-f", 7005)),
        ]);
        for (key, primary) in [
            ("{video}", "node-a:7000"),
            ("{audio}", "node-b:7001"),
            ("{queue}", "node-c:7002"),
        ] {
            let slot = Slot::for_key(key);
            assert_eq!(
                primary_of(&slots, slot)
                    .map(|node| node.to_string())
                    .as_deref(),
                Some(primary),
                "{key}",
            );
        }
    }

    #[test]
    fn a_slot_no_range_holds_has_no_primary() {
        let slots = Value::Array(vec![range(0, 10, ("node-a", 7000), ("node-d", 7003))]);
        assert_eq!(primary_of(&slots, Slot::for_key("{audio}")), None);
        assert_eq!(primary_of(&Value::Nil, Slot::for_key("{audio}")), None);
    }

    #[test]
    fn an_ask_names_the_node_the_slot_moves_to_and_nothing_else_does() {
        let ask = redis::RedisError::from((
            redis::ErrorKind::Server(ServerErrorKind::Ask),
            "An error was signalled by the server",
            "3999 node-b:7001".to_owned(),
        ));
        assert_eq!(
            asked(&Err(ask)).map(|node| node.to_string()).as_deref(),
            Some("node-b:7001")
        );
        let moved = redis::RedisError::from((
            redis::ErrorKind::Server(ServerErrorKind::Moved),
            "An error was signalled by the server",
            "3999 node-b:7001".to_owned(),
        ));
        assert_eq!(asked(&Err(moved)), None);
        assert_eq!(asked(&Ok(Value::Okay)), None);
    }
}
