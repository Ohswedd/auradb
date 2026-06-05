//! A deterministic, in-process multi-node simulation harness.
//!
//! The harness wires several [`RaftNode`]s together over an in-memory message
//! bus with no real sockets and no wall-clock timing. It is the substrate for
//! the crate's multi-node consensus tests and for the replication crate's
//! end-to-end apply tests: everything is driven by [`Sim::tick`] and message
//! delivery, so runs are fully reproducible.

use std::collections::{HashMap, VecDeque};

use auradb_cluster::{NodeId, NodeRole};

use crate::error::Result;
use crate::log::{Command, LogIndex, MemStorage};
use crate::node::{Envelope, RaftConfig, RaftNode};

/// A queued message in flight between two nodes.
struct InFlight {
    from: NodeId,
    envelope: Envelope,
}

/// A deterministic cluster of in-memory Raft nodes.
pub struct Sim {
    nodes: HashMap<NodeId, RaftNode<MemStorage>>,
    order: Vec<NodeId>,
    bus: VecDeque<InFlight>,
    /// Ids of nodes that are "partitioned": their messages are dropped.
    partitioned: std::collections::HashSet<NodeId>,
}

impl Sim {
    /// Build a cluster of `ids.len()` nodes that all know about each other.
    pub fn new(ids: &[NodeId]) -> Sim {
        Sim::with_timeouts(ids, 10, 20, 3)
    }

    /// Build a cluster with explicit timeouts (in ticks).
    pub fn with_timeouts(
        ids: &[NodeId],
        election_min: u32,
        election_max: u32,
        heartbeat: u32,
    ) -> Sim {
        let mut nodes = HashMap::new();
        for &id in ids {
            let peers = ids.iter().copied().filter(|&p| p != id).collect();
            let config = RaftConfig {
                id,
                peers,
                election_timeout_min: election_min,
                election_timeout_max: election_max,
                heartbeat_interval: heartbeat,
            };
            nodes.insert(id, RaftNode::new(config, MemStorage::new()));
        }
        Sim {
            nodes,
            order: ids.to_vec(),
            bus: VecDeque::new(),
            partitioned: std::collections::HashSet::new(),
        }
    }

    /// Borrow a node by id.
    pub fn node(&self, id: NodeId) -> &RaftNode<MemStorage> {
        &self.nodes[&id]
    }

    /// Mutably borrow a node by id.
    pub fn node_mut(&mut self, id: NodeId) -> &mut RaftNode<MemStorage> {
        self.nodes.get_mut(&id).expect("node exists")
    }

    /// Partition a node: it stops sending and receiving until healed.
    pub fn partition(&mut self, id: NodeId) {
        self.partitioned.insert(id);
    }

    /// Heal a previously partitioned node.
    pub fn heal(&mut self, id: NodeId) {
        self.partitioned.remove(&id);
    }

    /// Advance every node by one tick, collecting outgoing messages.
    pub fn tick(&mut self) {
        for id in self.order.clone() {
            self.nodes.get_mut(&id).expect("node").tick();
            self.collect(id);
        }
    }

    /// Propose a command on a specific node (must be leader).
    pub fn propose(&mut self, id: NodeId, command: Command) -> Result<LogIndex> {
        let index = self.nodes.get_mut(&id).expect("node").propose(command)?;
        self.collect(id);
        Ok(index)
    }

    fn collect(&mut self, id: NodeId) {
        if self.partitioned.contains(&id) {
            // Drop outgoing messages from a partitioned node.
            self.nodes.get_mut(&id).expect("node").take_messages();
            return;
        }
        let msgs = self.nodes.get_mut(&id).expect("node").take_messages();
        for envelope in msgs {
            self.bus.push_back(InFlight { from: id, envelope });
        }
    }

    /// Deliver one queued message. Returns `false` if the bus was empty.
    pub fn deliver_one(&mut self) -> bool {
        while let Some(inflight) = self.bus.pop_front() {
            let to = inflight.envelope.to;
            if self.partitioned.contains(&to) || self.partitioned.contains(&inflight.from) {
                continue; // dropped by the partition
            }
            if let Some(node) = self.nodes.get_mut(&to) {
                node.step(inflight.from, inflight.envelope.message)
                    .expect("step never fails in-memory");
                self.collect(to);
            }
            return true;
        }
        false
    }

    /// Deliver all queued messages, following cascades to quiescence.
    pub fn deliver_all(&mut self) {
        let mut guard = 0;
        while self.deliver_one() {
            guard += 1;
            assert!(guard < 100_000, "message delivery did not converge");
        }
    }

    /// Tick repeatedly, delivering messages, until a leader exists or `max_ticks`
    /// is exhausted. Returns the leader's id if one emerged.
    pub fn run_until_leader(&mut self, max_ticks: u32) -> Option<NodeId> {
        for _ in 0..max_ticks {
            self.tick();
            self.deliver_all();
            if let Some(id) = self.leader() {
                // Let one more round of heartbeats settle commit indices.
                self.tick();
                self.deliver_all();
                return Some(id);
            }
        }
        self.leader()
    }

    /// The current leader, if exactly one node believes it is leader.
    pub fn leader(&self) -> Option<NodeId> {
        let leaders: Vec<NodeId> = self
            .order
            .iter()
            .copied()
            .filter(|id| self.nodes[id].role() == NodeRole::Leader)
            .collect();
        if leaders.len() == 1 {
            Some(leaders[0])
        } else {
            None
        }
    }

    /// All node ids in their configured order.
    pub fn ids(&self) -> &[NodeId] {
        &self.order
    }
}
