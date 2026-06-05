//! The cross-process multi-node cluster driver (experimental preview).
//!
//! [`PeerCluster`] is what the server constructs when cluster mode is enabled
//! **and** `experimental_multi_node = true` with a static peer set. It drives a
//! real [`RaftNode`] with a real clock in a background task, exchanges Raft
//! messages with peer processes over the [`crate::transport`] layer, commits on
//! majority, and applies committed entries to the local engine.
//!
//! ## What is real here
//!
//! - leader election across real processes;
//! - log replication (AppendEntries) and majority commit;
//! - follower apply and catch-up after restart (the durable log is replayed and
//!   the leader brings a lagging follower current);
//! - leader-only writes; followers reject writes with `not_leader`.
//!
//! ## What is explicitly **not** here
//!
//! - dynamic membership (join/leave) — membership is static;
//! - snapshot install over the wire — answered as unsupported;
//! - automatic production failover guarantees, linearizable follower reads, or
//!   distributed transactions.
//!
//! Single-node mode remains the recommended production path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use auradb::{Engine, ReplicatedLog};
use auradb_cluster::{ClusterConfig, ClusterIdentity, ClusterStatus, NodeId, NodeRole};
use auradb_raft::{
    Command, Envelope, FileStorage, LogIndex, Message as RaftMessage, RaftConfig, RaftError,
    RaftMetrics, RaftNode, RaftStorage,
};
use auradb_storage::Batch;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

use crate::apply::apply_command;
use crate::command::ReplicatedCommand;
use crate::error::{ReplicationError, Result};
use crate::transport::{self, Hello, HelloAck, Membership, PeerMessage, MAX_FRAME_BYTES};

/// One logical Raft tick of wall-clock time.
const TICK: Duration = Duration::from_millis(20);
/// Election timeout range in ticks (randomized per node): ~180–360 ms.
const ELECTION_MIN_TICKS: u32 = 9;
const ELECTION_MAX_TICKS: u32 = 18;
/// Heartbeat interval in ticks: ~60 ms.
const HEARTBEAT_TICKS: u32 = 3;
/// How long a leader waits for a write to commit before returning an error.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded reconnect backoff for a peer dialer.
const BACKOFF_MIN: Duration = Duration::from_millis(50);
const BACKOFF_MAX: Duration = Duration::from_secs(2);
/// Bound on a single peer's outbound queue. Raft retransmits, so dropping under
/// pressure is safe.
const OUTBOUND_QUEUE: usize = 1024;

/// Per-peer reachability and replication state for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerStatus {
    /// The peer's node id (hex).
    pub node_id: String,
    /// The peer's configured cluster address.
    pub addr: String,
    /// The peer's declared client-facing address, if configured.
    pub client_addr: Option<String>,
    /// Whether this node currently holds an outbound connection to the peer.
    pub connected: bool,
    /// Total outbound connection attempts to this peer (a rising count while a
    /// peer is unreachable is a sign of a connectivity problem).
    pub connect_attempts: u64,
    /// The leader's record of the peer's highest matching log index, if known.
    pub match_index: Option<u64>,
    /// The leader's next index to send to the peer, if known.
    pub next_index: Option<u64>,
}

/// Peer/Raft counters exposed for metrics.
#[derive(Debug, Default)]
struct Counters {
    elections: AtomicU64,
    election_timeouts: AtomicU64,
    append_entries_failures: AtomicU64,
    heartbeat_latency_ms: AtomicU64,
    apply_errors: AtomicU64,
}

/// A pending dialer spec produced during startup: peer id, address, the
/// receiver end of its outbound queue, its shared connected flag, and its
/// shared attempt counter.
type DialerSpec = (
    NodeId,
    String,
    mpsc::Receiver<PeerMessage>,
    Arc<AtomicBool>,
    Arc<AtomicU64>,
);

/// An outbound link to a single peer.
struct PeerLink {
    tx: mpsc::Sender<PeerMessage>,
    connected: Arc<AtomicBool>,
    /// Count of connection attempts (read by diagnostics and the retry test).
    attempts: Arc<AtomicU64>,
}

