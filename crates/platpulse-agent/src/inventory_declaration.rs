//! The Inventory Declaration Record (issue #181).
//!
//! `inventory_revision` is the Agent's monotonic *intent*, and the Inventory
//! content hash is the *fact*: the Server refuses a report whose content
//! differs from the accepted content at the same revision, and it must keep
//! doing so (otherwise Inventory would degrade to last-write-wins). What was
//! missing was a local memory of the last effectively accepted Inventory, so
//! an Agent that edited `agent.toml` without bumping the revision kept
//! re-sending a report guaranteed to be refused while the projections froze.
//!
//! The record is written from the Report Receipt, inside the transaction that
//! applies it, and only when the Server effectively accepted the Inventory. It
//! is never derived from the Agent's own declaration, which is exactly what is
//! in doubt.

use std::fmt;
use std::path::Path;

use platpulse_core::inventory::NodeInventory;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Sqlite, SqliteConnection, Transaction};

/// The last Node Inventory the Server effectively accepted from this Agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryDeclaration {
    pub revision: u64,
    /// Canonical content hash of the accepted Inventory, as the Server stores
    /// it in `agents.inventory_sha256`.
    pub sha256: String,
    /// The report whose receipt made this declaration effective.
    pub report_id: String,
    pub adopted_at: String,
}

/// Why a declaration is refused before it is persisted or sent: the Server
/// would refuse it, and the remedy is a local configuration change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryDeclarationConflict {
    /// The content changed at a revision the Server already accepted. The
    /// Agent must bump `inventory_revision` to declare the new Node set.
    ContentChanged {
        revision: u64,
        recorded_sha256: String,
        declared_sha256: String,
    },
    /// The declared revision is below the last accepted revision; the Server
    /// would refuse it as stale.
    RevisionRegressed {
        declared_revision: u64,
        recorded_revision: u64,
    },
}

impl fmt::Display for InventoryDeclarationConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContentChanged { revision, .. } => write!(
                f,
                "inventory content changed but inventory_revision is still {revision}; bump inventory_revision (e.g. to {}) to declare the new Node set",
                revision.saturating_add(1)
            ),
            Self::RevisionRegressed {
                declared_revision,
                recorded_revision,
            } => write!(
                f,
                "inventory_revision {declared_revision} is below the last Node Inventory the Server accepted (revision {recorded_revision}); bump inventory_revision to at least {}",
                recorded_revision.saturating_add(1)
            ),
        }
    }
}

impl std::error::Error for InventoryDeclarationConflict {}

