//! A minimal, correct, deterministic Raft node.
//!
//! The node is driven by a **logical clock**: [`RaftNode::tick`] advances time by
//! one unit. Election and heartbeat timeouts are measured in ticks, so tests
//! control time exactly and never depend on wall-clock timing. Randomized
//! election timeouts come from a small deterministic PRNG seeded by the node id,
//! so a multi-node cluster still elects a stable leader without any real
//! randomness.
//!
//! The node does no I/O of its own beyond the [`RaftStorage`] it owns and a
//! queue of outgoing [`Envelope`]s the caller delivers. This keeps consensus a
//! pure state machine that is trivial to test in-process.

use std::collections::HashMap;

use auradb_cluster::{NodeId, NodeRole};

use crate::error::{RaftError, Result};
use crate::log::{Command, LogEntry, LogIndex, RaftStorage, Term};

/// Static configuration for a Raft node.
#[derive(Debug, Clone)]
pub struct RaftConfig {
    /// This node's id.
    pub id: NodeId,
    /// The other voting members (this node excluded).
    pub peers: Vec<NodeId>,
    /// Minimum election timeout, in ticks.
    pub election_timeout_min: u32,
    /// Maximum election timeout, in ticks (must be >= min).
    pub election_timeout_max: u32,
    /// Heartbeat interval, in ticks (must be < election_timeout_min).
    pub heartbeat_interval: u32,
}

impl RaftConfig {
    /// A single-node configuration (no peers).
    pub fn single_node(id: NodeId) -> RaftConfig {
        RaftConfig {
            id,
            peers: Vec::new(),
            election_timeout_min: 10,
            election_timeout_max: 20,
            heartbeat_interval: 3,
        }
    }

    /// The full voter set (peers plus self).
    fn voters(&self) -> usize {
        self.peers.len() + 1
    }

    /// The number of votes needed for a majority.
    fn majority(&self) -> usize {
        self.voters() / 2 + 1
    }
}

/// A Raft message addressed to a peer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// The recipient.
    pub to: NodeId,
    /// The message body.
    pub message: Message,
}

/// The Raft RPC messages.
///
/// These derive `Serialize`/`Deserialize` so the cross-process peer transport
/// (see `auradb-replication`) can carry them over the wire. The Raft state
/// machine itself remains transport-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Message {
    /// A candidate solicits a vote.
    RequestVote {
        /// Candidate's term.
        term: Term,
        /// Index of the candidate's last log entry.
        last_log_index: LogIndex,
        /// Term of the candidate's last log entry.
        last_log_term: Term,
    },
    /// A vote response.
    RequestVoteResp {
        /// Responder's term.
        term: Term,
        /// Whether the vote was granted.
        granted: bool,
    },
    /// A leader replicates entries (an empty `entries` is a heartbeat).
    AppendEntries {
        /// Leader's term.
        term: Term,
        /// Index immediately preceding the new entries.
        prev_log_index: LogIndex,
        /// Term of the `prev_log_index` entry.
        prev_log_term: Term,
        /// The entries to store.
        entries: Vec<LogEntry>,
        /// Leader's commit index.
        leader_commit: LogIndex,
    },
    /// A response to AppendEntries.
    AppendEntriesResp {
        /// Responder's term.
        term: Term,
        /// Whether the append succeeded (log matched).
        success: bool,
        /// On success, the responder's new last matching index.
        match_index: LogIndex,
    },
}

/// Counters exposed for replication metrics.
#[derive(Debug, Clone, Default)]
pub struct RaftMetrics {
    /// Number of times this node became leader.
    pub leader_changes: u64,
    /// Number of votes this node has granted.
    pub votes_granted: u64,
    /// Number of AppendEntries messages this node has sent.
    pub append_entries_sent: u64,
    /// Number of AppendEntries messages this node has received.
    pub append_entries_received: u64,
}

