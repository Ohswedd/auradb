//! Deterministic Raft consensus tests.
//!
//! Every test drives a logical clock and an in-memory message bus, so there is
//! no wall-clock timing and no flakiness. Multi-node tests use the [`Sim`]
//! harness; single-node and durability tests drive a [`RaftNode`] directly.

use auradb_cluster::{NodeId, NodeRole};
use auradb_raft::{
    Command, CommandKind, FileStorage, HardState, LogIndex, MemStorage, Message, RaftConfig,
    RaftNode, RaftStorage, Sim, Term,
};

fn ids(n: u64) -> Vec<NodeId> {
    (1..=n).map(NodeId::from_raw).collect()
}

fn db_command(payload: &[u8]) -> Command {
    Command::new(CommandKind::Database, payload.to_vec())
}

#[test]
fn single_node_elects_self() {
    let id = NodeId::from_raw(1);
    let mut node = RaftNode::new(RaftConfig::single_node(id), MemStorage::new());
    node.campaign();
    assert_eq!(node.role(), NodeRole::Leader);
    assert_eq!(node.leader_id(), Some(id));
    // The leader anchored a no-op for its term, which commits immediately.
    assert_eq!(node.commit_index(), LogIndex(1));
}

#[test]
fn follower_grants_vote_once_per_term() {
    let mut node = RaftNode::new(
        RaftConfig::single_node(NodeId::from_raw(9)),
        MemStorage::new(),
    );
    let a = NodeId::from_raw(2);
    let b = NodeId::from_raw(3);
    node.step(
        a,
        Message::RequestVote {
            term: Term(5),
            last_log_index: LogIndex::ZERO,
            last_log_term: Term::ZERO,
        },
    )
    .unwrap();
    let granted_a = matches!(
        node.take_messages().first().map(|e| &e.message),
        Some(Message::RequestVoteResp { granted: true, .. })
    );
    assert!(granted_a, "first candidate in a term gets the vote");

    // A different candidate in the same term must be refused.
    node.step(
        b,
        Message::RequestVote {
            term: Term(5),
            last_log_index: LogIndex::ZERO,
            last_log_term: Term::ZERO,
        },
    )
    .unwrap();
    let granted_b = matches!(
        node.take_messages().first().map(|e| &e.message),
        Some(Message::RequestVoteResp { granted: true, .. })
    );
    assert!(!granted_b, "the vote is already spent for this term");
}

#[test]
fn candidate_becomes_leader_with_majority() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).expect("a leader is elected");
    assert!(sim.ids().contains(&leader));
    // Exactly one leader.
    let leaders = sim
        .ids()
        .iter()
        .filter(|id| sim.node(**id).role() == NodeRole::Leader)
        .count();
    assert_eq!(leaders, 1);
}

#[test]
fn leader_sends_heartbeats() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    let follower = *sim.ids().iter().find(|&&id| id != leader).unwrap();
    let before = sim.node(follower).metrics().append_entries_received;
    for _ in 0..6 {
        sim.tick();
        sim.deliver_all();
    }
    let after = sim.node(follower).metrics().append_entries_received;
    assert!(after > before, "followers keep receiving heartbeats");
}

#[test]
fn follower_steps_down_on_higher_term() {
    let id = NodeId::from_raw(1);
    let mut node = RaftNode::new(RaftConfig::single_node(id), MemStorage::new());
    node.campaign();
    assert_eq!(node.role(), NodeRole::Leader);
    let term = node.term();
    // A message from a higher term forces a step-down.
    node.step(
        NodeId::from_raw(2),
        Message::AppendEntries {
            term: term.next(),
            prev_log_index: LogIndex::ZERO,
            prev_log_term: Term::ZERO,
            entries: vec![],
            leader_commit: LogIndex::ZERO,
        },
    )
    .unwrap();
    assert_eq!(node.role(), NodeRole::Follower);
    assert_eq!(node.term(), term.next());
}

#[test]
fn append_entries_rejects_inconsistent_log() {
    let mut node = RaftNode::new(
        RaftConfig::single_node(NodeId::from_raw(1)),
        MemStorage::new(),
    );
    let leader = NodeId::from_raw(2);
    // prev_log_index points past our (empty) log: inconsistent.
    node.step(
        leader,
        Message::AppendEntries {
            term: Term(1),
            prev_log_index: LogIndex(5),
            prev_log_term: Term(1),
            entries: vec![],
            leader_commit: LogIndex::ZERO,
        },
    )
    .unwrap();
    let success = matches!(
        node.take_messages().first().map(|e| &e.message),
        Some(Message::AppendEntriesResp { success: true, .. })
    );
    assert!(!success, "an inconsistent append is rejected");
}