/// Failure while reading or evaluating the record. A conflict is a local
/// configuration error; a database error is a store failure and must keep the
/// caller's existing classification (a transient lock stays retryable).
#[derive(Debug, thiserror::Error)]
pub enum InventoryGuardError {
    #[error(transparent)]
    Conflict(#[from] InventoryDeclarationConflict),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Read the record, if this Agent has ever had an Inventory accepted.
pub(crate) async fn read_inventory_declaration(
    connection: &mut SqliteConnection,
) -> Result<Option<InventoryDeclaration>, sqlx::Error> {
    let row: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT revision, sha256, report_id, adopted_at FROM inventory_declaration WHERE singleton = 1",
    )
    .fetch_optional(connection)
    .await?;
    Ok(row.map(
        |(revision, sha256, report_id, adopted_at)| InventoryDeclaration {
            revision: revision.max(0) as u64,
            sha256,
            report_id,
            adopted_at,
        },
    ))
}

/// Record the accepted Inventory inside the receipt-application transaction.
///
/// The revision must not regress: a receipt that would move the record
/// backwards cannot come from a Server that accepted a newer declaration, and
/// keeping the high-water mark makes the record immune to any future
/// out-of-order receipt application.
pub(crate) async fn record_inventory_declaration(
    tx: &mut Transaction<'_, Sqlite>,
    inventory: &NodeInventory,
    report_id: &str,
    adopted_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO inventory_declaration (singleton, revision, sha256, report_id, adopted_at) VALUES (1, ?, ?, ?, ?) \
         ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision, sha256=excluded.sha256, report_id=excluded.report_id, adopted_at=excluded.adopted_at \
         WHERE excluded.revision >= inventory_declaration.revision",
    )
    .bind(inventory.revision as i64)
    .bind(inventory.content_sha256().to_string())
    .bind(report_id)
    .bind(adopted_at)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// Compare a declaration against the record. Pure, so every declaration site
/// and the read-only `validate-config` check share one rule.
///
/// No record means first run or an Agent that upgraded past this feature:
/// adopt silently. Refusing there would make every existing deployment stop
/// until an operator intervened, with no conflict to resolve.
pub(crate) fn guard_declaration(
    recorded: Option<&InventoryDeclaration>,
    declared: &NodeInventory,
) -> Result<(), InventoryDeclarationConflict> {
    let Some(recorded) = recorded else {
        return Ok(());
    };
    if declared.revision < recorded.revision {
        return Err(InventoryDeclarationConflict::RevisionRegressed {
            declared_revision: declared.revision,
            recorded_revision: recorded.revision,
        });
    }
    let declared_sha256 = declared.content_sha256().to_string();
    if declared.revision == recorded.revision && declared_sha256 != recorded.sha256 {
        return Err(InventoryDeclarationConflict::ContentChanged {
            revision: declared.revision,
            recorded_sha256: recorded.sha256.clone(),
            declared_sha256,
        });
    }
    Ok(())
}

/// The `validate-config` check: compare the configured Inventory against the
/// record in an existing Agent Store, strictly read-only.
///
/// `validate-config` must not create, migrate, or lock the Agent Store, so this
/// opens the database read-only and treats "nothing to compare against" as no
/// conflict: a missing file, an Agent Store older than the record table, and a
/// database another process is holding in a way a read-only reader cannot use
/// all mean the same thing here — this command has no evidence of a conflict.
pub async fn check_declaration_read_only(
    state_db: &Path,
    declared: &NodeInventory,
) -> Result<(), InventoryDeclarationConflict> {
    if !state_db.exists() {
        return Ok(());
    }
    let options = SqliteConnectOptions::new()
        .filename(state_db)
        .read_only(true);
    let Ok(mut connection) = SqliteConnection::connect_with(&options).await else {
        return Ok(());
    };
    let recorded = read_inventory_declaration(&mut connection)
        .await
        .ok()
        .flatten();
    guard_declaration(recorded.as_ref(), declared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{AgentDatabaseConfig, AgentStore};
    use platpulse_core::inventory::InventoryNode;
    use tempfile::tempdir;

    fn inventory(revision: u64, rpc_endpoint: &str) -> NodeInventory {
        NodeInventory {
            revision,
            nodes: vec![InventoryNode {
                node_id: "0195f2a1-2b3c-4d5e-8f90-123456789abc".parse().unwrap(),
                display_name: None,
                network_key: "platon-mainnet".parse().unwrap(),
                rpc_endpoint: rpc_endpoint.parse().unwrap(),
                process: None,
            }],
        }
    }

    async fn open_store(path: &Path) -> AgentStore {
        AgentStore::open(AgentDatabaseConfig::new(path))
            .await
            .unwrap()
    }

    async fn record(
        store: &mut AgentStore,
        inventory: &NodeInventory,
        report_id: &str,
        adopted_at: &str,
    ) -> Option<InventoryDeclaration> {
        let mut tx = store.connection().begin().await.unwrap();
        record_inventory_declaration(&mut tx, inventory, report_id, adopted_at)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        read_inventory_declaration(store.connection())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn record_follows_the_accepted_declaration_without_ever_regressing() {
        let dir = tempdir().unwrap();
        let mut store = open_store(&dir.path().join("agent.db")).await;

        // First run, or an Agent upgrading past this feature: nothing to
        // compare against, so the declaration is adopted silently.
        assert_eq!(
            read_inventory_declaration(store.connection())
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            guard_declaration(None, &inventory(4, "ws://127.0.0.1:6790")),
            Ok(())
        );

        let declared = inventory(4, "ws://127.0.0.1:6790");
        let recorded = record(&mut store, &declared, "report-1", "2026-01-01T00:00:00Z")
            .await
            .unwrap();
        assert_eq!(recorded.revision, 4);
        assert_eq!(recorded.sha256, declared.content_sha256().to_string());
        assert_eq!(recorded.report_id, "report-1");
        assert_eq!(recorded.adopted_at, "2026-01-01T00:00:00Z");

        // A bumped revision with new content moves the record.
        let bumped = inventory(5, "ws://127.0.0.1:6791");
        let recorded = record(&mut store, &bumped, "report-2", "2026-01-01T00:00:05Z")
            .await
            .unwrap();
        assert_eq!(recorded.revision, 5);
        assert_eq!(recorded.sha256, bumped.content_sha256().to_string());

        // A replayed older receipt can never move the record backwards: the
        // record is a high-water mark for the Agent's declaration.
        let recorded = record(
            &mut store,
            &inventory(4, "ws://127.0.0.1:6790"),
            "report-1-again",
            "2026-01-01T00:00:06Z",
        )
        .await
        .unwrap();
        assert_eq!(recorded.revision, 5);
        assert_eq!(recorded.report_id, "report-2");
        store.close().await.unwrap();
    }

    #[test]
    fn guard_refuses_changed_content_and_a_regressed_revision() {
        let recorded = InventoryDeclaration {
            revision: 4,
            sha256: inventory(4, "ws://127.0.0.1:6790")
                .content_sha256()
                .to_string(),
            report_id: "report-1".to_owned(),
            adopted_at: "2026-01-01T00:00:00Z".to_owned(),
        };

        // Re-declaring the identical Inventory at the identical revision is
        // what the Agent does on every collection tick: it must stay allowed.
        assert_eq!(
            guard_declaration(Some(&recorded), &inventory(4, "ws://127.0.0.1:6790")),
            Ok(())
        );

        let conflict = guard_declaration(Some(&recorded), &inventory(4, "ws://127.0.0.1:6791"))
            .expect_err("changed content at the accepted revision must be refused");
        assert_eq!(
            conflict.to_string(),
            "inventory content changed but inventory_revision is still 4; \
             bump inventory_revision (e.g. to 5) to declare the new Node set"
        );
        assert!(matches!(
            conflict,
            InventoryDeclarationConflict::ContentChanged { revision: 4, .. }
        ));

        let conflict = guard_declaration(Some(&recorded), &inventory(3, "ws://127.0.0.1:6790"))
            .expect_err("a regressed revision must be refused");
        assert!(matches!(
            conflict,
            InventoryDeclarationConflict::RevisionRegressed {
                declared_revision: 3,
                recorded_revision: 4,
            }
        ));
        assert!(
            conflict
                .to_string()
                .contains("below the last Node Inventory")
        );
    }

    /// `validate-config` must be able to answer "did I forget to bump?" and
    /// must never create or migrate the Agent Store to do it.
    #[tokio::test]
    async fn read_only_check_never_creates_the_store() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("agent.db");
        let declared = inventory(4, "ws://127.0.0.1:6790");

        assert_eq!(check_declaration_read_only(&path, &declared).await, Ok(()));
        assert!(
            !path.exists(),
            "the read-only check must not create the Agent Store"
        );

        let mut store = open_store(&path).await;
        record(&mut store, &declared, "report-1", "2026-01-01T00:00:00Z").await;
        store.close().await.unwrap();

        assert_eq!(check_declaration_read_only(&path, &declared).await, Ok(()));
        assert!(
            check_declaration_read_only(&path, &inventory(4, "ws://127.0.0.1:6791"))
                .await
                .is_err()
        );
        assert!(
            check_declaration_read_only(&path, &inventory(3, "ws://127.0.0.1:6790"))
                .await
                .is_err()
        );
    }
}
