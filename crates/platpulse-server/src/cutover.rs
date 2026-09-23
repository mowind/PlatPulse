//! Coordinated v1 -> v2 production cutover gate (issue #192).
//!
//! [ADR 0007](../../docs/adr/0007-server-managed-inventory-revision.md) and
//! [main design 15.10.4/15.10.5](../../docs/design/platpulse.md#server-managed-inventory-revision-target)
//! make the offline baseline conversion an authorization-free preparation step:
//! it must stay impossible to serve a converted deployment as if the switch had
//! happened. This module owns the single explicit switch gate.
//!
//! A converted deployment is one whose Server database carries the issue #191
//! conversion marker. Until the resume command writes the durable cutover
//! marker, the Server resolves InventoryCutover::AwaitingResume: it refuses
//! ordinary collection/ingestion, Admin mutations and every external-effect
//! worker, and reports itself not ready. The resume command re-verifies the
//! coordinated checkpoint (which itself re-binds the Agent preparation evidence
//! and the frozen-v1 declaration) and the offline conversion (which re-derives
//! the v2 fingerprint and proves both halves agree) before it accepts the
//! converted Server database as the configured one. A missing, changed or
//! inconsistent participant stops the switch; a converted database can never be
//! served through a shortcut.
//!
//! Once resumed, ordinary Reports are v2-only and the frozen v1 route becomes
//! replay-only (see http::report_ingestion). Before that point the coordinated
//! checkpoint can still be restored wholesale (old Server database, Agent Store
//! and configuration); after it, restoring the checkpoint is a separate
//! disaster-recovery action, never a software rollback, and this module refuses
//! to present it as one.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sqlx::{Connection, SqliteConnection, SqlitePool};
use thiserror::Error;

use crate::checkpoint::{CHECKPOINT_MANIFEST, CheckpointSummary, SERVER_DIRECTORY};
use crate::config::ServerConfig;
use crate::migration::{ConversionSummary, MIGRATION_MANIFEST};

/// Runtime Inventory protocol mode derived from the durable markers.
///
/// The mode is resolved once when the Server database opens and is fixed for
/// the process lifetime: resuming the cutover is a stopped-Server operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryCutover {
    /// No conversion marker: an ordinary deployment. Frozen v1 ingestion stays
    /// ordinary and the v2 route remains available for opt-in deployments.
    NotConverted,
    /// A converted deployment that has not passed the explicit switch gate.
    /// Business writes (ingestion, Admin mutations, external-effect workers)
    /// are refused until the resume command authorizes them.
    AwaitingResume,
    /// The coordinated cutover is resumed. Ordinary Reports are v2-only and the
    /// frozen v1 route replays retained Receipts only.
    Resumed,
}

impl InventoryCutover {
    /// Whether the frozen v1 route is restricted to exact replay.
    pub fn v1_replay_only(self) -> bool {
        matches!(self, Self::Resumed)
    }

    /// Whether business writes are refused pending the switch gate.
    pub fn business_writes_blocked(self) -> bool {
        matches!(self, Self::AwaitingResume)
    }

    /// Stable operator-facing name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotConverted => "not_converted",
            Self::AwaitingResume => "awaiting_resume",
            Self::Resumed => "resumed",
        }
    }
}

/// The durable cutover binding read back for diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutoverBinding {
    pub state: String,
    pub resumed_at: String,
    pub checkpoint_manifest_sha256: String,
    pub conversion_manifest_sha256: String,
    pub agent_id: String,
    pub protocol_major: i64,
    pub preserved_revision: i64,
    pub previous_sha256: Option<String>,
    pub fingerprint_sha256: Option<String>,
}

/// Sanitized result of one cutover command. Never contains a secret.
#[derive(Debug, Clone)]
pub struct CutoverSummary {
    pub mode: InventoryCutover,
    pub agent_id: Option<String>,
    pub preserved_revision: Option<i64>,
    pub previous_sha256: Option<String>,
    pub fingerprint_sha256: Option<String>,
    pub resumed_at: Option<String>,
    pub deleted_nodes: usize,
}

/// The current cutover state of one configured deployment.
#[derive(Debug, Clone)]
pub struct CutoverStatus {
    pub mode: InventoryCutover,
    pub binding: Option<CutoverBinding>,
}