#[test]
fn append_entries_repairs_log() {
    // A follower with a divergent tail is repaired by the leader backing off.
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    // Replicate several entries.
    for i in 0..3 {
        sim.propose(leader, db_command(&[i])).unwrap();
        sim.deliver_all();
    }
    let last = sim.node(leader).last_log_index();
    let all_ids = sim.ids().to_vec();
    for id in all_ids {
        // Every follower converges to the leader's last index.
        let mut guard = 0;
        while sim.node(id).last_log_index() < last && guard < 50 {
            sim.tick();
            sim.deliver_all();
            guard += 1;
        }
        assert_eq!(sim.node(id).last_log_index(), last, "node {id} repaired");
    }
}

#[test]
fn leader_advances_commit_index_after_majority() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    let before = sim.node(leader).commit_index();
    let index = sim.propose(leader, db_command(b"x")).unwrap();
    // Settle replication.
    for _ in 0..10 {
        sim.tick();
        sim.deliver_all();
    }
    assert!(sim.node(leader).commit_index() >= index);
    assert!(sim.node(leader).commit_index() > before);
}

#[test]
fn committed_entries_are_applied_once() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    sim.propose(leader, db_command(b"once")).unwrap();
    for _ in 0..10 {
        sim.tick();
        sim.deliver_all();
    }
    let first = sim.node_mut(leader).take_committed();
    assert!(first
        .iter()
        .any(|e| e.command.kind == CommandKind::Database));
    // A second drain yields nothing: applied entries are not re-delivered.
    let second = sim.node_mut(leader).take_committed();
    assert!(second.is_empty(), "entries apply exactly once");
}

#[test]
fn restart_preserves_term_vote_and_log() {
    let dir = tempfile::tempdir().unwrap();
    let id = NodeId::from_raw(1);
    let (term, last_index, vote) = {
        let storage = FileStorage::open(dir.path()).unwrap();
        let mut node = RaftNode::new(RaftConfig::single_node(id), storage);
        node.campaign(); // becomes leader, term -> 1, votes for self
        node.propose(db_command(b"a")).unwrap();
        node.propose(db_command(b"b")).unwrap();
        (
            node.term(),
            node.last_log_index(),
            node.storage().hard_state().voted_for,
        )
    };
    // Reopen from the same durable files.
    let storage = FileStorage::open(dir.path()).unwrap();
    let node = RaftNode::new(RaftConfig::single_node(id), storage);
    assert_eq!(node.term(), term);
    assert_eq!(node.last_log_index(), last_index);
    assert_eq!(node.storage().hard_state().voted_for, vote);
}

#[test]
fn two_node_connection_establishes() {
    let mut sim = Sim::new(&ids(2));
    assert!(
        sim.run_until_leader(200).is_some(),
        "two nodes elect a leader"
    );
}

#[test]
fn three_node_elects_leader() {
    let mut sim = Sim::new(&ids(3));
    assert!(sim.run_until_leader(200).is_some());
}

#[test]
fn leader_replicates_entry_to_follower() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    let index = sim.propose(leader, db_command(b"replicated")).unwrap();
    for _ in 0..10 {
        sim.tick();
        sim.deliver_all();
    }
    for &id in sim.ids() {
        let entry = sim.node(id).storage().entry_at(index);
        assert!(entry.is_some(), "node {id} has the replicated entry");
        assert_eq!(entry.unwrap().command.payload, b"replicated");
    }
}

#[test]
fn follower_catches_up_after_restart() {
    // Model a restart as a fresh follower with empty storage rejoining: the
    // leader replays its log to bring the follower fully up to date.
    let all = ids(3);
    let mut sim = Sim::new(&all);
    let leader = sim.run_until_leader(200).unwrap();
    for i in 0..5 {
        sim.propose(leader, db_command(&[i])).unwrap();
    }
    sim.deliver_all();
    let target = sim.node(leader).last_log_index();

    // Reset one follower's log to empty (simulating loss) and let it catch up.
    let follower = *all.iter().find(|&&id| id != leader).unwrap();
    let cfg = RaftConfig {
        id: follower,
        peers: all.iter().copied().filter(|&p| p != follower).collect(),
        election_timeout_min: 10,
        election_timeout_max: 20,
        heartbeat_interval: 3,
    };
    *sim.node_mut(follower) = RaftNode::new(cfg, MemStorage::new());

    let mut guard = 0;
    while sim.node(follower).last_log_index() < target && guard < 100 {
        sim.tick();
        sim.deliver_all();
        guard += 1;
    }
    assert_eq!(sim.node(follower).last_log_index(), target);
}

