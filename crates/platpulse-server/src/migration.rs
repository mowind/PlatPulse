//! Offline Server-managed Inventory baseline conversion (issue #191).
//!
//! [ADR 0007](../../docs/adr/0007-server-managed-inventory-revision.md) and
//! [main design 15.10.4](../../docs/design/platpulse.md#server-managed-inventory-revision-target)
//! require the frozen-v1 -> v2 cutover to convert, from a *verified coordinated
//! checkpoint*, the Server accepted baseline and the Agent Inventory
//! Declaration Record to the v2 canonical declaration fingerprint while keeping
//! the already-accepted revision. The conversion is offline: it re-verifies the
//! checkpoint, writes a converted deployment into a new directory, and never
//! touches the live Server database, the live Agent Store or the original
//! checkpoint. It starts no collector, ingestion path, Admin mutation or
//! external-effect worker.
//!
//! There is deliberately no atomic transaction across the two independent
//! SQLite databases. Instead every participant is converted into an isolated
//! copy that is only accepted after verify_conversion re-derives the v2
//! fingerprint from the source checkpoint and proves both converted halves
//! agree. A failure leaves the checkpoint and the live deployment untouched.
//!
//! The two hash algorithms are explicit: the frozen v1 accepted hash is
//! sha256(json(NodeInventory{revision, nodes})) (revision-inclusive,
//! declaration-order-sensitive), while the v2 fingerprint is
//! sha256(json({declaration_version: 2, nodes sorted by Node ID})). The
//! conversion never rewrites an old Report or a retained Receipt, never
//! re-enrolls, and never deletes the spool to repair a failure.

use std::path::{Path, PathBuf};

use platpulse_core::inventory::{InventoryDeclaration, NodeInventory};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, SqliteConnection};
use thiserror::Error;

use crate::checkpoint::{
    AGENT_DIRECTORY, AGENT_MANIFEST, AgentCheckpointManifest, CHECKPOINT_FORMAT_VERSION,
    CHECKPOINT_KIND, CHECKPOINT_MANIFEST, CoordinationCheckpointManifest, ROLE_SERVER_DB,
    ROLE_SERVER_PEPPER, ROLE_SERVER_TLS_KEY, SERVER_DIRECTORY,
};

/// Manifest kind discriminator for one offline conversion.
pub const MIGRATION_KIND: &str = "platpulse.upgrade-conversion";

/// Format version of the conversion manifest.
pub const MIGRATION_FORMAT_VERSION: u32 = 1;

/// Conversion manifest file name.
pub const MIGRATION_MANIFEST: &str = "migration.json";

/// Artifact role of the exact Agent Store snapshot inside the Agent half.
const AGENT_ROLE_STORE: &str = "agent-store";

/// The v1 revision-inclusive accepted-hash algorithm, documented to operators.
pub const PREVIOUS_ALGORITHM: &str = "sha256(json(NodeInventory{revision,nodes})): frozen v1, revision-inclusive and declaration-order-sensitive";

/// The v2 canonical declaration fingerprint algorithm.
pub const CONVERTED_ALGORITHM: &str = "sha256(json({declaration_version:2,nodes sorted by node_id})): v2 canonical declaration fingerprint, revision-excluded and order-independent";

/// DDL for the Agent-side conversion marker. Must match Agent migration 0016.
const AGENT_MIGRATION_DDL: &str = "CREATE TABLE IF NOT EXISTS inventory_migration (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), protocol_major INTEGER NOT NULL CHECK (protocol_major >= 1), preserved_revision INTEGER NOT NULL CHECK (preserved_revision >= 0), previous_sha256 TEXT, fingerprint_sha256 TEXT, converted_at TEXT NOT NULL)";

