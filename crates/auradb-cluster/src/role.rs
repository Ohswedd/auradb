//! Node role within a Raft cluster.

use serde::{Deserialize, Serialize};

/// The role a node currently plays in consensus.
///
/// When cluster mode is disabled the node has no consensus role; callers report
/// that separately (see [`crate::ClusterStatus::enabled`]). When cluster mode is
/// enabled a node is always exactly one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    /// Passive: accepts log entries from a leader and grants votes.
    #[default]
    Follower,
    /// Standing for election in the current term.
    Candidate,
    /// Elected leader for the current term; the only node that accepts writes.
    Leader,
}

impl NodeRole {
    /// The stable lowercase string identifier for this role.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeRole::Follower => "follower",
            NodeRole::Candidate => "candidate",
            NodeRole::Leader => "leader",
        }
    }

    /// Whether a node in this role accepts client writes.
    pub fn accepts_writes(self) -> bool {
        matches!(self, NodeRole::Leader)
    }
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_leader_accepts_writes() {
        assert!(NodeRole::Leader.accepts_writes());
        assert!(!NodeRole::Follower.accepts_writes());
        assert!(!NodeRole::Candidate.accepts_writes());
    }

    #[test]
    fn role_strings_are_stable() {
        assert_eq!(NodeRole::Follower.as_str(), "follower");
        assert_eq!(NodeRole::Candidate.as_str(), "candidate");
        assert_eq!(NodeRole::Leader.as_str(), "leader");
    }
}