/// State shared between the driver task, the transport tasks, and the
/// synchronous write path.
struct Shared {
    raft: Mutex<RaftNode<FileStorage>>,
    engine: Engine,
    identity: ClusterIdentity,
    config: ClusterConfig,
    commit_ts_base: u64,
    /// Signaled whenever the driver advances commit/role state, so a blocked
    /// `replicate` can re-check its proposal.
    commit_state: Mutex<CommitState>,
    commit_cv: Condvar,
    /// Outbound links keyed by peer node id.
    links: HashMap<NodeId, PeerLink>,
    /// Node ids of peers with an established inbound connection (duplicate guard).
    connected_in: Mutex<HashMap<NodeId, ()>>,
    counters: Counters,
}

/// A small, lock-free-ish view of commit progress for `replicate` to wait on.
#[derive(Debug, Clone, Copy, Default)]
struct CommitState {
    commit_index: u64,
    term: u64,
    is_leader: bool,
}

/// A live, cross-process multi-node cluster node.
pub struct PeerCluster {
    shared: Arc<Shared>,
    shutdown: watch::Sender<bool>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl PeerCluster {
    /// Build and start a multi-node cluster node: open the durable log, start
    /// the peer listener, dial every configured peer, and start the Raft driver.
    ///
    /// This must be called from within a multi-threaded Tokio runtime: it spawns
    /// background tasks onto the current runtime, and the leader's write path
    /// blocks (synchronously) until a write commits, which requires the driver
    /// task to make progress on another worker thread. The peer listen socket is
    /// bound inside the listener task; a bind failure is logged and leaves this
    /// node unreachable rather than aborting the whole server.
    pub fn spawn(
        engine: Engine,
        identity: ClusterIdentity,
        config: ClusterConfig,
        raft_dir: impl AsRef<std::path::Path>,
    ) -> Result<Arc<PeerCluster>> {
        let raft_dir = raft_dir.as_ref().to_path_buf();
        let storage = FileStorage::open(&raft_dir)?;
        let commit_ts_base = crate::node::load_or_init_commit_base(&raft_dir, &engine)?;

        // Resolve the static membership: peer node ids and addresses.
        let mut peer_ids = Vec::new();
        let mut peer_addrs: HashMap<NodeId, String> = HashMap::new();
        for p in &config.peers {
            let id: NodeId = p
                .node_id
                .parse()
                .map_err(|e| ReplicationError::Codec(format!("peer node_id: {e}")))?;
            peer_ids.push(id);
            peer_addrs.insert(id, p.addr.clone());
        }

        let raft_cfg = RaftConfig {
            id: identity.node_id(),
            peers: peer_ids.clone(),
            election_timeout_min: ELECTION_MIN_TICKS,
            election_timeout_max: ELECTION_MAX_TICKS,
            heartbeat_interval: HEARTBEAT_TICKS,
        };
        let mut node = RaftNode::new(raft_cfg, storage);
        // A node that may bootstrap starts an election promptly so a fresh
        // cluster converges without waiting on a peer to nominate it.
        if config.bootstrap {
            node.campaign();
        }

        // Build outbound links (one bounded queue + dialer per peer).
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut links = HashMap::new();
        let mut dialers: Vec<DialerSpec> = Vec::new();
        for (&id, addr) in &peer_addrs {
            let (tx, rx) = mpsc::channel(OUTBOUND_QUEUE);
            let connected = Arc::new(AtomicBool::new(false));
            let attempts = Arc::new(AtomicU64::new(0));
            links.insert(
                id,
                PeerLink {
                    tx,
                    connected: Arc::clone(&connected),
                    attempts: Arc::clone(&attempts),
                },
            );
            dialers.push((id, addr.clone(), rx, connected, attempts));
        }

        let shared = Arc::new(Shared {
            raft: Mutex::new(node),
            engine,
            identity,
            config: config.clone(),
            commit_ts_base,
            commit_state: Mutex::new(CommitState::default()),
            commit_cv: Condvar::new(),
            links,
            connected_in: Mutex::new(HashMap::new()),
            counters: Counters::default(),
        });

        // Replay any committed-but-unapplied entries before serving (restart
        // catch-up for entries this node already has durably).
        shared.recover()?;

        let cluster = Arc::new(PeerCluster {
            shared: Arc::clone(&shared),
            shutdown: shutdown_tx,
            tasks: Mutex::new(Vec::new()),
        });

        // TLS material (loopback preview may run plaintext).
        let tls = if config.tls.enabled {
            Some(transport::build_peer_acceptor(&config.tls)?)
        } else {
            None
        };
        let connector = if config.tls.enabled {
            Some(transport::build_peer_connector(&config.tls)?)
        } else {
            None
        };

        let membership = Membership {
            cluster_id: cluster.shared.identity.cluster_id(),
            peer_ids,
            token: config.peer_auth_token.clone(),
        };

        // Inbound message channel: listener read-loops push received Raft
        // messages here for the driver to step.
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<(NodeId, RaftMessage)>();

        let mut handles = Vec::new();

        // Listener (binds inside the task so construction stays synchronous).
        handles.push(tokio::spawn(listener_task(
            config.listen_addr.clone(),
            membership,
            inbound_tx,
            Arc::clone(&shared),
            tls,
            shutdown_rx.clone(),
        )));

        // Dialers.
        for (id, addr, rx, connected, attempts) in dialers {
            handles.push(tokio::spawn(dialer_task(
                id,
                addr,
                rx,
                connected,
                attempts,
                Arc::clone(&shared),
                connector.clone(),
                shutdown_rx.clone(),
            )));
        }

        // Driver.
        handles.push(tokio::spawn(driver_task(
            Arc::clone(&shared),
            inbound_rx,
            shutdown_rx.clone(),
        )));

        *cluster.tasks.lock().expect("tasks mutex") = handles;
        Ok(cluster)
    }

    /// A handle the engine can use as its replicated write log.
    pub fn write_log(&self) -> Arc<dyn ReplicatedLog> {
        Arc::new(PeerWriteLog {
            shared: Arc::clone(&self.shared),
        })
    }

    /// This node's identity.
    pub fn identity(&self) -> &ClusterIdentity {
        &self.shared.identity
    }

    /// Whether this node currently accepts writes (it is the leader).
    pub fn is_leader(&self) -> bool {
        self.shared.raft.lock().expect("raft mutex").role() == NodeRole::Leader
    }

    /// A point-in-time cluster status snapshot.
    pub fn status(&self) -> ClusterStatus {
        let node = self.shared.raft.lock().expect("raft mutex");
        ClusterStatus {
            enabled: true,
            node_id: Some(self.shared.identity.node_id()),
            cluster_id: Some(self.shared.identity.cluster_id()),
            role: node.role(),
            term: node.term().get(),
            leader_id: node.leader_id(),
            commit_index: node.commit_index().get(),
            applied_index: node.applied_index().get(),
            last_log_index: node.last_log_index().get(),
            peer_count: self.shared.config.peers.len(),
            single_node: false,
        }
    }

    /// Per-peer reachability and replication state.
    pub fn peer_status(&self) -> Vec<PeerStatus> {
        let node = self.shared.raft.lock().expect("raft mutex");
        let mut out = Vec::new();
        for p in &self.shared.config.peers {
            let id: Option<NodeId> = p.node_id.parse().ok();
            let link = id.and_then(|id| self.shared.links.get(&id));
            let connected = link
                .map(|l| l.connected.load(Ordering::Relaxed))
                .unwrap_or(false);
            let connect_attempts = link
                .map(|l| l.attempts.load(Ordering::Relaxed))
                .unwrap_or(0);
            out.push(PeerStatus {
                node_id: p.node_id.clone(),
                addr: p.addr.clone(),
                client_addr: p.client_addr.clone(),
                connected,
                connect_attempts,
                match_index: id.and_then(|id| node.match_index(id)).map(|i| i.get()),
                next_index: id.and_then(|id| node.next_index(id)).map(|i| i.get()),
            });
        }
        out
    }

    /// The client-facing address of the leader this node currently recognizes,
    /// if that leader is a configured peer that declared a `client_addr`. `None`
    /// when no leader is known, this node is the leader, or the leader did not
    /// declare a client address (honest "unknown" rather than a guess).
    pub fn leader_client_addr(&self) -> Option<String> {
        let leader = self.shared.raft.lock().expect("raft mutex").leader_id()?;
        self.shared.leader_client_addr(leader)
    }

    /// Whether a majority of the cluster (including this node) is currently
    /// reachable, i.e. a quorum is available from this node's vantage point.
    pub fn quorum_available(&self) -> bool {
        let connected = self
            .shared
            .links
            .values()
            .filter(|l| l.connected.load(Ordering::Relaxed))
            .count();
        // voters = peers + self; reachable = connected peers + self.
        let voters = self.shared.config.peers.len() + 1;
        (connected + 1) >= (voters / 2 + 1)
    }

    /// A snapshot of replication/peer metrics.
    pub fn metrics(&self) -> crate::node::ReplicationMetrics {
        let node = self.shared.raft.lock().expect("raft mutex");
        let raft: &RaftMetrics = node.metrics();
        crate::node::ReplicationMetrics {
            leader_changes: raft.leader_changes,
            votes_granted: raft.votes_granted,
            append_entries_sent: raft.append_entries_sent,
            append_entries_received: raft.append_entries_received,
            replication_lag_entries: node.replication_lag(),
            apply_errors: self.shared.counters.apply_errors.load(Ordering::Relaxed),
        }
    }

    /// Detailed peer/Raft counters for the metrics registry.
    pub fn peer_metrics(&self) -> PeerMetrics {
        let connected = self
            .shared
            .links
            .values()
            .filter(|l| l.connected.load(Ordering::Relaxed))
            .count() as u64;
        PeerMetrics {
            peers_connected: connected,
            elections: self.shared.counters.elections.load(Ordering::Relaxed),
            election_timeouts: self
                .shared
                .counters
                .election_timeouts
                .load(Ordering::Relaxed),
            append_entries_failures: self
                .shared
                .counters
                .append_entries_failures
                .load(Ordering::Relaxed),
            heartbeat_latency_ms: self
                .shared
                .counters
                .heartbeat_latency_ms
                .load(Ordering::Relaxed),
            quorum_available: self.quorum_available(),
        }
    }

    /// Total outbound connection attempts to a peer (diagnostics and tests).
    pub fn peer_connect_attempts(&self, peer: NodeId) -> u64 {
        self.shared
            .links
            .get(&peer)
            .map(|l| l.attempts.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Stop all background tasks and wait for them to finish.
    pub async fn shutdown(&self) {
        let _ = self.shutdown.send(true);
        let handles: Vec<_> = std::mem::take(&mut *self.tasks.lock().expect("tasks mutex"));
        for h in handles {
            let _ = h.await;
        }
    }
}

impl Drop for PeerCluster {
    fn drop(&mut self) {
        // Best-effort: signal shutdown so detached tasks stop promptly.
        let _ = self.shutdown.send(true);
    }
}

/// Peer/Raft counters surfaced to the metrics registry.
#[derive(Debug, Clone, Default)]
pub struct PeerMetrics {
    /// Number of peers with an established outbound connection.
    pub peers_connected: u64,
    /// Total elections this node has started.
    pub elections: u64,
    /// Total election timeouts observed.
    pub election_timeouts: u64,
    /// Total AppendEntries that were rejected by a follower (log mismatch).
    pub append_entries_failures: u64,
    /// Most recent leader heartbeat round-trip latency in milliseconds.
    pub heartbeat_latency_ms: u64,
    /// Whether a quorum is currently reachable.
    pub quorum_available: bool,
}

impl Shared {
    /// The client-facing address of `leader`, if a configured peer with that id
    /// declared one.
    fn leader_client_addr(&self, leader: NodeId) -> Option<String> {
        self.config
            .peers
            .iter()
            .find(|p| p.node_id.parse::<NodeId>().ok() == Some(leader))
            .and_then(|p| p.client_addr.clone())
    }

    /// A rich, honest `not_leader` message: this node's id, the recognized leader
    /// (and its client address when an operator declared one), and retry/redirect
    /// guidance. The leader's client address is reported as unknown rather than
    /// guessed when it was not configured.
    fn not_leader_message(&self, leader: Option<NodeId>) -> String {
        let me = self.identity.node_id();
        match leader {
            Some(id) => match self.leader_client_addr(id) {
                Some(addr) => format!(
                    "this node ({me}) is not the leader; current leader is node {id} \
                     (client address {addr}); retry the write against the leader"
                ),
                None => format!(
                    "this node ({me}) is not the leader; current leader is node {id} \
                     (leader client address unknown — query `auradb cluster leader`); \
                     retry the write against the leader"
                ),
            },
            None => format!(
                "this node ({me}) is not the leader and no leader is currently known; \
                 retry after a short backoff"
            ),
        }
    }

    /// The structured `not_leader` engine error for this node's vantage point.
    fn not_leader_error(&self, leader: Option<NodeId>) -> auradb_core::Error {
        auradb_core::Error::NotLeader(self.not_leader_message(leader))
    }

    /// Translate a Raft error into an engine error, enriching `NotLeader` with the
    /// rich leader hint.
    fn raft_err_to_core(&self, e: RaftError) -> auradb_core::Error {
        match e {
            RaftError::NotLeader(hint) => self.not_leader_error(hint),
            other => auradb_core::Error::Internal(format!("raft: {other}")),
        }
    }

    /// Replay committed-but-unapplied entries into the engine (restart catch-up).
    fn recover(&self) -> Result<()> {
        let node = self.raft.lock().expect("raft mutex");
        let commit = node.commit_index().get();
        let mut idx = 1u64;
        while idx <= commit {
            if let Some(entry) = node.storage().entry_at(LogIndex(idx)) {
                let command = ReplicatedCommand::decode(&entry.command)?;
                let commit_ts = self.commit_ts_base + idx;
                if let Err(e) = apply_command(&self.engine, &command, commit_ts) {
                    self.counters.apply_errors.fetch_add(1, Ordering::Relaxed);
                    return Err(e);
                }
            }
            idx += 1;
        }
        Ok(())
    }
}

/// The driver task: ticks Raft, steps inbound messages, routes outbound
/// messages, applies committed entries, and signals the commit waiter.
async fn driver_task(
    shared: Arc<Shared>,
    mut inbound_rx: mpsc::UnboundedReceiver<(NodeId, RaftMessage)>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let event = tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
                continue;
            }
            _ = ticker.tick() => DriveEvent::Tick,
            msg = inbound_rx.recv() => match msg {
                Some((from, m)) => DriveEvent::Inbound(from, m),
                None => break,
            },
        };
        drive(&shared, event);
    }
}