#[derive(Debug, sqlx::FromRow)]
struct ConvertedAgentRow {
    last_inventory_revision: i64,
    inventory_sha256: Option<String>,
    inventory_protocol_major: Option<i64>,
    active_boot_id: Option<String>,
    active_boot_status: String,
    close_report_id: Option<String>,
    last_report_sequence: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct MarkerRow {
    protocol_major: i64,
    preserved_revision: i64,
    previous_sha256: Option<String>,
    fingerprint_sha256: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ClosingReceiptRow {
    disposition: String,
    report_body_sha256: String,
    receipt_body: Vec<u8>,
}

#[derive(Debug, sqlx::FromRow)]
struct ConvertedAgentStateRow {
    agent_id: Option<String>,
    agent_epoch: i64,
    boot_id: Option<String>,
    boot_state: String,
    previous_boot_id: Option<String>,
    pending_transition: Option<String>,
    pending_previous_boot_id: Option<String>,
    close_report_id: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct DeclarationRecordRow {
    revision: i64,
    sha256: String,
}

/// The converted baseline derived from one verified frozen-v1 declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineConversion {
    /// The accepted revision, preserved exactly (for example 57 stays 57).
    pub preserved_revision: i64,
    /// The frozen v1 revision-inclusive hash, when one was accepted.
    pub previous_sha256: Option<String>,
    /// The v2 canonical declaration fingerprint, when one was accepted.
    pub fingerprint_sha256: Option<String>,
    /// A legitimate never-accepted Agent: nothing to convert, nothing invented.
    pub uninitialized: bool,
}

impl BaselineConversion {
    /// The Server protocol major that must describe the converted baseline.
    /// An Agent that never accepted an Inventory keeps an unknown/NULL protocol.
    pub fn expected_protocol_major(&self) -> Option<i64> {
        if self.uninitialized { None } else { Some(2) }
    }
}

/// The persisted conversion manifest binding both converted halves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionManifest {
    pub kind: String,
    pub format_version: u32,
    pub created_at: String,
    pub server_version: String,
    /// SHA-256 of the source coordination manifest, binding the derivation.
    pub checkpoint_manifest_sha256: String,
    pub agent_id: String,
    pub agent_epoch: i64,
    pub closed_boot_id: String,
    pub next_boot_id: String,
    pub pending_transition: String,
    pub closing_report_id: String,
    pub closing_report_sequence: i64,
    pub closing_receipt_disposition: String,
    pub inventory_revision: i64,
    pub previous_sha256: Option<String>,
    pub fingerprint_sha256: Option<String>,
    pub uninitialized: bool,
    pub previous_algorithm: String,
    pub converted_algorithm: String,
    pub agent_directory: String,
    pub server_directory: String,
}

/// Sanitized summary of a converted deployment (never a secret).
#[derive(Debug, Clone)]
pub struct ConversionSummary {
    pub directory: PathBuf,
    pub agent_id: String,
    pub closed_boot_id: String,
    pub next_boot_id: String,
    pub inventory_revision: i64,
    pub previous_sha256: Option<String>,
    pub fingerprint_sha256: Option<String>,
    pub uninitialized: bool,
    pub deleted_nodes: usize,
}

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error(transparent)]
    Checkpoint(#[from] crate::checkpoint::CheckpointError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("migration JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the conversion input is invalid: {0}")]
    Invalid(String),
    #[error("the conversion output directory is unsafe or already exists: {0}")]
    UnsafeDirectory(String),
    #[error("the conversion state changed: {0}")]
    StateChanged(String),
}

/// Convert a verified frozen-v1 baseline into the v2 canonical baseline.
///
/// Pure and side-effect free so it can be checked against the uninitialized,
/// accepted-empty and normal cases. A never-accepted Agent stays uninitialized:
/// this function never invents an accepted empty declaration.
pub fn convert_baseline(
    revision: i64,
    inventory_sha256: Option<&str>,
    declaration: &NodeInventory,
) -> Result<BaselineConversion, MigrationError> {
    match inventory_sha256 {
        None => {
            if revision != 0 {
                return Err(MigrationError::Invalid(format!(
                    "the Server records accepted revision {revision} with no accepted Inventory hash; the baseline evidence is incomplete"
                )));
            }
            Ok(BaselineConversion {
                preserved_revision: 0,
                previous_sha256: None,
                fingerprint_sha256: None,
                uninitialized: true,
            })
        }
        Some(hash) => {
            if revision < 1 {
                return Err(MigrationError::Invalid(format!(
                    "the Server records accepted Inventory hash {hash} at invalid revision {revision}; refusing to truncate or renumber it"
                )));
            }
            let recomputed = declaration.content_sha256().to_string();
            if recomputed != hash {
                return Err(MigrationError::Invalid(
                    "the verified declaration does not hash to the Server accepted v1 Inventory hash".to_owned(),
                ));
            }
            let fingerprint = InventoryDeclaration {
                nodes: declaration.nodes.clone(),
            }
            .fingerprint()
            .to_string();
            Ok(BaselineConversion {
                preserved_revision: revision,
                previous_sha256: Some(hash.to_owned()),
                fingerprint_sha256: Some(fingerprint),
                uninitialized: false,
            })
        }
    }
}

/// Convert a verified coordinated checkpoint into an isolated v2 deployment.
pub async fn convert_checkpoint(
    checkpoint: &Path,
    output: &Path,
) -> Result<ConversionSummary, MigrationError> {
    crate::init::restrict_umask();
    // Full re-verification of the source: identity, successful Closing, the
    // frozen v1 declaration revision/hash and both baselines. Reuses issue
    // #190 and touches no live database.
    let _verified = crate::checkpoint::verify_checkpoint(checkpoint).await?;
    let (checkpoint_manifest, agent_manifest, conversion) =
        load_verified_inputs(checkpoint).await?;

    let absolute_output = std::path::absolute(output)?;
    let absolute_checkpoint = std::path::absolute(checkpoint)?;
    if absolute_output.starts_with(&absolute_checkpoint)
        || absolute_checkpoint.starts_with(&absolute_output)
    {
        return Err(MigrationError::UnsafeDirectory(
            "the conversion output must be separate from the source checkpoint".to_owned(),
        ));
    }

    let created_directory = prepare_directory(output)?;
    let result = convert_inner(
        checkpoint,
        output,
        &checkpoint_manifest,
        &agent_manifest,
        &conversion,
    )
    .await;
    if result.is_err() {
        cleanup_directory(output, created_directory);
    }
    result
}