/// A deterministic Raft node.
pub struct RaftNode<S: RaftStorage> {
    config: RaftConfig,
    storage: S,
    role: NodeRole,
    leader_id: Option<NodeId>,
    commit_index: LogIndex,
    last_applied: LogIndex,
    election_elapsed: u32,
    heartbeat_elapsed: u32,
    randomized_election_timeout: u32,
    next_index: HashMap<NodeId, LogIndex>,
    match_index: HashMap<NodeId, LogIndex>,
    votes: HashMap<NodeId, bool>,
    rng_state: u64,
    outbox: Vec<Envelope>,
    metrics: RaftMetrics,
}

impl<S: RaftStorage> RaftNode<S> {
    /// Create a node, recovering term, vote, and commit index from storage.
    pub fn new(config: RaftConfig, storage: S) -> RaftNode<S> {
        let hs = storage.hard_state();
        // Seed the PRNG from the node id so timeouts are distinct per node yet
        // fully deterministic across runs.
        let seed = config.id.get() ^ 0x9e37_79b9_7f4a_7c15;
        let mut node = RaftNode {
            config,
            storage,
            role: NodeRole::Follower,
            leader_id: None,
            commit_index: hs.commit_index,
            last_applied: hs.commit_index,
            election_elapsed: 0,
            heartbeat_elapsed: 0,
            randomized_election_timeout: 0,
            next_index: HashMap::new(),
            match_index: HashMap::new(),
            votes: HashMap::new(),
            rng_state: seed,
            outbox: Vec::new(),
            metrics: RaftMetrics::default(),
        };
        node.reset_election_timeout();
        node
    }

    // ---- accessors ----

    /// This node's id.
    pub fn id(&self) -> NodeId {
        self.config.id
    }

    /// The current role.
    pub fn role(&self) -> NodeRole {
        self.role
    }

    /// The current term.
    pub fn term(&self) -> Term {
        self.storage.hard_state().current_term
    }

    /// The recognized leader, if any.
    pub fn leader_id(&self) -> Option<NodeId> {
        self.leader_id
    }

    /// Whether this node is the leader.
    pub fn is_leader(&self) -> bool {
        self.role == NodeRole::Leader
    }

    /// The highest committed index.
    pub fn commit_index(&self) -> LogIndex {
        self.commit_index
    }

    /// The highest applied index.
    pub fn applied_index(&self) -> LogIndex {
        self.last_applied
    }

    /// The last log index.
    pub fn last_log_index(&self) -> LogIndex {
        self.storage.last_index()
    }

    /// A read-only view of the metrics counters.
    pub fn metrics(&self) -> &RaftMetrics {
        &self.metrics
    }

    /// Borrow the underlying storage (for inspection).
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Mutably borrow the underlying storage (for maintenance such as log
    /// compaction). The consensus invariants are unaffected as long as the caller
    /// only compacts a prefix at or below the applied index.
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }

    /// Replication lag in entries (commit minus applied).
    pub fn replication_lag(&self) -> u64 {
        self.commit_index
            .get()
            .saturating_sub(self.last_applied.get())
    }

    /// The leader's record of a peer's highest matching log index, if this node
    /// is (or was) a leader tracking that peer. Used for per-peer diagnostics.
    pub fn match_index(&self, peer: NodeId) -> Option<LogIndex> {
        self.match_index.get(&peer).copied()
    }

    /// The leader's next-index for a peer (the next log index to send), if
    /// tracked. Used for per-peer diagnostics.
    pub fn next_index(&self, peer: NodeId) -> Option<LogIndex> {
        self.next_index.get(&peer).copied()
    }

    /// The configured peer ids.
    pub fn peers(&self) -> &[NodeId] {
        &self.config.peers
    }

    // ---- driving the clock ----