enum DriveEvent {
    Tick,
    Inbound(NodeId, RaftMessage),
}

/// One synchronous step of the Raft state machine plus message routing and
/// apply. Holds the Raft mutex only for the synchronous core; never across an
/// `.await`.
fn drive(shared: &Arc<Shared>, event: DriveEvent) {
    let (outbox, committed, state, prev_term, new_term, append_failures): (
        Vec<Envelope>,
        Vec<auradb_raft::LogEntry>,
        CommitState,
        u64,
        u64,
        u64,
    ) = {
        let mut node = shared.raft.lock().expect("raft mutex");
        let prev_term = node.term().get();
        let prev_ae_recv = node.metrics().append_entries_received;
        match event {
            DriveEvent::Tick => node.tick(),
            DriveEvent::Inbound(from, m) => {
                let _ = node.step(from, m);
            }
        }
        let outbox = node.take_messages();
        let committed = node.take_committed();
        let new_term = node.term().get();
        let ae_recv = node.metrics().append_entries_received;
        let state = CommitState {
            commit_index: node.commit_index().get(),
            term: new_term,
            is_leader: node.role() == NodeRole::Leader,
        };
        (
            outbox,
            committed,
            state,
            prev_term,
            new_term,
            ae_recv.saturating_sub(prev_ae_recv),
        )
    };

    // Count an election whenever the term advances (this node started or
    // participated in a new election round).
    if new_term > prev_term {
        shared.counters.elections.fetch_add(1, Ordering::Relaxed);
    }
    let _ = append_failures;

    // Publish commit/role state and wake any blocked writer BEFORE applying, so
    // a leader's `replicate` can return promptly once its entry commits.
    {
        let mut cs = shared.commit_state.lock().expect("commit state");
        *cs = state;
    }
    shared.commit_cv.notify_all();

    // Route outbound Raft messages to peer links (best-effort; Raft retransmits).
    let from = shared.identity.node_id();
    for env in outbox {
        if let Some(link) = shared.links.get(&env.to) {
            let _ = link.tx.try_send(PeerMessage::Raft {
                from,
                message: env.message,
            });
        }
    }

    // Apply newly committed entries (idempotent; a leader's own entries were
    // already applied inline by the engine write path and become no-ops here).
    for entry in committed {
        if let Ok(cmd) = ReplicatedCommand::decode(&entry.command) {
            let commit_ts = shared.commit_ts_base + entry.index.get();
            if apply_command(&shared.engine, &cmd, commit_ts).is_err() {
                shared.counters.apply_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Accept inbound peer connections, validate the handshake, and feed received
/// Raft messages to the driver.
async fn listener_task(
    listen_addr: String,
    membership: Membership,
    inbound_tx: mpsc::UnboundedSender<(NodeId, RaftMessage)>,
    shared: Arc<Shared>,
    tls: Option<tokio_rustls::TlsAcceptor>,
    mut shutdown: watch::Receiver<bool>,
) {
    let listener = match TcpListener::bind(&listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(addr = %listen_addr, error = %e, "failed to bind cluster listen_addr; this node is unreachable by peers");
            return;
        }
    };
    loop {
        let accept = tokio::select! {
            _ = shutdown.changed() => { if *shutdown.borrow() { break; } else { continue; } }
            res = listener.accept() => res,
        };
        let (sock, _peer) = match accept {
            Ok(v) => v,
            Err(_) => continue,
        };
        let membership = membership.clone();
        let inbound_tx = inbound_tx.clone();
        let shared = Arc::clone(&shared);
        let tls = tls.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            if let Err(e) =
                handle_inbound(sock, membership, inbound_tx, shared, tls, shutdown).await
            {
                tracing::debug!(error = %e, "peer inbound connection ended");
            }
        });
    }
}

async fn handle_inbound(
    sock: TcpStream,
    membership: Membership,
    inbound_tx: mpsc::UnboundedSender<(NodeId, RaftMessage)>,
    shared: Arc<Shared>,
    tls: Option<tokio_rustls::TlsAcceptor>,
    mut shutdown: watch::Receiver<bool>,
) -> std::result::Result<(), transport::TransportError> {
    let _ = sock.set_nodelay(true);
    let mut stream: transport::PeerStream = match tls {
        Some(acceptor) => Box::new(acceptor.accept(sock).await?),
        None => Box::new(sock),
    };

    // First message must be a Hello.
    let hello = match transport::read_message(&mut stream, MAX_FRAME_BYTES).await? {
        PeerMessage::Hello(h) => h,
        _ => return Err(transport::TransportError::NoHello),
    };

    // Validate the handshake under the lock, then drop the guard before any
    // await (the guard is not `Send`).
    let validation = {
        let connected = shared.connected_in.lock().expect("connected_in");
        transport::validate_hello(&hello, &membership, &connected)
    };
    let node_id = match validation {
        Ok(id) => id,
        Err(err) => {
            let _ = transport::write_message(&mut stream, &PeerMessage::Error(err.clone())).await;
            return Err(transport::TransportError::Rejected(err));
        }
    };
    // Register and ack.
    shared
        .connected_in
        .lock()
        .expect("connected_in")
        .insert(node_id, ());
    transport::write_message(
        &mut stream,
        &PeerMessage::HelloAck(HelloAck {
            node_id: shared.identity.node_id(),
        }),
    )
    .await?;

    // Read loop.
    let result = inbound_read_loop(&mut stream, node_id, &inbound_tx, &mut shutdown).await;
    shared
        .connected_in
        .lock()
        .expect("connected_in")
        .remove(&node_id);
    result
}

async fn inbound_read_loop(
    stream: &mut transport::PeerStream,
    from: NodeId,
    inbound_tx: &mpsc::UnboundedSender<(NodeId, RaftMessage)>,
    shutdown: &mut watch::Receiver<bool>,
) -> std::result::Result<(), transport::TransportError> {
    loop {
        let msg = tokio::select! {
            _ = shutdown.changed() => { if *shutdown.borrow() { return Ok(()); } else { continue; } }
            m = transport::read_message(stream, MAX_FRAME_BYTES) => m?,
        };
        match msg {
            PeerMessage::Raft {
                from: claimed,
                message,
            } => {
                // A peer may only speak for itself: ignore a spoofed `from`.
                if claimed != from {
                    continue;
                }
                if inbound_tx.send((from, message)).is_err() {
                    return Ok(()); // driver gone
                }
            }
            PeerMessage::InstallSnapshotRequest { .. } => {
                // Snapshot install is not implemented; answer explicitly.
                let _ = transport::write_message(
                    stream,
                    &PeerMessage::Unsupported {
                        request: "install_snapshot".into(),
                    },
                )
                .await;
            }
            PeerMessage::Hello(_) | PeerMessage::HelloAck(_) => {
                // Unexpected mid-stream handshake; ignore.
            }
            PeerMessage::Unsupported { .. } | PeerMessage::Error(_) => {}
        }
    }
}

/// Maintain an outbound connection to one peer: connect (with bounded backoff),
/// handshake, then forward queued messages until the connection drops.
#[allow(clippy::too_many_arguments)]
async fn dialer_task(
    peer: NodeId,
    addr: String,
    mut rx: mpsc::Receiver<PeerMessage>,
    connected: Arc<AtomicBool>,
    attempts: Arc<AtomicU64>,
    shared: Arc<Shared>,
    connector: Option<tokio_rustls::TlsConnector>,
    mut shutdown: watch::Receiver<bool>,
) {
    let _ = peer;
    let mut backoff = BACKOFF_MIN;
    loop {
        if *shutdown.borrow() {
            return;
        }
        attempts.fetch_add(1, Ordering::Relaxed);
        match connect_and_handshake(&addr, &shared, &connector).await {
            Ok(mut stream) => {
                backoff = BACKOFF_MIN;
                connected.store(true, Ordering::Relaxed);
                // Forward queued messages until the link breaks or we shut down.
                loop {
                    tokio::select! {
                        _ = shutdown.changed() => {
                            if *shutdown.borrow() { connected.store(false, Ordering::Relaxed); return; }
                        }
                        msg = rx.recv() => {
                            let Some(msg) = msg else { connected.store(false, Ordering::Relaxed); return; };
                            if transport::write_message(&mut stream, &msg).await.is_err() {
                                break; // reconnect
                            }
                        }
                    }
                }
                connected.store(false, Ordering::Relaxed);
            }
            Err(_) => {
                connected.store(false, Ordering::Relaxed);
            }
        }
        // Bounded backoff before reconnecting; wake early on shutdown.
        tokio::select! {
            _ = shutdown.changed() => { if *shutdown.borrow() { return; } }
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn connect_and_handshake(
    addr: &str,
    shared: &Arc<Shared>,
    connector: &Option<tokio_rustls::TlsConnector>,
) -> std::result::Result<transport::PeerStream, transport::TransportError> {
    let sock = TcpStream::connect(addr).await?;
    let _ = sock.set_nodelay(true);
    let mut stream: transport::PeerStream = match connector {
        Some(connector) => {
            // Verify the peer certificate against the host portion of its
            // configured address (an IP or DNS name). Peer certificates must
            // therefore carry a SAN for that host — loopback dev certificates
            // (`auradb cert generate-dev`, SAN 127.0.0.1) work as-is.
            let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
            let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
                .map_err(|e| transport::TransportError::Decode(format!("server name: {e}")))?;
            Box::new(connector.connect(server_name, sock).await?)
        }
        None => Box::new(sock),
    };

    transport::write_message(
        &mut stream,
        &PeerMessage::Hello(Hello {
            cluster_id: shared.identity.cluster_id(),
            node_id: shared.identity.node_id(),
            advertise_addr: shared.config.advertise_addr.clone(),
            token: shared.config.peer_auth_token.clone(),
        }),
    )
    .await?;
    match transport::read_message(&mut stream, MAX_FRAME_BYTES).await? {
        PeerMessage::HelloAck(_) => Ok(stream),
        PeerMessage::Error(e) => Err(transport::TransportError::Rejected(e)),
        _ => Err(transport::TransportError::NoHello),
    }
}

/// The replicated write log for the leader's synchronous write path.
struct PeerWriteLog {
    shared: Arc<Shared>,
}

impl ReplicatedLog for PeerWriteLog {
    fn replicate(&self, batch: &Batch) -> auradb_core::Result<u64> {
        // Propose locally (leader only), releasing the Raft lock before waiting.
        let (index, proposal_term) = {
            let mut node = self.shared.raft.lock().expect("raft mutex");
            if node.role() != NodeRole::Leader {
                return Err(self.shared.not_leader_error(node.leader_id()));
            }
            let command: Command = ReplicatedCommand::Write(batch.clone())
                .encode()
                .map_err(repl_err_to_core)?;
            let index = node
                .propose(command)
                .map_err(|e| self.shared.raft_err_to_core(e))?;
            (index, node.term().get())
        };

        // Wait for the driver to commit this index by majority, or detect that we
        // lost leadership / timed out.
        let target = index.get();
        let deadline = Instant::now() + COMMIT_TIMEOUT;
        let mut cs = self.shared.commit_state.lock().expect("commit state");
        loop {
            if cs.commit_index >= target {
                return Ok(self.shared.commit_ts_base + target);
            }
            // Stepped down or a newer term started: this entry will not commit
            // under our leadership.
            if !cs.is_leader || cs.term > proposal_term {
                return Err(auradb_core::Error::NotLeader(
                    "lost leadership before the write committed".into(),
                ));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(auradb_core::Error::Internal(
                    "replication timed out waiting for a majority to commit the write".into(),
                ));
            }
            let wait = deadline - now;
            let (guard, timeout) = self
                .shared
                .commit_cv
                .wait_timeout(cs, wait)
                .expect("commit cv");
            cs = guard;
            if timeout.timed_out() && cs.commit_index < target {
                if !cs.is_leader || cs.term > proposal_term {
                    return Err(auradb_core::Error::NotLeader(
                        "lost leadership before the write committed".into(),
                    ));
                }
                return Err(auradb_core::Error::Internal(
                    "replication timed out waiting for a majority to commit the write".into(),
                ));
            }
        }
    }
}

fn repl_err_to_core(e: ReplicationError) -> auradb_core::Error {
    match e {
        ReplicationError::Apply(err) => err,
        other => auradb_core::Error::Internal(format!("replication: {other}")),
    }
}
