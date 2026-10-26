//! In-process synchronization boundaries for Server read-then-write invariants.
//!
//! Several Server writes decide on a snapshot and then write from it: Report
//! acceptance reads the Agent's active Boot, last accepted sequence and
//! Inventory revision (ADR 0007); Node admission reads the Node's owner and
//! purge tombstone before it upserts the projection; an Owner test
//! notification reads the channel's cooldown ledger before it records the
//! next test. Deciding on one snapshot and writing from another lets two
//! writers both pass the same check and both commit.
//!
//! SQLite serializes writers on its single write connection, so these
//! invariants hold there without help. The PostgreSQL Storage Backend runs
//! several work connections so independent writes stop queuing behind one
//! another (ADR 0013), and that parallelism is only sound if writes that share
//! an invariant still serialize. The fences here are that serialization, each
//! keyed by exactly the subject the invariant is about, every other subject
//! unaffected. The owning Server holds the database's session advisory lock,
//! so a single process is the whole scope a fence has to cover.
//!
//! Lock order, so no two holders can wait on each other: an Agent fence, then
//! Node fences in ascending Node ID order ([`KeyedFence::hold_all`]), then
//! the retention sweep gate. No fence is ever awaited while its holder keeps a
//! database connection or transaction open (the SQLite pool has exactly one),
//! so a short lookup that returns its connection before the fence is fine. A
//! notification-channel fence is never held together with another fence.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::OwnedMutexGuard;

/// Serializes holders of one key, and only of that key.
#[derive(Default)]
pub(crate) struct KeyedFence {
    gates: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl KeyedFence {
    /// Wait until no other holder of `key` is inside its critical section,
    /// and hold that exclusivity until the returned guard drops.
    ///
    /// A gate lives exactly as long as some holder holds or waits for it, so a
    /// busy deployment does not accumulate one entry per key it has ever
    /// seen.
    pub(crate) async fn hold(&self, key: &str) -> OwnedMutexGuard<()> {
        self.gate(key).lock_owned().await
    }

    /// Hold every key at once, acquired in ascending order with duplicates
    /// folded, so two holders of overlapping key sets can never each hold one
    /// key the other is waiting for.
    pub(crate) async fn hold_all<I, K>(&self, keys: I) -> Vec<OwnedMutexGuard<()>>
    where
        I: IntoIterator<Item = K>,
        K: Into<String>,
    {
        let keys = keys
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<String>>();
        let mut held = Vec::with_capacity(keys.len());
        for key in &keys {
            held.push(self.hold(key).await);
        }
        held
    }

    fn gate(&self, key: &str) -> Arc<AsyncMutex<()>> {
        let mut gates = self.gates.lock().expect("write fence poisoned");
        gates.retain(|_, gate| gate.strong_count() > 0);
        match gates.get(key).and_then(Weak::upgrade) {
            Some(gate) => gate,
            None => {
                let gate = Arc::new(AsyncMutex::new(()));
                gates.insert(key.to_owned(), Arc::downgrade(&gate));
                gate
            }
        }
    }
}

/// Every write fence the Server shares across requests and workers.
#[derive(Default)]
pub(crate) struct WriteFences {
    /// One Report of one Agent inside its receipt transaction at a time
    /// (ADR 0007): acceptance reads and advances the Agent's sequence.
    pub(crate) agents: KeyedFence,
    /// One writer of one Node's ownership and existence at a time: Report
    /// admission (owner, purge tombstone, projection upsert), Owner Purge,
    /// Agent Removal and Owner Transfer creation/cancellation (issue #280).
    pub(crate) nodes: KeyedFence,
    /// One Owner test command per notification channel at a time, so the
    /// cooldown read and the ledger row that starts the next cooldown commit
    /// together (issue #280).
    pub(crate) notification_channels: KeyedFence,
}

impl WriteFences {
    pub(crate) fn new() -> Self {
        Self::default()
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
    async fn one_key_admits_one_holder_at_a_time() {
        let fence = Arc::new(KeyedFence::default());
        let inside = Arc::new(AtomicUsize::new(0));
        let mut holders = Vec::new();
        for _ in 0..8 {
            let fence = Arc::clone(&fence);
            let inside = Arc::clone(&inside);
            holders.push(tokio::spawn(async move {
                let _held = fence.hold("agent-a").await;
                let concurrent = inside.fetch_add(1, Ordering::SeqCst) + 1;
                assert_eq!(concurrent, 1, "a second holder of one key was admitted");
                tokio::time::sleep(Duration::from_millis(2)).await;
                inside.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for holder in holders {
            holder.await.unwrap();
        }
    }

    // The fences replace SQLite's single writer on the PostgreSQL work
    // connections, so they must not become a global write lock.
    #[tokio::test]
    async fn different_keys_hold_their_gates_at_the_same_time() {
        let fence = Arc::new(KeyedFence::default());
        let both_inside = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(AtomicUsize::new(0));
        let mut holders = Vec::new();
        for key in ["agent-a", "agent-b"] {
            let fence = Arc::clone(&fence);
            let both_inside = Arc::clone(&both_inside);
            let arrived = Arc::clone(&arrived);
            holders.push(tokio::spawn(async move {
                let _held = fence.hold(key).await;
                if arrived.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    both_inside.store(true, Ordering::SeqCst);
                }
                tokio::time::timeout(Duration::from_secs(5), async {
                    while arrived.load(Ordering::SeqCst) < 2 {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .expect("two keys were serialized against each other");
            }));
        }
        for holder in holders {
            holder.await.unwrap();
        }
        assert!(both_inside.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_released_key_leaves_no_gate_behind() {
        let fence = KeyedFence::default();
        drop(fence.hold("agent-a").await);
        // A released gate is swept by the next hold rather than at the moment
        // its guard drops: the sweep is what keeps the map from growing with
        // every key the Server has ever served.
        drop(fence.hold("agent-b").await);
        let gates = fence.gates.lock().unwrap();
        assert_eq!(gates.len(), 1, "a released key's gate outlived its holder");
        assert!(gates.contains_key("agent-b"));
    }

    // Two Reports declaring overlapping Node sets in opposite orders must not
    // each hold one Node the other waits for.
    #[tokio::test]
    async fn overlapping_key_sets_in_opposite_orders_do_not_deadlock() {
        let fence = Arc::new(KeyedFence::default());
        let mut holders = Vec::new();
        for round in 0..32 {
            let fence = Arc::clone(&fence);
            let keys = if round % 2 == 0 {
                vec!["node-a", "node-b", "node-c"]
            } else {
                vec!["node-c", "node-b", "node-a"]
            };
            holders.push(tokio::spawn(async move {
                let held = fence.hold_all(keys).await;
                tokio::task::yield_now().await;
                drop(held);
            }));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            for holder in holders {
                holder.await.unwrap();
            }
        })
        .await
        .expect("overlapping key sets deadlocked");
    }

    #[tokio::test]
    async fn a_repeated_key_is_held_once() {
        let fence = KeyedFence::default();
        let held =
            tokio::time::timeout(Duration::from_secs(1), fence.hold_all(["node-a", "node-a"]))
                .await
                .expect("a key repeated in one set waited on itself");
        assert_eq!(held.len(), 1);
    }
}