/// Re-verify the converted deployment offline and prove it matches the source.
///
/// No live database, collector, ingestion path, Admin mutation or
/// external-effect worker is touched.
pub async fn verify_conversion(
    checkpoint: &Path,
    converted: &Path,
) -> Result<ConversionSummary, MigrationError> {
    let _verified = crate::checkpoint::verify_checkpoint(checkpoint).await?;
    let (checkpoint_manifest, _agent_manifest, conversion) =
        load_verified_inputs(checkpoint).await?;

    let manifest_bytes = std::fs::read(converted.join(MIGRATION_MANIFEST)).map_err(|error| {
        MigrationError::Invalid(format!("cannot read the conversion manifest: {error}"))
    })?;
    let manifest: ConversionManifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.kind != MIGRATION_KIND || manifest.format_version != MIGRATION_FORMAT_VERSION {
        return Err(MigrationError::Invalid(format!(
            "unexpected conversion kind {} format {}",
            manifest.kind, manifest.format_version
        )));
    }
    let checkpoint_manifest_sha256 =
        hex_sha256(&std::fs::read(checkpoint.join(CHECKPOINT_MANIFEST))?);
    require_manifest_binding(
        &manifest,
        &checkpoint_manifest,
        &conversion,
        &checkpoint_manifest_sha256,
    )?;

    let server_directory = converted.join(&manifest.server_directory);
    let agent_directory = converted.join(&manifest.agent_directory);
    verify_converted_server(&server_directory, &checkpoint_manifest, &conversion).await?;
    verify_converted_agent(&agent_directory, &checkpoint_manifest, &conversion).await?;
    verify_converted_configs(&server_directory, &agent_directory)?;

    Ok(summary_of(
        &manifest,
        converted,
        checkpoint_manifest.server.deleted_node_ids.len(),
    ))
}

async fn load_verified_inputs(
    checkpoint: &Path,
) -> Result<
    (
        CoordinationCheckpointManifest,
        AgentCheckpointManifest,
        BaselineConversion,
    ),
    MigrationError,
> {
    let checkpoint_manifest: CoordinationCheckpointManifest =
        serde_json::from_slice(&std::fs::read(checkpoint.join(CHECKPOINT_MANIFEST))?)?;
    if checkpoint_manifest.kind != CHECKPOINT_KIND
        || checkpoint_manifest.format_version != CHECKPOINT_FORMAT_VERSION
    {
        return Err(MigrationError::Invalid(
            "the source is not a coordinated upgrade checkpoint".to_owned(),
        ));
    }
    let agent_manifest: AgentCheckpointManifest = serde_json::from_slice(&std::fs::read(
        checkpoint
            .join(&checkpoint_manifest.agent_directory)
            .join(AGENT_MANIFEST),
    )?)?;
    let declaration: NodeInventory = serde_json::from_str(&agent_manifest.declaration_json)?;
    let conversion = convert_baseline(
        checkpoint_manifest.server.accepted_inventory_revision,
        checkpoint_manifest
            .server
            .accepted_inventory_sha256
            .as_deref(),
        &declaration,
    )?;
    Ok((checkpoint_manifest, agent_manifest, conversion))
}

fn require_manifest_binding(
    manifest: &ConversionManifest,
    checkpoint_manifest: &CoordinationCheckpointManifest,
    conversion: &BaselineConversion,
    checkpoint_manifest_sha256: &str,
) -> Result<(), MigrationError> {
    if manifest.checkpoint_manifest_sha256 != checkpoint_manifest_sha256
        || manifest.agent_id != checkpoint_manifest.agent.agent_id
        || manifest.agent_epoch != checkpoint_manifest.agent.agent_epoch
        || manifest.closed_boot_id != checkpoint_manifest.agent.closed_boot_id
        || manifest.next_boot_id != checkpoint_manifest.agent.next_boot_id
        || manifest.pending_transition != checkpoint_manifest.agent.pending_transition
        || manifest.closing_report_id != checkpoint_manifest.agent.closing_report_id
        || manifest.closing_report_sequence != checkpoint_manifest.agent.closing_report_sequence
        || manifest.closing_receipt_disposition
            != checkpoint_manifest.agent.closing_receipt_disposition
        || manifest.inventory_revision != conversion.preserved_revision
        || manifest.previous_sha256 != conversion.previous_sha256
        || manifest.fingerprint_sha256 != conversion.fingerprint_sha256
        || manifest.uninitialized != conversion.uninitialized
        || manifest.previous_algorithm != PREVIOUS_ALGORITHM
        || manifest.converted_algorithm != CONVERTED_ALGORITHM
    {
        return Err(MigrationError::Invalid(
            "the conversion manifest does not match the source checkpoint".to_owned(),
        ));
    }
    Ok(())
}

