//! Per-Agent serialization for Report Ingestion.
//!
//! Accepting a Report is a read-then-write invariant: ingestion reads the
//! Agent's active Boot, last accepted sequence and Inventory revision, decides,
//! and only then writes the next state back (ADR 0007; ADR 0013, concurrency
//! section).
//! Deciding on one snapshot and writing from another lets the newer Report
//! commit first and the older one commit afterwards, which would move
//! `last_report_sequence` and the Current Projection backwards — the very
//! sequence fence the acceptance rules implement would hold for neither.
//!
//! SQLite serializes writers on its single write connection, so the invariant
//! holds there without help. The PostgreSQL Storage Backend runs several work
//! connections so Reports of different Agents stop queuing behind one another
//! (ADR 0013), and that parallelism is only sound if one Agent's Reports still
//! serialize. This fence is that serialization: one Report of one Agent inside
//! the receipt transaction at a time, every other Agent unaffected. The owning
//! Server holds the database's session advisory lock, so a single process is
//! the whole scope the fence has to cover.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::OwnedMutexGuard;

/// Serializes Report acceptance per Agent, and only per Agent.
#[derive(Default)]
pub(crate) struct AgentIngestFence {
    gates: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl AgentIngestFence {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Wait until no other Report of `agent_id` is inside its receipt
    /// transaction, and hold that exclusivity until the returned guard drops.
    ///
    /// A gate lives exactly as long as some Report holds or waits for it, so a
    /// busy deployment does not accumulate one entry per Agent it has ever
    /// seen.
    pub(crate) async fn hold(&self, agent_id: &str) -> OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self.gates.lock().expect("Agent ingest fence poisoned");
            gates.retain(|_, gate| gate.strong_count() > 0);
            match gates.get(agent_id).and_then(Weak::upgrade) {
                Some(gate) => gate,
                None => {
                    let gate = Arc::new(AsyncMutex::new(()));
                    gates.insert(agent_id.to_owned(), Arc::downgrade(&gate));
                    gate
                }
            }
        };
        gate.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    // Two Reports of the same Agent must never be inside the acceptance
    // decision together: the second would read a sequence the first has not
    // written yet.
    #[tokio::test]
    async fn one_agent_admits_one_report_at_a_time() {
        let fence = Arc::new(AgentIngestFence::new());
        let inside = Arc::new(AtomicUsize::new(0));
        let mut reports = Vec::new();
        for _ in 0..8 {
            let fence = Arc::clone(&fence);
            let inside = Arc::clone(&inside);
            reports.push(tokio::spawn(async move {
                let _held = fence.hold("agent-a").await;
                let concurrent = inside.fetch_add(1, Ordering::SeqCst) + 1;
                assert_eq!(concurrent, 1, "a second Report of one Agent was admitted");
                tokio::time::sleep(Duration::from_millis(2)).await;
                inside.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for report in reports {
            report.await.unwrap();
        }
    }

    // The fence is the PostgreSQL work-connection pool's replacement for
    // SQLite's single writer, so it must not become a global ingest lock.
    #[tokio::test]
    async fn different_agents_hold_their_gates_at_the_same_time() {
        let fence = Arc::new(AgentIngestFence::new());
        let both_inside = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(AtomicUsize::new(0));
        let mut reports = Vec::new();
        for agent in ["agent-a", "agent-b"] {
            let fence = Arc::clone(&fence);
            let both_inside = Arc::clone(&both_inside);
            let arrived = Arc::clone(&arrived);
            reports.push(tokio::spawn(async move {
                let _held = fence.hold(agent).await;
                if arrived.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    both_inside.store(true, Ordering::SeqCst);
                }
                tokio::time::timeout(Duration::from_secs(5), async {
                    while arrived.load(Ordering::SeqCst) < 2 {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .expect("two Agents were serialized against each other");
            }));
        }
        for report in reports {
            report.await.unwrap();
        }
        assert!(both_inside.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_finished_agent_leaves_no_gate_behind() {
        let fence = AgentIngestFence::new();
        drop(fence.hold("agent-a").await);
        // A released gate is swept by the next hold rather than at the moment
        // its guard drops: the sweep is what keeps the map from growing with
        // every Agent the Server has ever served.
        drop(fence.hold("agent-b").await);
        let gates = fence.gates.lock().unwrap();
        assert_eq!(
            gates.len(),
            1,
            "a released Agent's gate outlived its Report"
        );
        assert!(gates.contains_key("agent-b"));
    }
}