    /// Advance the logical clock by one tick.
    pub fn tick(&mut self) {
        match self.role {
            NodeRole::Leader => {
                self.heartbeat_elapsed += 1;
                if self.heartbeat_elapsed >= self.config.heartbeat_interval {
                    self.heartbeat_elapsed = 0;
                    self.broadcast_append();
                }
            }
            NodeRole::Follower | NodeRole::Candidate => {
                self.election_elapsed += 1;
                if self.election_elapsed >= self.randomized_election_timeout {
                    self.campaign();
                }
            }
        }
    }

    /// Drain queued outgoing messages.
    pub fn take_messages(&mut self) -> Vec<Envelope> {
        std::mem::take(&mut self.outbox)
    }

    /// Take newly committed (but not yet applied) entries, advancing the applied
    /// index. The caller applies them to the state machine and the applied index
    /// matches the committed index afterwards.
    pub fn take_committed(&mut self) -> Vec<LogEntry> {
        let mut out = Vec::new();
        let mut idx = self.last_applied.next();
        while idx <= self.commit_index {
            if let Some(entry) = self.storage.entry_at(idx) {
                out.push(entry);
            }
            idx = idx.next();
        }
        self.last_applied = self.commit_index;
        out
    }

    // ---- proposing ----

    /// Propose a command (leader only). Returns the index assigned.
    pub fn propose(&mut self, command: Command) -> Result<LogIndex> {
        if self.role != NodeRole::Leader {
            return Err(RaftError::NotLeader(self.leader_id));
        }
        let index = self.storage.last_index().next();
        let entry = LogEntry {
            term: self.term(),
            index,
            command,
        };
        self.storage.append(std::slice::from_ref(&entry))?;
        self.match_index.insert(self.config.id, index);
        self.maybe_advance_commit()?;
        self.broadcast_append();
        Ok(index)
    }

    // ---- handling messages ----

    /// Process an incoming message from `from`, queueing any responses.
    pub fn step(&mut self, from: NodeId, message: Message) -> Result<()> {
        // Any message carrying a newer term forces us to step down and adopt it.
        let msg_term = message_term(&message);
        if msg_term > self.term() {
            self.become_follower(msg_term, None)?;
        }
        match message {
            Message::RequestVote {
                term,
                last_log_index,
                last_log_term,
            } => self.handle_request_vote(from, term, last_log_index, last_log_term),
            Message::RequestVoteResp { term, granted } => {
                self.handle_vote_resp(from, term, granted)
            }
            Message::AppendEntries {
                term,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
            } => self.handle_append_entries(
                from,
                term,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
            ),
            Message::AppendEntriesResp {
                term,
                success,
                match_index,
            } => self.handle_append_resp(from, term, success, match_index),
        }
    }

    fn handle_request_vote(
        &mut self,
        from: NodeId,
        term: Term,
        last_log_index: LogIndex,
        last_log_term: Term,
    ) -> Result<()> {
        let mut hs = self.storage.hard_state();
        let granted = if term < hs.current_term {
            false
        } else {
            let can_vote = hs.voted_for.is_none() || hs.voted_for == Some(from);
            let log_ok = (last_log_term, last_log_index)
                >= (self.storage.last_term(), self.storage.last_index());
            if can_vote && log_ok {
                hs.voted_for = Some(from);
                self.storage.save_hard_state(&hs)?;
                self.election_elapsed = 0;
                self.metrics.votes_granted += 1;
                true
            } else {
                false
            }
        };
        self.send(
            from,
            Message::RequestVoteResp {
                term: self.term(),
                granted,
            },
        );
        Ok(())
    }

