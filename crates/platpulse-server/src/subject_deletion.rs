//! What happens to alerts and notifications when an Owner deletes a subject
//! (main design §15.7, ADR 0004, issue #175).
//!
//! A Node Purge or an Agent Removal is not recovery. The deleted subject
//! leaves current Attention and every later alert evaluation, but its
//! Incident evidence is retained and annotated as subject-deleted -- an open
//! Incident is never rewritten into a claimed known recovery. Its not-yet-sent
//! notifications are cancelled, including resolution Events that carry no
//! incident_id, while already-sent or already-handed-off facts are left
//! exactly as they are. Other subjects, including a shared Validator subject,
//! are never touched.

use sqlx::SqliteConnection;

use crate::alerts::SubjectKind;

/// Counts of what retiring one deleted subject touched. Returned so the
/// deletion mutation can disclose the effect; nothing here fabricates a
/// recovery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubjectRetirement {
    pub incidents_annotated: i64,
    pub evaluation_states_retired: i64,
    pub deliveries_cancelled: i64,
}

impl SubjectRetirement {
    /// Accumulate another retired subject's effect. Agent Removal aggregates
    /// the Agent, its Host, and every purged Node.
    pub fn add(&mut self, other: &Self) {
        self.incidents_annotated += other.incidents_annotated;
        self.evaluation_states_retired += other.evaluation_states_retired;
        self.deliveries_cancelled += other.deliveries_cancelled;
    }
}

/// Whether a subject identity already carries the durable deletion marker.
///
/// The notification worker reads this immediately before handing a Delivery to
/// a provider: a Delivery that only became due after the deletion (for example
/// requeued by crash recovery or scheduled by a send that failed mid-delete)
/// is cancelled instead of sent. Network/Validator/Server subjects are shared
/// and are never Owner-deleted.
pub async fn subject_is_deleted(
    connection: &mut SqliteConnection,
    subject_kind: SubjectKind,
    subject_key: &str,
) -> Result<bool, sqlx::Error> {
    let deleted = match subject_kind {
        SubjectKind::Node => {
            sqlx::query_scalar::<_, i64>("SELECT 1 FROM deleted_nodes WHERE node_id = ?")
                .bind(subject_key)
                .fetch_optional(connection)
                .await?
                .is_some()
        }
        SubjectKind::Agent | SubjectKind::Host => sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM agents WHERE agent_id = ? AND deleted_at IS NOT NULL",
        )
        .bind(subject_key)
        .fetch_optional(connection)
        .await?
        .is_some(),
        SubjectKind::Network | SubjectKind::Validator | SubjectKind::Server => false,
    };
    Ok(deleted)
}

/// Retire one deleted subject inside the caller's deletion transaction:
/// annotate every retained Incident of that subject with the deletion instant,
/// discard the subject's current evaluation state so it can never surface as a
/// current problem, and cancel its not-yet-sent Deliveries. Already-delivered
/// facts and every other subject are deliberately left untouched.
pub async fn retire_subject(
    connection: &mut SqliteConnection,
    subject_kind: SubjectKind,
    subject_key: &str,
    deleted_at: &str,
) -> Result<SubjectRetirement, sqlx::Error> {
    let incidents = sqlx::query(
        "UPDATE alert_incidents SET subject_deleted_at = ?
         WHERE subject_kind = ? AND subject_key = ? AND subject_deleted_at IS NULL",
    )
    .bind(deleted_at)
    .bind(subject_kind.as_str())
    .bind(subject_key)
    .execute(&mut *connection)
    .await?;

    // Evaluation state is current bookkeeping, not Incident evidence: dropping
    // it removes the subject from rule state/summary reads without resolving
    // any Incident.
    let states =
        sqlx::query("DELETE FROM alert_rule_state WHERE subject_kind = ? AND subject_key = ?")
            .bind(subject_kind.as_str())
            .bind(subject_key)
            .execute(&mut *connection)
            .await?;

    let deliveries = crate::notifications::cancel_unsent_for_subject(
        connection,
        subject_kind,
        subject_key,
        deleted_at,
    )
    .await?;

    Ok(SubjectRetirement {
        incidents_annotated: incidents.rows_affected() as i64,
        evaluation_states_retired: states.rows_affected() as i64,
        deliveries_cancelled: deliveries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    use tempfile::tempdir;

    async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempdir().unwrap();
        let database = crate::database::initialize(crate::database::ServerDatabaseConfig::new(
            dir.path().join("server.db"),
        ))
        .await
        .unwrap();
        let pool = database.pool().clone();
        (dir, pool)
    }

    #[tokio::test]
    async fn deleted_subject_is_detected_from_its_durable_marker_only() {
        let (_dir, pool) = test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(
            !subject_is_deleted(&mut conn, SubjectKind::Node, "node-a")
                .await
                .unwrap()
        );
        assert!(
            !subject_is_deleted(&mut conn, SubjectKind::Agent, "agent-a")
                .await
                .unwrap()
        );
        // Shared subjects are never deletable through this boundary.
        assert!(
            !subject_is_deleted(&mut conn, SubjectKind::Validator, "validator-a")
                .await
                .unwrap()
        );

        sqlx::query("INSERT INTO users (user_id, username, role, password_hash, created_at, updated_at) VALUES ('owner', 'owner', 'owner', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents (agent_id, agent_epoch, created_at, updated_at, deleted_at) VALUES ('agent-a', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z')")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO deleted_nodes (node_id, agent_id, network_key, deleted_by_user_id, deleted_at) VALUES ('node-a', 'agent-a', 'mainnet', 'owner', '2026-01-02T00:00:00Z')")
            .execute(&mut *conn)
            .await
            .unwrap();

        assert!(
            subject_is_deleted(&mut conn, SubjectKind::Node, "node-a")
                .await
                .unwrap()
        );
        assert!(
            subject_is_deleted(&mut conn, SubjectKind::Agent, "agent-a")
                .await
                .unwrap()
        );
        assert!(
            subject_is_deleted(&mut conn, SubjectKind::Host, "agent-a")
                .await
                .unwrap()
        );
    }
}