#[test]
fn network_partition_simulation() {
    let all = ids(3);
    let mut sim = Sim::new(&all);
    let leader = sim.run_until_leader(200).unwrap();

    // Partition the leader away; the remaining majority elects a new one.
    sim.partition(leader);
    let mut new_leader = None;
    for _ in 0..200 {
        sim.tick();
        sim.deliver_all();
        if let Some(l) = sim
            .ids()
            .iter()
            .copied()
            .find(|&id| id != leader && sim.node(id).role() == NodeRole::Leader)
        {
            new_leader = Some(l);
            break;
        }
    }
    let new_leader = new_leader.expect("majority elects a new leader during the partition");
    assert_ne!(new_leader, leader);

    // Heal the old leader; it must step down to the higher term.
    sim.heal(leader);
    for _ in 0..50 {
        sim.tick();
        sim.deliver_all();
    }
    assert_eq!(
        sim.node(leader).role(),
        NodeRole::Follower,
        "the old leader steps down after healing"
    );
}

#[test]
fn raft_log_corruption_detected_via_storage() {
    // A direct durability assertion living with the consensus suite.
    let dir = tempfile::tempdir().unwrap();
    {
        let mut s = FileStorage::open(dir.path()).unwrap();
        s.save_hard_state(&HardState {
            current_term: Term(3),
            voted_for: None,
            commit_index: LogIndex::ZERO,
        })
        .unwrap();
        s.append(&[auradb_raft::LogEntry {
            term: Term(3),
            index: LogIndex(1),
            command: db_command(b"data"),
        }])
        .unwrap();
    }
    let path = dir.path().join("raft-log.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    let n = bytes.len();
    bytes[n - 2] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();
    assert!(FileStorage::open(dir.path()).is_err());
}

// ---- deterministic multi-node partition tests (v0.4.1) ----

/// Drive the simulation until `pred` holds or `budget` ticks are exhausted.
fn run_until(sim: &mut Sim, budget: u32, mut pred: impl FnMut(&Sim) -> bool) -> bool {
    for _ in 0..budget {
        if pred(sim) {
            return true;
        }
        sim.tick();
        sim.deliver_all();
    }
    pred(sim)
}

fn other_leader(sim: &Sim, excluding: NodeId) -> Option<NodeId> {
    sim.ids()
        .iter()
        .copied()
        .find(|&id| id != excluding && sim.node(id).role() == NodeRole::Leader)
}

#[test]
fn minority_partition_cannot_commit() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    // Isolate the leader: it is now a minority of one and cannot reach a majority.
    sim.partition(leader);
    let committed_before = sim.node(leader).commit_index();
    let proposed = sim.propose(leader, db_command(b"isolated")).unwrap();
    // Give it ample time; with no majority the entry never commits.
    for _ in 0..100 {
        sim.tick();
        sim.deliver_all();
    }
    assert!(
        sim.node(leader).commit_index() < proposed,
        "an isolated minority leader cannot commit new entries"
    );
    assert_eq!(sim.node(leader).commit_index(), committed_before);
}

#[test]
fn majority_partition_elects_leader() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    sim.partition(leader);
    // The remaining majority elects a different leader.
    let elected = run_until(&mut sim, 200, |s| other_leader(s, leader).is_some());
    assert!(elected, "the majority side elects a new leader");
    let new_leader = other_leader(&sim, leader).unwrap();
    assert_ne!(new_leader, leader);
}

#[test]
fn leader_loses_majority_stops_committing() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    // Commit one entry while healthy.
    let committed = sim.propose(leader, db_command(b"healthy")).unwrap();
    run_until(&mut sim, 50, |s| s.node(leader).commit_index() >= committed);
    let high_water = sim.node(leader).commit_index();
    // Lose the majority; further proposals on the old leader cannot commit.
    sim.partition(leader);
    let _ = sim.propose(leader, db_command(b"after-loss"));
    for _ in 0..80 {
        sim.tick();
        sim.deliver_all();
    }
    assert_eq!(
        sim.node(leader).commit_index(),
        high_water,
        "a leader without a majority stops advancing the commit index"
    );
}

#[test]
fn old_leader_rejoins_and_steps_down() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    sim.partition(leader);
    run_until(&mut sim, 200, |s| other_leader(s, leader).is_some());
    // Heal the old leader; it observes the higher term and reverts to follower.
    sim.heal(leader);
    let stepped_down = run_until(&mut sim, 100, |s| {
        s.node(leader).role() == NodeRole::Follower
    });
    assert!(stepped_down, "the old leader steps down after rejoining");
}