    fn handle_vote_resp(&mut self, from: NodeId, term: Term, granted: bool) -> Result<()> {
        if self.role != NodeRole::Candidate || term != self.term() {
            return Ok(());
        }
        self.votes.insert(from, granted);
        let yes = self.votes.values().filter(|&&v| v).count();
        if yes >= self.config.majority() {
            self.become_leader()?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_append_entries(
        &mut self,
        from: NodeId,
        term: Term,
        prev_log_index: LogIndex,
        prev_log_term: Term,
        entries: Vec<LogEntry>,
        leader_commit: LogIndex,
    ) -> Result<()> {
        self.metrics.append_entries_received += 1;
        let current = self.term();
        if term < current {
            self.send(
                from,
                Message::AppendEntriesResp {
                    term: current,
                    success: false,
                    match_index: LogIndex::ZERO,
                },
            );
            return Ok(());
        }
        // Valid leader for this term: become/stay follower and reset the timer.
        self.become_follower(term, Some(from))?;
        self.election_elapsed = 0;

        // Log consistency check on the entry preceding the new ones.
        let prev_ok = match self.storage.term_at(prev_log_index) {
            Some(t) => t == prev_log_term,
            None => false,
        };
        if !prev_ok {
            self.send(
                from,
                Message::AppendEntriesResp {
                    term: self.term(),
                    success: false,
                    match_index: LogIndex::ZERO,
                },
            );
            return Ok(());
        }

        // Append, truncating any conflicting suffix.
        let mut match_index = prev_log_index;
        for entry in &entries {
            match self.storage.term_at(entry.index) {
                Some(existing) if existing == entry.term => {
                    // Already present and consistent.
                }
                Some(_) => {
                    self.storage.truncate_from(entry.index)?;
                    self.storage.append(std::slice::from_ref(entry))?;
                }
                None => {
                    self.storage.append(std::slice::from_ref(entry))?;
                }
            }
            match_index = entry.index;
        }

        if leader_commit > self.commit_index {
            self.commit_index = min_index(leader_commit, self.storage.last_index());
            self.persist_commit_index()?;
        }

        self.send(
            from,
            Message::AppendEntriesResp {
                term: self.term(),
                success: true,
                match_index,
            },
        );
        Ok(())
    }

    fn handle_append_resp(
        &mut self,
        from: NodeId,
        term: Term,
        success: bool,
        match_index: LogIndex,
    ) -> Result<()> {
        if self.role != NodeRole::Leader || term != self.term() {
            return Ok(());
        }
        if success {
            self.match_index.insert(from, match_index);
            self.next_index.insert(from, match_index.next());
            self.maybe_advance_commit()?;
        } else {
            // Log mismatch: back off and retry from an earlier index.
            let next = self
                .next_index
                .get(&from)
                .copied()
                .unwrap_or_else(|| self.storage.last_index().next());
            let backed = LogIndex(next.get().saturating_sub(1).max(1));
            self.next_index.insert(from, backed);
            self.send_append_to(from);
        }
        Ok(())
    }

    // ---- role transitions ----

    /// Force this node to start an election immediately (used on timeout and
    /// exposed for tests).
    pub fn campaign(&mut self) {
        let mut hs = self.storage.hard_state();
        hs.current_term = hs.current_term.next();
        hs.voted_for = Some(self.config.id);
        let _ = self.storage.save_hard_state(&hs);
        self.role = NodeRole::Candidate;
        self.leader_id = None;
        self.votes.clear();
        self.votes.insert(self.config.id, true);
        self.reset_election_timeout();

        if self.config.majority() <= 1 {
            // Single-node cluster: we are the majority.
            let _ = self.become_leader();
            return;
        }

        let last_log_index = self.storage.last_index();
        let last_log_term = self.storage.last_term();
        let term = self.term();
        let peers = self.config.peers.clone();
        for peer in peers {
            self.send(
                peer,
                Message::RequestVote {
                    term,
                    last_log_index,
                    last_log_term,
                },
            );
        }
    }

    fn become_follower(&mut self, term: Term, leader: Option<NodeId>) -> Result<()> {
        let mut hs = self.storage.hard_state();
        if term > hs.current_term {
            hs.current_term = term;
            hs.voted_for = None;
            self.storage.save_hard_state(&hs)?;
        }
        self.role = NodeRole::Follower;
        if leader.is_some() {
            self.leader_id = leader;
        }
        self.votes.clear();
        Ok(())
    }

    fn become_leader(&mut self) -> Result<()> {
        self.role = NodeRole::Leader;
        self.leader_id = Some(self.config.id);
        self.metrics.leader_changes += 1;
        self.heartbeat_elapsed = 0;
        let last = self.storage.last_index();
        self.next_index.clear();
        self.match_index.clear();
        for peer in &self.config.peers {
            self.next_index.insert(*peer, last.next());
            self.match_index.insert(*peer, LogIndex::ZERO);
        }
        self.match_index.insert(self.config.id, last);
        // Anchor the new term with a no-op so prior-term entries can commit.
        let index = last.next();
        let entry = LogEntry {
            term: self.term(),
            index,
            command: Command::noop(),
        };
        self.storage.append(std::slice::from_ref(&entry))?;
        self.match_index.insert(self.config.id, index);
        self.maybe_advance_commit()?;
        self.broadcast_append();
        Ok(())
    }

    // ---- replication helpers ----

    fn broadcast_append(&mut self) {
        let peers = self.config.peers.clone();
        for peer in peers {
            self.send_append_to(peer);
        }
    }

    fn send_append_to(&mut self, peer: NodeId) {
        let next = self
            .next_index
            .get(&peer)
            .copied()
            .unwrap_or_else(|| self.storage.last_index().next());
        let prev_log_index = next.prev();
        let prev_log_term = self.storage.term_at(prev_log_index).unwrap_or(Term::ZERO);
        let entries = self.storage.entries_from(next);
        self.metrics.append_entries_sent += 1;
        self.send(
            peer,
            Message::AppendEntries {
                term: self.term(),
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit: self.commit_index,
            },
        );
    }

    fn maybe_advance_commit(&mut self) -> Result<()> {
        let last = self.storage.last_index();
        let current_term = self.term();
        // Find the highest index replicated on a majority whose entry is from the
        // current term (the Raft commit rule).
        let mut n = self.commit_index.next();
        let mut new_commit = self.commit_index;
        while n <= last {
            let replicated = self.match_index.values().filter(|&&m| m >= n).count();
            let from_current_term = self.storage.term_at(n) == Some(current_term);
            if replicated >= self.config.majority() && from_current_term {
                new_commit = n;
            }
            n = n.next();
        }
        if new_commit > self.commit_index {
            self.commit_index = new_commit;
            self.persist_commit_index()?;
        }
        Ok(())
    }

    fn persist_commit_index(&mut self) -> Result<()> {
        let mut hs = self.storage.hard_state();
        if hs.commit_index != self.commit_index {
            hs.commit_index = self.commit_index;
            self.storage.save_hard_state(&hs)?;
        }
        Ok(())
    }

    fn reset_election_timeout(&mut self) {
        self.election_elapsed = 0;
        let span = self
            .config
            .election_timeout_max
            .saturating_sub(self.config.election_timeout_min)
            .saturating_add(1)
            .max(1);
        let r = (self.next_rand() % span as u64) as u32;
        self.randomized_election_timeout = self.config.election_timeout_min + r;
    }

    fn next_rand(&mut self) -> u64 {
        // SplitMix64: deterministic, well-distributed, no external dependency.
        self.rng_state = self.rng_state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn send(&mut self, to: NodeId, message: Message) {
        self.outbox.push(Envelope { to, message });
    }
}

fn message_term(message: &Message) -> Term {
    match message {
        Message::RequestVote { term, .. }
        | Message::RequestVoteResp { term, .. }
        | Message::AppendEntries { term, .. }
        | Message::AppendEntriesResp { term, .. } => *term,
    }
}

fn min_index(a: LogIndex, b: LogIndex) -> LogIndex {
    if a < b {
        a
    } else {
        b
    }
}

/// Convenience constructor: a single-node Raft node over the given storage.
pub fn single_node<S: RaftStorage>(id: NodeId, storage: S) -> RaftNode<S> {
    RaftNode::new(RaftConfig::single_node(id), storage)
}