async fn convert_inner(
    checkpoint: &Path,
    output: &Path,
    checkpoint_manifest: &CoordinationCheckpointManifest,
    agent_manifest: &AgentCheckpointManifest,
    conversion: &BaselineConversion,
) -> Result<ConversionSummary, MigrationError> {
    let created_at = crate::auth::format_rfc3339(crate::auth::now_utc());
    let source_server = checkpoint.join(&checkpoint_manifest.server_directory);
    let source_agent = checkpoint.join(&checkpoint_manifest.agent_directory);
    let target_server = output.join(SERVER_DIRECTORY);
    let target_agent = output.join(AGENT_DIRECTORY);
    create_private_directory(&target_server)?;
    create_private_directory(&target_agent)?;

    copy_artifact(
        &source_server,
        &checkpoint_manifest.server.artifacts,
        ROLE_SERVER_DB,
        &target_server.join("server.db"),
    )?;
    copy_artifact(
        &source_server,
        &checkpoint_manifest.server.artifacts,
        ROLE_SERVER_PEPPER,
        &target_server.join("server-pepper"),
    )?;
    let tls_key_copied = checkpoint_manifest
        .server
        .artifacts
        .iter()
        .any(|artifact| artifact.role == ROLE_SERVER_TLS_KEY);
    if tls_key_copied {
        copy_artifact(
            &source_server,
            &checkpoint_manifest.server.artifacts,
            ROLE_SERVER_TLS_KEY,
            &target_server.join("tls-private-key.pem"),
        )?;
    }

    let converted_server_db = target_server.join("server.db");
    convert_server_database(
        &converted_server_db,
        &checkpoint_manifest.agent.agent_id,
        conversion,
        &created_at,
    )
    .await?;

    copy_artifact(
        &source_agent,
        &agent_manifest.artifacts,
        AGENT_ROLE_STORE,
        &target_agent.join("agent-store.sqlite"),
    )?;
    copy_file_private(
        &source_agent.join("credential"),
        &target_agent.join("credential"),
    )?;
    let converted_agent_store = target_agent.join("agent-store.sqlite");
    convert_agent_store(
        &converted_agent_store,
        &checkpoint_manifest.agent.closing_report_id,
        conversion,
        &created_at,
    )
    .await?;

    write_converted_agent_config(&source_agent, &target_agent)?;
    write_converted_server_config(&source_server, &target_server, tls_key_copied)?;

    let manifest = ConversionManifest {
        kind: MIGRATION_KIND.to_owned(),
        format_version: MIGRATION_FORMAT_VERSION,
        created_at,
        server_version: crate::VERSION.to_owned(),
        checkpoint_manifest_sha256: hex_sha256(&std::fs::read(
            checkpoint.join(CHECKPOINT_MANIFEST),
        )?),
        agent_id: checkpoint_manifest.agent.agent_id.clone(),
        agent_epoch: checkpoint_manifest.agent.agent_epoch,
        closed_boot_id: checkpoint_manifest.agent.closed_boot_id.clone(),
        next_boot_id: checkpoint_manifest.agent.next_boot_id.clone(),
        pending_transition: checkpoint_manifest.agent.pending_transition.clone(),
        closing_report_id: checkpoint_manifest.agent.closing_report_id.clone(),
        closing_report_sequence: checkpoint_manifest.agent.closing_report_sequence,
        closing_receipt_disposition: checkpoint_manifest
            .agent
            .closing_receipt_disposition
            .clone(),
        inventory_revision: conversion.preserved_revision,
        previous_sha256: conversion.previous_sha256.clone(),
        fingerprint_sha256: conversion.fingerprint_sha256.clone(),
        uninitialized: conversion.uninitialized,
        previous_algorithm: PREVIOUS_ALGORITHM.to_owned(),
        converted_algorithm: CONVERTED_ALGORITHM.to_owned(),
        agent_directory: AGENT_DIRECTORY.to_owned(),
        server_directory: SERVER_DIRECTORY.to_owned(),
    };
    write_private(
        &output.join(MIGRATION_MANIFEST),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;

    // Self-check: the produced deployment must pass the same offline
    // verification an operator runs, so a partially written result can never
    // be mistaken for a validated conversion.
    verify_conversion(checkpoint, output).await
}

async fn convert_server_database(
    path: &Path,
    agent_id: &str,
    conversion: &BaselineConversion,
    converted_at: &str,
) -> Result<(), MigrationError> {
    let mut connection = open_read_write(path).await?;
    // Apply the marker migration to the converted copy. The source checkpoint
    // is never opened.
    if let Err(error) = crate::database::SERVER_MIGRATOR
        .run_direct(&mut connection)
        .await
    {
        connection.close().await?;
        return Err(MigrationError::Database(sqlx::Error::Migrate(Box::new(
            error,
        ))));
    }
    let mut tx = connection.begin().await?;
    if conversion.uninitialized {
        // Never invent an accepted empty declaration: only the protocol
        // marker is cleared for an Agent that has no accepted Inventory.
        sqlx::query(
            "UPDATE agents SET inventory_protocol_major = NULL WHERE agent_id = ? AND deleted_at IS NULL AND inventory_sha256 IS NULL",
        )
        .bind(agent_id)
        .execute(&mut *tx)
        .await?;
    } else {
        let updated = sqlx::query(
            "UPDATE agents SET inventory_sha256 = ?, inventory_protocol_major = 2 WHERE agent_id = ? AND deleted_at IS NULL",
        )
        .bind(conversion.fingerprint_sha256.as_deref())
        .bind(agent_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(MigrationError::StateChanged(
                "the converted Server database has no live Agent row to convert".to_owned(),
            ));
        }
    }
    sqlx::query(
        "INSERT INTO inventory_migration (singleton, protocol_major, preserved_revision, previous_sha256, fingerprint_sha256, converted_at) VALUES (1, 2, ?, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET protocol_major = 2, preserved_revision = excluded.preserved_revision, previous_sha256 = excluded.previous_sha256, fingerprint_sha256 = excluded.fingerprint_sha256, converted_at = excluded.converted_at",
    )
    .bind(conversion.preserved_revision)
    .bind(conversion.previous_sha256.as_deref())
    .bind(conversion.fingerprint_sha256.as_deref())
    .bind(converted_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    connection.close().await?;
    Ok(())
}

async fn convert_agent_store(
    path: &Path,
    closing_report_id: &str,
    conversion: &BaselineConversion,
    converted_at: &str,
) -> Result<(), MigrationError> {
    let mut connection = open_read_write(path).await?;
    sqlx::query(AGENT_MIGRATION_DDL)
        .execute(&mut connection)
        .await?;
    let mut tx = connection.begin().await?;
    let existing: Option<(i64, String)> =
        sqlx::query_as("SELECT revision, sha256 FROM inventory_declaration WHERE singleton = 1")
            .fetch_optional(&mut *tx)
            .await?;
    if conversion.uninitialized {
        if existing.is_some() {
            return Err(MigrationError::Invalid(
                "the Agent holds an accepted declaration but the Server baseline is uninitialized; the two halves disagree".to_owned(),
            ));
        }
    } else {
        if let Some((revision, _)) = &existing
            && *revision > conversion.preserved_revision
        {
            return Err(MigrationError::Invalid(
                "the Agent Inventory Declaration Record is newer than the verified accepted revision".to_owned(),
            ));
        }
        // Preserve the record provenance when it already exists: only the
        // algorithm-dependent fingerprint changes. A missing record is created
        // from the verified Closing, which is the report that made this
        // declaration effective.
        sqlx::query(
            "INSERT INTO inventory_declaration (singleton, revision, sha256, report_id, adopted_at) VALUES (1, ?, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET sha256 = excluded.sha256",
        )
        .bind(conversion.preserved_revision)
        .bind(conversion.fingerprint_sha256.as_deref())
        .bind(closing_report_id)
        .bind(converted_at)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO inventory_migration (singleton, protocol_major, preserved_revision, previous_sha256, fingerprint_sha256, converted_at) VALUES (1, 2, ?, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET protocol_major = 2, preserved_revision = excluded.preserved_revision, previous_sha256 = excluded.previous_sha256, fingerprint_sha256 = excluded.fingerprint_sha256, converted_at = excluded.converted_at",
    )
    .bind(conversion.preserved_revision)
    .bind(conversion.previous_sha256.as_deref())
    .bind(conversion.fingerprint_sha256.as_deref())
    .bind(converted_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    connection.close().await?;
    Ok(())
}

async fn verify_converted_server(
    directory: &Path,
    checkpoint_manifest: &CoordinationCheckpointManifest,
    conversion: &BaselineConversion,
) -> Result<(), MigrationError> {
    let db_path = directory.join("server.db");
    let mut connection = open_read_only(&db_path).await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await?;
    if integrity != "ok" {
        connection.close().await?;
        return Err(MigrationError::Invalid(format!(
            "the converted Server database failed its integrity check: {integrity}"
        )));
    }
    let row: Option<ConvertedAgentRow> = sqlx::query_as(
        "SELECT last_inventory_revision, inventory_sha256, inventory_protocol_major, active_boot_id, active_boot_status, close_report_id, last_report_sequence FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
    )
    .bind(&checkpoint_manifest.agent.agent_id)
    .fetch_optional(&mut connection)
    .await?;
    let row = row.ok_or_else(|| {
        MigrationError::Invalid("the converted Server database has no live Agent row".to_owned())
    })?;
    if row.last_inventory_revision != conversion.preserved_revision
        || row.inventory_sha256 != conversion.fingerprint_sha256
        || row.inventory_protocol_major != conversion.expected_protocol_major()
        || row.active_boot_id.as_deref() != Some(checkpoint_manifest.agent.closed_boot_id.as_str())
        || row.active_boot_status != "closed"
        || row.close_report_id.as_deref()
            != Some(checkpoint_manifest.agent.closing_report_id.as_str())
        || row.last_report_sequence != Some(checkpoint_manifest.agent.closing_report_sequence)
    {
        connection.close().await?;
        return Err(MigrationError::Invalid(
            "the converted Server baseline, Boot linkage or Closing identity changed".to_owned(),
        ));
    }
    let marker: Option<MarkerRow> = sqlx::query_as(
        "SELECT protocol_major, preserved_revision, previous_sha256, fingerprint_sha256 FROM inventory_migration WHERE singleton = 1",
    )
    .fetch_optional(&mut connection)
    .await?;
    let Some(marker) = marker else {
        connection.close().await?;
        return Err(MigrationError::Invalid(
            "the converted Server database has no conversion marker".to_owned(),
        ));
    };
    if marker.protocol_major != 2
        || marker.preserved_revision != conversion.preserved_revision
        || marker.previous_sha256 != conversion.previous_sha256
        || marker.fingerprint_sha256 != conversion.fingerprint_sha256
    {
        connection.close().await?;
        return Err(MigrationError::Invalid(
            "the converted Server conversion marker does not match the derivation".to_owned(),
        ));
    }
    // The original Closing Receipt and its Report identity survive verbatim.
    let receipt: Option<ClosingReceiptRow> = sqlx::query_as(
        "SELECT disposition, report_body_sha256, receipt_body FROM agent_report_receipts WHERE report_id = ?",
    )
    .bind(&checkpoint_manifest.agent.closing_report_id)
    .fetch_optional(&mut connection)
    .await?;
    let deleted: Vec<String> =
        sqlx::query_scalar("SELECT node_id FROM deleted_nodes WHERE agent_id = ? ORDER BY node_id")
            .bind(&checkpoint_manifest.agent.agent_id)
            .fetch_all(&mut connection)
            .await?;
    let boot: Option<String> = sqlx::query_scalar(
        "SELECT status FROM agent_boots WHERE agent_id = ? AND agent_epoch = ? AND boot_id = ?",
    )
    .bind(&checkpoint_manifest.agent.agent_id)
    .bind(checkpoint_manifest.agent.agent_epoch)
    .bind(&checkpoint_manifest.agent.closed_boot_id)
    .fetch_optional(&mut connection)
    .await?;
    connection.close().await?;
    let Some(receipt) = receipt else {
        return Err(MigrationError::Invalid(
            "the converted Server database lost the original Closing Receipt".to_owned(),
        ));
    };
    if receipt.disposition != checkpoint_manifest.agent.closing_receipt_disposition
        || receipt.report_body_sha256 != checkpoint_manifest.agent.closing_report_body_sha256
        || Some(hex_sha256(&receipt.receipt_body))
            != checkpoint_manifest.server.closing_receipt_body_sha256
    {
        return Err(MigrationError::Invalid(
            "the conversion rewrote the original Closing Receipt".to_owned(),
        ));
    }
    if deleted != checkpoint_manifest.server.deleted_node_ids {
        return Err(MigrationError::Invalid(
            "the conversion changed the preserved deletion identities".to_owned(),
        ));
    }
    if boot.as_deref() != Some("closed") {
        return Err(MigrationError::Invalid(
            "the converted Server database lost the closed old Boot".to_owned(),
        ));
    }
    Ok(())
}

async fn verify_converted_agent(
    directory: &Path,
    checkpoint_manifest: &CoordinationCheckpointManifest,
    conversion: &BaselineConversion,
) -> Result<(), MigrationError> {
    let path = directory.join("agent-store.sqlite");
    let mut connection = open_read_only(&path).await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await?;
    if integrity != "ok" {
        connection.close().await?;
        return Err(MigrationError::Invalid(format!(
            "the converted Agent Store failed its integrity check: {integrity}"
        )));
    }
    let state: Option<ConvertedAgentStateRow> = sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id FROM agent_state WHERE singleton = 1",
    )
    .fetch_optional(&mut connection)
    .await?;
    let Some(state) = state else {
        connection.close().await?;
        return Err(MigrationError::Invalid(
            "the converted Agent Store has no enrollment state".to_owned(),
        ));
    };
    if state.agent_id.as_deref() != Some(checkpoint_manifest.agent.agent_id.as_str())
        || state.agent_epoch != checkpoint_manifest.agent.agent_epoch
        || state.boot_id.as_deref() != Some(checkpoint_manifest.agent.next_boot_id.as_str())
        || state.boot_state != "drained_pending"
        || state.previous_boot_id.as_deref()
            != Some(checkpoint_manifest.agent.closed_boot_id.as_str())
        || state.pending_transition.as_deref() != Some("drained_previous")
        || state.pending_previous_boot_id.as_deref()
            != Some(checkpoint_manifest.agent.closed_boot_id.as_str())
        || state.close_report_id.as_deref()
            != Some(checkpoint_manifest.agent.closing_report_id.as_str())
    {
        connection.close().await?;
        return Err(MigrationError::Invalid(
            "the converted Agent Store lost the pending DrainedPrevious Boot linkage".to_owned(),
        ));
    }
    let marker: Option<MarkerRow> = sqlx::query_as(
        "SELECT protocol_major, preserved_revision, previous_sha256, fingerprint_sha256 FROM inventory_migration WHERE singleton = 1",
    )
    .fetch_optional(&mut connection)
    .await?;
    let Some(marker) = marker else {
        connection.close().await?;
        return Err(MigrationError::Invalid(
            "the converted Agent Store has no conversion marker".to_owned(),
        ));
    };
    if marker.protocol_major != 2
        || marker.preserved_revision != conversion.preserved_revision
        || marker.previous_sha256 != conversion.previous_sha256
        || marker.fingerprint_sha256 != conversion.fingerprint_sha256
    {
        connection.close().await?;
        return Err(MigrationError::Invalid(
            "the converted Agent conversion marker does not match the derivation".to_owned(),
        ));
    }
    let record: Option<DeclarationRecordRow> =
        sqlx::query_as("SELECT revision, sha256 FROM inventory_declaration WHERE singleton = 1")
            .fetch_optional(&mut connection)
            .await?;
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports")
        .fetch_one(&mut connection)
        .await?;
    connection.close().await?;
    if queued != 0 {
        return Err(MigrationError::Invalid(format!(
            "the converted Agent Store still holds {queued} undelivered report(s)"
        )));
    }
    match (conversion.uninitialized, record) {
        (true, Some(_)) => Err(MigrationError::Invalid(
            "the converted Agent Store invented an accepted declaration for an uninitialized baseline".to_owned(),
        )),
        (true, None) => Ok(()),
        (false, None) => Err(MigrationError::Invalid(
            "the converted Agent Store lost its Inventory Declaration Record".to_owned(),
        )),
        (false, Some(record)) => {
            if record.revision != conversion.preserved_revision
                || Some(&record.sha256) != conversion.fingerprint_sha256.as_ref()
            {
                return Err(MigrationError::Invalid(
                    "the converted Agent Inventory Declaration Record does not bind the v2 fingerprint".to_owned(),
                ));
            }
            Ok(())
        }
    }
}

fn verify_converted_configs(
    server_directory: &Path,
    agent_directory: &Path,
) -> Result<(), MigrationError> {
    let agent_config = std::fs::read_to_string(agent_directory.join("agent.toml"))?;
    let agent_value: toml::Value = toml::from_str(&agent_config).map_err(|error| {
        MigrationError::Invalid(format!("the converted agent.toml is invalid: {error}"))
    })?;
    let agent_table = agent_value.as_table().ok_or_else(|| {
        MigrationError::Invalid("the converted agent.toml is not a table".to_owned())
    })?;
    if agent_table.contains_key("inventory_revision") {
        return Err(MigrationError::Invalid(
            "the converted agent.toml still carries inventory_revision; v2 assigns it on the Server".to_owned(),
        ));
    }
    let expected_store = agent_directory.join("agent-store.sqlite");
    if agent_table.get("state_db").and_then(|value| value.as_str()) != expected_store.to_str() {
        return Err(MigrationError::Invalid(
            "the converted agent.toml does not point at the converted Agent Store".to_owned(),
        ));
    }
    let expected_credential = agent_directory.join("credential");
    if agent_table
        .get("credential_file")
        .and_then(|value| value.as_str())
        != expected_credential.to_str()
    {
        return Err(MigrationError::Invalid(
            "the converted agent.toml does not point at the converted credential".to_owned(),
        ));
    }
    let server_config = std::fs::read_to_string(server_directory.join("server.toml"))?;
    let server_value: toml::Value = toml::from_str(&server_config).map_err(|error| {
        MigrationError::Invalid(format!("the converted server.toml is invalid: {error}"))
    })?;
    let server_table = server_value.as_table().ok_or_else(|| {
        MigrationError::Invalid("the converted server.toml is not a table".to_owned())
    })?;
    let expected_db = server_directory.join("server.db");
    if server_table.get("db_path").and_then(|value| value.as_str()) != expected_db.to_str() {
        return Err(MigrationError::Invalid(
            "the converted server.toml does not point at the converted database".to_owned(),
        ));
    }
    let expected_pepper = server_directory.join("server-pepper");
    if server_table
        .get("pepper_file")
        .and_then(|value| value.as_str())
        != expected_pepper.to_str()
    {
        return Err(MigrationError::Invalid(
            "the converted server.toml does not point at the converted pepper".to_owned(),
        ));
    }
    Ok(())
}

fn write_converted_agent_config(source: &Path, target: &Path) -> Result<(), MigrationError> {
    let original = std::fs::read_to_string(source.join("agent.toml"))?;
    let mut value: toml::Value = toml::from_str(&original).map_err(|error| {
        MigrationError::Invalid(format!("the preserved agent.toml is invalid: {error}"))
    })?;
    let table = value.as_table_mut().ok_or_else(|| {
        MigrationError::Invalid("the preserved agent.toml is not a table".to_owned())
    })?;
    if table.remove("inventory_revision").is_none() {
        return Err(MigrationError::Invalid(
            "the preserved agent.toml has no inventory_revision; it is not a frozen v1 configuration".to_owned(),
        ));
    }
    table.insert(
        "state_db".to_owned(),
        toml::Value::String(target.join("agent-store.sqlite").display().to_string()),
    );
    table.insert(
        "credential_file".to_owned(),
        toml::Value::String(target.join("credential").display().to_string()),
    );
    let converted = toml::to_string(&value).map_err(|error| {
        MigrationError::Invalid(format!(
            "cannot serialize the converted agent.toml: {error}"
        ))
    })?;
    write_private(&target.join("agent.toml"), converted.as_bytes())?;
    Ok(())
}

fn write_converted_server_config(
    source: &Path,
    target: &Path,
    tls_key_copied: bool,
) -> Result<(), MigrationError> {
    let original = std::fs::read_to_string(source.join("server.toml"))?;
    let mut value: toml::Value = toml::from_str(&original).map_err(|error| {
        MigrationError::Invalid(format!("the preserved server.toml is invalid: {error}"))
    })?;
    let table = value.as_table_mut().ok_or_else(|| {
        MigrationError::Invalid("the preserved server.toml is not a table".to_owned())
    })?;
    table.insert(
        "state_dir".to_owned(),
        toml::Value::String(target.display().to_string()),
    );
    table.insert(
        "db_path".to_owned(),
        toml::Value::String(target.join("server.db").display().to_string()),
    );
    table.insert(
        "pepper_file".to_owned(),
        toml::Value::String(target.join("server-pepper").display().to_string()),
    );
    if tls_key_copied && let Some(tls) = table.get_mut("tls").and_then(|value| value.as_table_mut())
    {
        tls.insert(
            "private_key_file".to_owned(),
            toml::Value::String(target.join("tls-private-key.pem").display().to_string()),
        );
    }
    let converted = toml::to_string(&value).map_err(|error| {
        MigrationError::Invalid(format!(
            "cannot serialize the converted server.toml: {error}"
        ))
    })?;
    write_private(&target.join("server.toml"), converted.as_bytes())?;
    Ok(())
}

fn copy_artifact(
    source_directory: &Path,
    artifacts: &[crate::checkpoint::CheckpointArtifact],
    role: &str,
    destination: &Path,
) -> Result<(), MigrationError> {
    let artifact = artifacts
        .iter()
        .find(|artifact| artifact.role == role)
        .ok_or_else(|| {
            MigrationError::Invalid(format!("the checkpoint is missing its {role} artifact"))
        })?;
    if !crate::file_security::is_safe_basename(&artifact.file) {
        return Err(MigrationError::Invalid(format!(
            "unsafe checkpoint entry name {}",
            artifact.file
        )));
    }
    copy_file_private(&source_directory.join(&artifact.file), destination)
}

fn summary_of(
    manifest: &ConversionManifest,
    directory: &Path,
    deleted_nodes: usize,
) -> ConversionSummary {
    ConversionSummary {
        directory: directory.to_path_buf(),
        agent_id: manifest.agent_id.clone(),
        closed_boot_id: manifest.closed_boot_id.clone(),
        next_boot_id: manifest.next_boot_id.clone(),
        inventory_revision: manifest.inventory_revision,
        previous_sha256: manifest.previous_sha256.clone(),
        fingerprint_sha256: manifest.fingerprint_sha256.clone(),
        uninitialized: manifest.uninitialized,
        deleted_nodes,
    }
}

async fn open_read_write(path: &Path) -> Result<SqliteConnection, MigrationError> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false);
    SqliteConnection::connect_with(&options)
        .await
        .map_err(MigrationError::from)
}

async fn open_read_only(path: &Path) -> Result<SqliteConnection, MigrationError> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    SqliteConnection::connect_with(&options)
        .await
        .map_err(MigrationError::from)
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn prepare_directory(path: &Path) -> Result<bool, MigrationError> {
    if path.as_os_str().is_empty() {
        return Err(MigrationError::UnsafeDirectory(
            "the directory must not be empty".to_owned(),
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(MigrationError::UnsafeDirectory(format!(
                    "{} is not a directory",
                    path.display()
                )));
            }
            if std::fs::read_dir(path)?.next().is_some() {
                return Err(MigrationError::UnsafeDirectory(format!(
                    "{} already exists and is not empty",
                    path.display()
                )));
            }
            secure_directory(path)?;
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)?;
            secure_directory(path)?;
            Ok(true)
        }
        Err(error) => Err(MigrationError::Io(error)),
    }
}

fn cleanup_directory(path: &Path, created: bool) {
    if created {
        let _ = std::fs::remove_dir_all(path);
    } else {
        for entry in [MIGRATION_MANIFEST, AGENT_DIRECTORY, SERVER_DIRECTORY] {
            let target = path.join(entry);
            if target.is_dir() {
                let _ = std::fs::remove_dir_all(&target);
            } else {
                let _ = std::fs::remove_file(&target);
            }
        }
    }
}

fn create_private_directory(path: &Path) -> Result<(), MigrationError> {
    std::fs::create_dir_all(path)?;
    secure_directory(path)
}

fn copy_file_private(source: &Path, destination: &Path) -> Result<(), MigrationError> {
    let bytes = std::fs::read(source)?;
    write_private(destination, &bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), MigrationError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> Result<(), MigrationError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> Result<(), MigrationError> {
    Ok(())
}