#[test]
fn partitioned_follower_catches_up_after_heal() {
    let all = ids(3);
    let mut sim = Sim::new(&all);
    let leader = sim.run_until_leader(200).unwrap();
    let follower = *all.iter().find(|&&id| id != leader).unwrap();
    // Isolate a follower, then commit several entries with the remaining majority.
    sim.partition(follower);
    for i in 0..4 {
        sim.propose(leader, db_command(&[i])).unwrap();
        sim.deliver_all();
    }
    let target = sim.node(leader).last_log_index();
    // Heal; the follower catches up to the leader's log.
    sim.heal(follower);
    let caught_up = run_until(&mut sim, 200, |s| {
        s.node(follower).last_log_index() >= target
    });
    assert!(caught_up, "the healed follower catches up to the leader");
}

#[test]
fn committed_entry_survives_leader_change() {
    let mut sim = Sim::new(&ids(3));
    let leader = sim.run_until_leader(200).unwrap();
    let idx = sim.propose(leader, db_command(b"durable")).unwrap();
    run_until(&mut sim, 50, |s| s.node(leader).commit_index() >= idx);

    // Force a leader change by partitioning the current leader.
    sim.partition(leader);
    run_until(&mut sim, 200, |s| other_leader(s, leader).is_some());
    let new_leader = other_leader(&sim, leader).unwrap();
    // The committed entry is present on the new leader.
    let entry = sim.node(new_leader).storage().entry_at(idx);
    assert!(
        entry.is_some(),
        "a committed entry survives the leader change"
    );
    assert_eq!(entry.unwrap().command.payload, b"durable");
}

#[test]
fn uncommitted_old_leader_entry_not_committed_after_partition() {
    let all = ids(3);
    let mut sim = Sim::new(&all);
    let leader = sim.run_until_leader(200).unwrap();

    // The leader appends an entry but is isolated before it can replicate: it is
    // uncommitted.
    sim.partition(leader);
    let orphan = sim.propose(leader, db_command(b"orphan")).unwrap();
    for _ in 0..20 {
        sim.tick();
        sim.deliver_all();
    }
    assert!(sim.node(leader).commit_index() < orphan);

    // The majority elects a new leader and commits its own entries at the same
    // indices.
    run_until(&mut sim, 200, |s| other_leader(s, leader).is_some());
    let new_leader = other_leader(&sim, leader).unwrap();
    let committed = sim.propose(new_leader, db_command(b"real")).unwrap();
    run_until(&mut sim, 100, |s| {
        s.node(new_leader).commit_index() >= committed
    });

    // Heal the old leader; its orphaned entry is repaired away and never commits.
    let real_commit = sim.node(new_leader).commit_index();
    sim.heal(leader);
    run_until(&mut sim, 200, |s| {
        s.node(leader).role() == NodeRole::Follower
            && s.node(leader).last_log_index() >= real_commit
    });
    let repaired = sim.node(leader).storage().entry_at(orphan);
    if let Some(entry) = repaired {
        assert_ne!(
            entry.command.payload, b"orphan",
            "the orphaned uncommitted entry was overwritten by the new leader's log"
        );
    }
}

#[test]
fn conflicting_logs_repaired_after_partition() {
    let all = ids(3);
    let mut sim = Sim::new(&all);
    let leader = sim.run_until_leader(200).unwrap();
    // Old leader writes a divergent, uncommitted tail while isolated.
    sim.partition(leader);
    sim.propose(leader, db_command(b"diverged-1")).unwrap();
    sim.propose(leader, db_command(b"diverged-2")).unwrap();
    for _ in 0..10 {
        sim.tick();
        sim.deliver_all();
    }
    // Majority elects a new leader and commits its own entries.
    run_until(&mut sim, 200, |s| other_leader(s, leader).is_some());
    let new_leader = other_leader(&sim, leader).unwrap();
    for _ in 0..3 {
        sim.propose(new_leader, db_command(b"authoritative"))
            .unwrap();
    }
    sim.deliver_all();
    let target = sim.node(new_leader).last_log_index();

    // Heal; the old leader's conflicting tail is repaired to match the new leader.
    sim.heal(leader);
    let converged = run_until(&mut sim, 300, |s| {
        s.node(leader).last_log_index() == target
            && s.node(leader).storage().last_term() == s.node(new_leader).storage().last_term()
    });
    assert!(converged, "the divergent log is repaired after healing");
}