#[derive(Debug, Error)]
pub enum CutoverError {
    #[error(transparent)]
    Ownership(#[from] crate::ownership::OwnershipError),
    #[error(transparent)]
    Checkpoint(#[from] crate::checkpoint::CheckpointError),
    #[error(transparent)]
    Migration(#[from] crate::migration::MigrationError),
    #[error("cutover database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("cutover IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cutover JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the cutover cannot be resumed: {0}")]
    Invalid(String),
    #[error("the cutover state changed: {0}")]
    StateChanged(String),
    #[error(
        "business writes have already resumed; restoring the coordinated checkpoint is a separate disaster-recovery action, not a software rollback: {0}"
    )]
    ForwardRepairRequired(String),
}

#[derive(Debug, sqlx::FromRow)]
struct MigrationMarkerRow {
    protocol_major: i64,
    preserved_revision: i64,
    previous_sha256: Option<String>,
    fingerprint_sha256: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ConvertedAgentRow {
    last_inventory_revision: i64,
    inventory_sha256: Option<String>,
    inventory_protocol_major: Option<i64>,
    active_boot_id: Option<String>,
    active_boot_status: String,
    close_report_id: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct CutoverRow {
    state: String,
    resumed_at: String,
    checkpoint_manifest_sha256: String,
    conversion_manifest_sha256: String,
    agent_id: String,
    protocol_major: i64,
    preserved_revision: i64,
    previous_sha256: Option<String>,
    fingerprint_sha256: Option<String>,
}

/// Resolve the runtime Inventory protocol mode from a Server database pool.
pub async fn detect_inventory_cutover(pool: &SqlitePool) -> Result<InventoryCutover, sqlx::Error> {
    let migrated_table: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'inventory_migration'",
    )
    .fetch_optional(pool)
    .await?;
    let converted: i64 = match migrated_table {
        Some(_) => {
            sqlx::query_scalar("SELECT COUNT(*) FROM inventory_migration")
                .fetch_one(pool)
                .await?
        }
        None => 0,
    };
    if converted == 0 {
        return Ok(InventoryCutover::NotConverted);
    }
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM inventory_cutover WHERE singleton = 1")
            .fetch_optional(pool)
            .await?;
    Ok(if state.as_deref() == Some("resumed") {
        InventoryCutover::Resumed
    } else {
        InventoryCutover::AwaitingResume
    })
}

/// Report the cutover state of one configured deployment without mutating it.
///
/// This is a read-only diagnostic: it starts no worker and writes nothing, so
/// it is safe to run against a stopped deployment (and, in development mode,
/// against a running dev Server).
pub async fn cutover_status(config: &ServerConfig) -> Result<CutoverStatus, CutoverError> {
    let mut connection = open_read_only(&config.db_path).await?;
    let name: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'inventory_migration'",
    )
    .fetch_optional(&mut connection)
    .await?;
    let converted: i64 = match name {
        Some(_) => {
            sqlx::query_scalar("SELECT COUNT(*) FROM inventory_migration")
                .fetch_one(&mut connection)
                .await?
        }
        None => 0,
    };
    let binding: Option<CutoverRow> = if converted == 0 {
        None
    } else {
        sqlx::query_as(
            "SELECT state, resumed_at, checkpoint_manifest_sha256, conversion_manifest_sha256, agent_id, protocol_major, preserved_revision, previous_sha256, fingerprint_sha256 FROM inventory_cutover WHERE singleton = 1",
        )
        .fetch_optional(&mut connection)
        .await?
    };
    connection.close().await?;
    let mode = match (&binding, converted) {
        (Some(row), _) if row.state == "resumed" => InventoryCutover::Resumed,
        _ if converted > 0 => InventoryCutover::AwaitingResume,
        _ => InventoryCutover::NotConverted,
    };
    Ok(CutoverStatus {
        mode,
        binding: binding.map(|row| CutoverBinding {
            state: row.state,
            resumed_at: row.resumed_at,
            checkpoint_manifest_sha256: row.checkpoint_manifest_sha256,
            conversion_manifest_sha256: row.conversion_manifest_sha256,
            agent_id: row.agent_id,
            protocol_major: row.protocol_major,
            preserved_revision: row.preserved_revision,
            previous_sha256: row.previous_sha256,
            fingerprint_sha256: row.fingerprint_sha256,
        }),
    })
}

/// Pass the explicit switch gate for a verified converted deployment.
///
/// The configured database must be the converted one. The command re-verifies
/// the coordinated checkpoint (issue #190) and the offline conversion (issue
/// #191), proves the converted Server baseline/Boot linkage still match, and
/// only then writes the durable cutover marker. Re-running with the same
/// binding is idempotent; any other source is refused.
pub async fn resume_cutover(
    config: &ServerConfig,
    checkpoint: &Path,
    converted: &Path,
) -> Result<CutoverSummary, CutoverError> {
    crate::init::restrict_umask();
    // Resuming the cutover is a stopped-Server operation: it takes the
    // exclusive ownership guard before SQLite opens, like every other offline
    // command. A running Server has no business writing here.
    let _ownership = crate::ownership::acquire_for_deployment(&config.db_path, config.development)?;

    // The gate consumes every prior result. verify_checkpoint re-binds the
    // Agent preparation evidence (final Closing, frozen-v1 declaration,
    // revision-inclusive hash) and both preserved halves; verify_conversion
    // re-derives the v2 canonical fingerprint and proves the converted Server
    // baseline and Agent Declaration Record agree.
    let verified = crate::checkpoint::verify_checkpoint(checkpoint).await?;
    let conversion = crate::migration::verify_conversion(checkpoint, converted).await?;

    let converted_db = converted.join(SERVER_DIRECTORY).join("server.db");
    if !same_file(&config.db_path, &converted_db)? {
        return Err(CutoverError::StateChanged(format!(
            "the configured database {} is not the verified converted database {}; point --config at the converted deployment instead of swapping only the binary",
            config.db_path.display(),
            converted_db.display()
        )));
    }

    let checkpoint_manifest_sha256 =
        hex_sha256(&std::fs::read(checkpoint.join(CHECKPOINT_MANIFEST))?);
    let conversion_manifest_sha256 =
        hex_sha256(&std::fs::read(converted.join(MIGRATION_MANIFEST))?);
    let now = crate::auth::format_rfc3339(crate::auth::now_utc());

    let mut connection = open_read_write(&converted_db).await?;
    let result = resume_inner(
        &mut connection,
        &verified,
        &conversion,
        &checkpoint_manifest_sha256,
        &conversion_manifest_sha256,
        &now,
    )
    .await;
    let close = connection.close().await;
    match result {
        Ok(summary) => {
            close?;
            Ok(summary)
        }
        Err(error) => Err(error),
    }
}

async fn resume_inner(
    connection: &mut SqliteConnection,
    verified: &CheckpointSummary,
    conversion: &ConversionSummary,
    checkpoint_manifest_sha256: &str,
    conversion_manifest_sha256: &str,
    now: &str,
) -> Result<CutoverSummary, CutoverError> {
    let mut tx = connection.begin().await?;
    let marker: Option<MigrationMarkerRow> = sqlx::query_as(
        "SELECT protocol_major, preserved_revision, previous_sha256, fingerprint_sha256 FROM inventory_migration WHERE singleton = 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(marker) = marker else {
        return Err(CutoverError::Invalid(
            "the deployment has no Inventory conversion marker".to_owned(),
        ));
    };
    if marker.protocol_major != 2
        || marker.preserved_revision != conversion.inventory_revision
        || marker.previous_sha256 != conversion.previous_sha256
        || marker.fingerprint_sha256 != conversion.fingerprint_sha256
    {
        return Err(CutoverError::StateChanged(
            "the converted Server conversion marker does not match the verified conversion"
                .to_owned(),
        ));
    }
    let agent: Option<ConvertedAgentRow> = sqlx::query_as(
        "SELECT last_inventory_revision, inventory_sha256, inventory_protocol_major, active_boot_id, active_boot_status, close_report_id FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
    )
    .bind(&verified.agent_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(agent) = agent else {
        return Err(CutoverError::StateChanged(
            "the converted Server has no live Agent row".to_owned(),
        ));
    };
    if agent.protocol_row_mismatch(conversion)
        || agent.active_boot_id.as_deref() != Some(verified.closed_boot_id.as_str())
        || agent.active_boot_status != "closed"
        || agent.close_report_id.as_deref() != Some(verified.closing_report_id.as_str())
    {
        return Err(CutoverError::StateChanged(
            "the converted Server baseline, Boot linkage or Closing identity changed after verification"
                .to_owned(),
        ));
    }

    let existing: Option<CutoverRow> = sqlx::query_as(
        "SELECT state, resumed_at, checkpoint_manifest_sha256, conversion_manifest_sha256, agent_id, protocol_major, preserved_revision, previous_sha256, fingerprint_sha256 FROM inventory_cutover WHERE singleton = 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(existing) = existing {
        let same_source = existing.state == "resumed"
            && existing.checkpoint_manifest_sha256 == checkpoint_manifest_sha256
            && existing.conversion_manifest_sha256 == conversion_manifest_sha256
            && existing.agent_id == verified.agent_id
            && existing.preserved_revision == conversion.inventory_revision
            && existing.previous_sha256 == conversion.previous_sha256
            && existing.fingerprint_sha256 == conversion.fingerprint_sha256;
        if !same_source {
            return Err(CutoverError::StateChanged(
                "the deployment is already bound to a different coordinated cutover".to_owned(),
            ));
        }
        tx.commit().await?;
        return Ok(summary_of(
            conversion,
            verified.deleted_nodes,
            Some(existing.resumed_at),
        ));
    }

    let expected_protocol_major = if conversion.uninitialized {
        None
    } else {
        Some(2)
    };
    if agent.inventory_protocol_major != expected_protocol_major {
        return Err(CutoverError::StateChanged(
            "the converted Server protocol marker does not match the verified conversion"
                .to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO inventory_cutover (singleton, state, resumed_at, checkpoint_manifest_sha256, conversion_manifest_sha256, agent_id, protocol_major, preserved_revision, previous_sha256, fingerprint_sha256) VALUES (1, 'resumed', ?, ?, ?, ?, 2, ?, ?, ?)",
    )
    .bind(now)
    .bind(checkpoint_manifest_sha256)
    .bind(conversion_manifest_sha256)
    .bind(&verified.agent_id)
    .bind(conversion.inventory_revision)
    .bind(conversion.previous_sha256.as_deref())
    .bind(conversion.fingerprint_sha256.as_deref())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(summary_of(
        conversion,
        verified.deleted_nodes,
        Some(now.to_owned()),
    ))
}

impl ConvertedAgentRow {
    fn protocol_row_mismatch(&self, conversion: &ConversionSummary) -> bool {
        let expected_protocol_major = if conversion.uninitialized {
            None
        } else {
            Some(2)
        };
        self.last_inventory_revision != conversion.inventory_revision
            || self.inventory_sha256 != conversion.fingerprint_sha256
            || self.inventory_protocol_major != expected_protocol_major
    }
}

/// Roll back a converted deployment before business writes resume.
///
/// The source checkpoint is re-verified and restored wholesale (old Server
/// database, Agent Store, identity/credential material and configuration) into
/// an isolated directory, and the restored copy is proven to be the same closed
/// old Boot awaiting its new Boot linkage. If the cutover already resumed, this
/// refuses: restoring the checkpoint then is a separate disaster-recovery action
/// that may lose post-cutover declarations, Purge barriers and other writes, and
/// must not be presented as a safe software rollback.
pub async fn rollback_cutover(
    checkpoint: &Path,
    restore_dir: &Path,
    converted: Option<&Path>,
) -> Result<CheckpointSummary, CutoverError> {
    crate::init::restrict_umask();
    let _verified = crate::checkpoint::verify_checkpoint(checkpoint).await?;
    if let Some(converted) = converted {
        let db = converted.join(SERVER_DIRECTORY).join("server.db");
        if db.exists() {
            let state = read_cutover_state(&db).await?;
            if state.as_deref() == Some("resumed") {
                return Err(CutoverError::ForwardRepairRequired(
                    "the coordinated cutover already resumed business writes; repair forward instead of restoring the checkpoint".to_owned(),
                ));
            }
        }
    }
    let summary = crate::checkpoint::restore_checkpoint(checkpoint, restore_dir).await?;
    Ok(summary)
}

async fn read_cutover_state(path: &Path) -> Result<Option<String>, CutoverError> {
    let mut connection = open_read_only(path).await?;
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'inventory_cutover'",
    )
    .fetch_optional(&mut connection)
    .await?;
    let state = if exists.is_some() {
        sqlx::query_scalar("SELECT state FROM inventory_cutover WHERE singleton = 1")
            .fetch_optional(&mut connection)
            .await?
    } else {
        None
    };
    connection.close().await?;
    Ok(state)
}

fn summary_of(
    conversion: &ConversionSummary,
    deleted_nodes: usize,
    resumed_at: Option<String>,
) -> CutoverSummary {
    CutoverSummary {
        mode: InventoryCutover::Resumed,
        agent_id: Some(conversion.agent_id.clone()),
        preserved_revision: Some(conversion.inventory_revision),
        previous_sha256: conversion.previous_sha256.clone(),
        fingerprint_sha256: conversion.fingerprint_sha256.clone(),
        resumed_at,
        deleted_nodes,
    }
}

fn same_file(left: &Path, right: &Path) -> Result<bool, std::io::Error> {
    Ok(std::fs::canonicalize(left)? == std::fs::canonicalize(right)?)
}

async fn open_read_write(path: &Path) -> Result<SqliteConnection, CutoverError> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false);
    SqliteConnection::connect_with(&options)
        .await
        .map_err(CutoverError::from)
}

async fn open_read_only(path: &Path) -> Result<SqliteConnection, CutoverError> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    SqliteConnection::connect_with(&options)
        .await
        .map_err(CutoverError::from)
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
