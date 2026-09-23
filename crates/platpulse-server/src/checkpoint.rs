//! Coordinated upgrade checkpoint (issue #190).
//!
//! [ADR 0007](../../docs/adr/0007-server-managed-inventory-revision.md) and
//! [main design 15.10.4](../../docs/design/platpulse.md#server-managed-inventory-revision-target)
//! require the v1 -> v2 cutover to pass through a coordinated checkpoint: with
//! every writer paused, preserve an exact logical copy of the Server database, the Agent Store,
//! the old configuration, identity/credential material and the verified
//! frozen-v1 declaration evidence, then prove the whole set can be restored
//! into an isolated environment at the same closed old Boot awaiting its new
//! Boot linkage.
//!
//! This module is the Server half. It consumes the Agent checkpoint produced by
//! platpulse-agent checkpoint create, re-verifies it against the Server's own
//! last accepted baseline and Closing Report Receipt, snapshots the Server
//! database with VACUUM INTO (an exact logical copy, including valid
//! uncheckpointed WAL content), and writes one coordination manifest that binds
//! both halves. It deliberately does not use the sanitized Admin Backup: that
//! path redacts and rewrites stored content and Receipts, so it cannot serve as
//! this contract's exact checkpoint.
//!
//! Every Server command takes the exclusive database ownership guard before
//! SQLite opens, exactly like the other offline commands, so a running Server
//! (collection, ingestion, Admin writes and notification workers) is refused
//! rather than silently captured mid-write. The Agent half refuses while the
//! Agent runtime owns its Store. The two halves are bound by identity, Epoch,
//! Boot linkage, Inventory revision/hash and the exact Closing report hash.
//!
//! create/verify/restore never start a collector, an ingestion path, an Admin
//! mutation or an external-effect worker, and never print credential contents.

use std::path::{Path, PathBuf};

use platpulse_core::inventory::NodeInventory;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sqlx::Connection;
use sqlx::SqliteConnection;
use thiserror::Error;

use crate::config::ServerConfig;

/// Format version of the coordinated checkpoint manifest.
pub const CHECKPOINT_FORMAT_VERSION: u32 = 1;

/// Manifest kind discriminator.
pub const CHECKPOINT_KIND: &str = "platpulse.upgrade-checkpoint";

/// Coordination manifest file name.
pub const CHECKPOINT_MANIFEST: &str = "checkpoint.json";

/// Directory holding the Agent half.
pub const AGENT_DIRECTORY: &str = "agent";
/// Directory holding the Server half.
pub const SERVER_DIRECTORY: &str = "server";
/// Agent manifest file name (must match the Agent crate).
pub const AGENT_MANIFEST: &str = "agent-checkpoint.json";
/// Agent manifest kind (must match the Agent crate).
pub const AGENT_MANIFEST_KIND: &str = "platpulse.upgrade-checkpoint.agent";

pub const ROLE_SERVER_DB: &str = "server-db";
pub const ROLE_SERVER_CONFIG: &str = "server-config";
pub const ROLE_SERVER_PEPPER: &str = "server-pepper";
pub const ROLE_SERVER_TLS_KEY: &str = "tls-private-key";

const AGENT_ROLE_STORE: &str = "agent-store";
const AGENT_ROLE_CONFIG: &str = "agent-config";
const AGENT_ROLE_CREDENTIAL: &str = "agent-credential";

/// One preserved file, shared by both halves of the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointArtifact {
    pub role: String,
    pub original_path: String,
    pub file: String,
    pub bytes: i64,
    pub sha256: String,
}

/// The Agent manifest as the Server reads it back for verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCheckpointManifest {
    pub kind: String,
    pub format_version: u32,
    pub agent_version: String,
    pub created_at: String,
    pub agent_id: String,
    pub agent_epoch: i64,
    pub inventory_protocol_major: i64,
    pub closed_boot_id: String,
    pub next_boot_id: String,
    pub pending_transition: String,
    pub closing_report_id: String,
    pub closing_report_sequence: i64,
    pub closing_receipt_disposition: String,
    pub inventory_revision: i64,
    pub inventory_sha256: String,
    pub accepted_inventory_revision: i64,
    pub accepted_inventory_sha256: String,
    pub accepted_inventory_protocol_major: i64,
    pub declaration_sha256: String,
    pub declaration_json: String,
    pub closing_report_body_sha256: String,
    pub artifacts: Vec<CheckpointArtifact>,
}

/// Identity/evidence facts copied out of the Agent manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBinding {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub inventory_protocol_major: i64,
    pub closed_boot_id: String,
    pub next_boot_id: String,
    pub pending_transition: String,
    pub closing_report_id: String,
    pub closing_report_sequence: i64,
    pub closing_receipt_disposition: String,
    pub inventory_revision: i64,
    pub inventory_sha256: String,
    pub accepted_inventory_revision: i64,
    pub accepted_inventory_sha256: String,
    pub accepted_inventory_protocol_major: i64,
    pub declaration_sha256: String,
    pub closing_report_body_sha256: String,
}

impl AgentBinding {
    fn of(manifest: &AgentCheckpointManifest) -> Self {
        Self {
            agent_id: manifest.agent_id.clone(),
            agent_epoch: manifest.agent_epoch,
            inventory_protocol_major: manifest.inventory_protocol_major,
            closed_boot_id: manifest.closed_boot_id.clone(),
            next_boot_id: manifest.next_boot_id.clone(),
            pending_transition: manifest.pending_transition.clone(),
            closing_report_id: manifest.closing_report_id.clone(),
            closing_report_sequence: manifest.closing_report_sequence,
            closing_receipt_disposition: manifest.closing_receipt_disposition.clone(),
            inventory_revision: manifest.inventory_revision,
            inventory_sha256: manifest.inventory_sha256.clone(),
            accepted_inventory_revision: manifest.accepted_inventory_revision,
            accepted_inventory_sha256: manifest.accepted_inventory_sha256.clone(),
            accepted_inventory_protocol_major: manifest.accepted_inventory_protocol_major,
            declaration_sha256: manifest.declaration_sha256.clone(),
            closing_report_body_sha256: manifest.closing_report_body_sha256.clone(),
        }
    }
}

/// The Server half's preserved facts and artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerBinding {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub accepted_inventory_revision: i64,
    pub accepted_inventory_sha256: Option<String>,
    pub inventory_protocol_major: Option<i64>,
    pub active_boot_id: Option<String>,
    pub active_boot_status: String,
    pub previous_boot_id: Option<String>,
    pub close_report_id: Option<String>,
    pub close_report_disposition: Option<String>,
    pub last_report_sequence: Option<i64>,
    pub closing_receipt_report_body_sha256: Option<String>,
    pub closing_receipt_body_sha256: Option<String>,
    pub closed_boot_row_status: Option<String>,
    pub deleted_node_ids: Vec<String>,
    pub artifacts: Vec<CheckpointArtifact>,
}

/// The single coordination manifest binding both halves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinationCheckpointManifest {
    pub kind: String,
    pub format_version: u32,
    pub created_at: String,
    pub server_version: String,
    pub agent_version: String,
    pub agent_directory: String,
    pub server_directory: String,
    pub agent_manifest: String,
    pub agent_manifest_sha256: String,
    pub agent: AgentBinding,
    pub server: ServerBinding,
}

/// Sanitized result of verifying one coordinated checkpoint.
#[derive(Debug, Clone)]
pub struct CheckpointSummary {
    pub agent_id: String,
    pub agent_epoch: i64,
    pub closed_boot_id: String,
    pub next_boot_id: String,
    pub inventory_revision: i64,
    pub inventory_sha256: String,
    pub closing_report_id: String,
    pub deleted_nodes: usize,
    pub agent_artifacts: usize,
    pub server_artifacts: usize,
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error(transparent)]
    Ownership(#[from] crate::ownership::OwnershipError),
    #[error("Server database error: {0}")]
    ServerDatabase(#[from] crate::database::ServerDatabaseError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("checkpoint IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checkpoint JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the checkpoint is invalid: {0}")]
    Invalid(String),
    #[error("the checkpoint directory is unsafe or already exists: {0}")]
    UnsafeDirectory(String),
    #[error("Server state changed during the checkpoint: {0}")]
    StateChanged(String),
}

/// Facts read from one Server database, whether live or a checkpoint snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerFacts {
    agent_id: String,
    agent_epoch: i64,
    accepted_inventory_revision: i64,
    accepted_inventory_sha256: Option<String>,
    inventory_protocol_major: Option<i64>,
    active_boot_id: Option<String>,
    active_boot_status: String,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    close_report_disposition: Option<String>,
    last_report_sequence: Option<i64>,
    closing_receipt_report_body_sha256: Option<String>,
    closing_receipt_body_sha256: Option<String>,
    closed_boot_row_status: Option<String>,
    deleted_node_ids: Vec<String>,
}

impl ServerFacts {
    fn into_binding(self, artifacts: Vec<CheckpointArtifact>) -> ServerBinding {
        ServerBinding {
            agent_id: self.agent_id,
            agent_epoch: self.agent_epoch,
            accepted_inventory_revision: self.accepted_inventory_revision,
            accepted_inventory_sha256: self.accepted_inventory_sha256,
            inventory_protocol_major: self.inventory_protocol_major,
            active_boot_id: self.active_boot_id,
            active_boot_status: self.active_boot_status,
            previous_boot_id: self.previous_boot_id,
            close_report_id: self.close_report_id,
            close_report_disposition: self.close_report_disposition,
            last_report_sequence: self.last_report_sequence,
            closing_receipt_report_body_sha256: self.closing_receipt_report_body_sha256,
            closing_receipt_body_sha256: self.closing_receipt_body_sha256,
            closed_boot_row_status: self.closed_boot_row_status,
            deleted_node_ids: self.deleted_node_ids,
            artifacts,
        }
    }

    fn from_binding(binding: &ServerBinding) -> Self {
        Self {
            agent_id: binding.agent_id.clone(),
            agent_epoch: binding.agent_epoch,
            accepted_inventory_revision: binding.accepted_inventory_revision,
            accepted_inventory_sha256: binding.accepted_inventory_sha256.clone(),
            inventory_protocol_major: binding.inventory_protocol_major,
            active_boot_id: binding.active_boot_id.clone(),
            active_boot_status: binding.active_boot_status.clone(),
            previous_boot_id: binding.previous_boot_id.clone(),
            close_report_id: binding.close_report_id.clone(),
            close_report_disposition: binding.close_report_disposition.clone(),
            last_report_sequence: binding.last_report_sequence,
            closing_receipt_report_body_sha256: binding.closing_receipt_report_body_sha256.clone(),
            closing_receipt_body_sha256: binding.closing_receipt_body_sha256.clone(),
            closed_boot_row_status: binding.closed_boot_row_status.clone(),
            deleted_node_ids: binding.deleted_node_ids.clone(),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct AgentRow {
    agent_epoch: i64,
    active_boot_id: Option<String>,
    active_boot_status: String,
    previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    last_report_sequence: Option<i64>,
    last_inventory_revision: i64,
    inventory_sha256: Option<String>,
    inventory_protocol_major: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct ReceiptRow {
    disposition: String,
    report_body_sha256: String,
    receipt_body: Vec<u8>,
}

#[derive(Debug, sqlx::FromRow)]
struct AgentStateRow {
    agent_id: Option<String>,
    agent_epoch: i64,
    boot_id: Option<String>,
    boot_state: String,
    previous_boot_id: Option<String>,
    pending_transition: Option<String>,
    pending_previous_boot_id: Option<String>,
    close_report_id: Option<String>,
    last_report_body: Option<Vec<u8>>,
}

#[derive(Debug, sqlx::FromRow)]
struct PreparationRow {
    agent_id: String,
    agent_epoch: i64,
    closing_report_id: String,
    closing_report_sequence: i64,
    closing_receipt_disposition: String,
    inventory_revision: i64,
    inventory_sha256: String,
    declaration_json: String,
    closed_boot_id: String,
    next_boot_id: String,
    pending_transition: String,
    accepted_inventory_revision: i64,
    accepted_inventory_sha256: String,
    accepted_inventory_protocol_major: i64,
}

const AGENT_STATE_SELECT: &str = "SELECT agent_id, agent_epoch, boot_id, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id, last_report_body FROM agent_state WHERE singleton=1";
const PREPARATION_SELECT: &str = "SELECT agent_id, agent_epoch, closing_report_id, closing_report_sequence, closing_receipt_disposition, inventory_revision, inventory_sha256, declaration_json, closed_boot_id, next_boot_id, pending_transition, accepted_inventory_revision, accepted_inventory_sha256, accepted_inventory_protocol_major FROM upgrade_preparation WHERE singleton=1";

/// Create the coordinated checkpoint: copy and verify the Agent half, snapshot
/// the Server database exactly, and bind both into one coordination manifest.
pub async fn create_checkpoint(
    config: &ServerConfig,
    agent_checkpoint: &Path,
    output: &Path,
) -> Result<CheckpointSummary, CheckpointError> {
    crate::init::restrict_umask();
    // Every checkpoint requires an exclusive stopped Server, exactly like
    // Restore: the ownership guard is taken before SQLite opens, and the
    // development-mode exception (which lets local tooling attach to a running
    // dev Server) is deliberately not applied to an exact snapshot.
    let _ownership = crate::ownership::acquire(&config.db_path)?;

    if !agent_checkpoint.is_dir() {
        return Err(CheckpointError::Invalid(format!(
            "the Agent checkpoint directory {} does not exist",
            agent_checkpoint.display()
        )));
    }
    let absolute_output = std::path::absolute(output)?;
    let absolute_agent = std::path::absolute(agent_checkpoint)?;
    if absolute_output.starts_with(&absolute_agent) || absolute_agent.starts_with(&absolute_output)
    {
        return Err(CheckpointError::UnsafeDirectory(
            "the coordinated output and the Agent checkpoint must be separate directories"
                .to_owned(),
        ));
    }

    let created_directory = prepare_directory(output)?;
    let result = create_inner(config, agent_checkpoint, output).await;
    if result.is_err() {
        cleanup_directory(output, created_directory);
    }
    result
}

async fn create_inner(
    config: &ServerConfig,
    agent_checkpoint: &Path,
    output: &Path,
) -> Result<CheckpointSummary, CheckpointError> {
    let config_source = config.config_path.clone().ok_or_else(|| {
        CheckpointError::Invalid(
            "the Server configuration file path is unknown; pass --config <server.toml>".to_owned(),
        )
    })?;

    let target_agent = output.join(AGENT_DIRECTORY);
    copy_tree(agent_checkpoint, &target_agent)?;

    // Verify the copied Agent half before trusting any fact from it.
    let agent_manifest_path = target_agent.join(AGENT_MANIFEST);
    let agent_manifest_sha256 = sha256_file(&agent_manifest_path)?;
    let (agent_manifest, agent_binding) = verify_agent_half(&target_agent).await?;

    // Open the live database directly, without running the Server migrator: a
    // coordinated checkpoint must not mutate its source. The exclusive
    // ownership guard already proves that no Server owns this database.
    let mut connection = open_live_database(&config.db_path).await?;
    let live_facts = collect_server_facts(&mut connection, &agent_manifest.agent_id).await?;
    cross_check(&agent_binding, &live_facts)?;

    let target_server = output.join(SERVER_DIRECTORY);
    create_private_directory(&target_server)?;
    let server_db_path = target_server.join("server.db");
    let server_db_text = server_db_path
        .to_str()
        .ok_or_else(|| CheckpointError::UnsafeDirectory("path is not UTF-8".to_owned()))?;
    if server_db_text.contains('\'') {
        return Err(CheckpointError::UnsafeDirectory(
            "the checkpoint path must not contain single quotes".to_owned(),
        ));
    }
    sqlx::query(&format!("VACUUM INTO '{server_db_text}'"))
        .execute(&mut connection)
        .await?;
    secure_file(&server_db_path)?;
    connection.close().await?;

    let server_config_path = target_server.join("server.toml");
    copy_file_private(&config_source, &server_config_path)?;
    let pepper_path = target_server.join("server-pepper");
    copy_file_private(&config.pepper_file, &pepper_path)?;
    let mut artifacts = vec![
        artifact(
            ROLE_SERVER_DB,
            &config.db_path,
            "server.db",
            &server_db_path,
        )?,
        artifact(
            ROLE_SERVER_CONFIG,
            &config_source,
            "server.toml",
            &server_config_path,
        )?,
        artifact(
            ROLE_SERVER_PEPPER,
            &config.pepper_file,
            "server-pepper",
            &pepper_path,
        )?,
    ];
    if let Some(tls) = &config.tls {
        let tls_key_path = target_server.join("tls-private-key.pem");
        copy_file_private(&tls.private_key_file, &tls_key_path)?;
        artifacts.push(artifact(
            ROLE_SERVER_TLS_KEY,
            &tls.private_key_file,
            "tls-private-key.pem",
            &tls_key_path,
        )?);
    }
    artifacts.sort_by(|left, right| left.role.cmp(&right.role));

    // Re-read the snapshot and require it to be exactly the verified state, and
    // confirm the Agent half is byte-identical after the copy.
    let snapshot_facts =
        collect_server_facts_in_file(&server_db_path, &agent_manifest.agent_id).await?;
    if snapshot_facts != live_facts {
        return Err(CheckpointError::StateChanged(
            "the Server database changed between verification and snapshot".to_owned(),
        ));
    }
    if sha256_file(&agent_manifest_path)? != agent_manifest_sha256 {
        return Err(CheckpointError::StateChanged(
            "the Agent checkpoint changed while the coordinated checkpoint was created".to_owned(),
        ));
    }

    let manifest = CoordinationCheckpointManifest {
        kind: CHECKPOINT_KIND.to_owned(),
        format_version: CHECKPOINT_FORMAT_VERSION,
        created_at: crate::auth::format_rfc3339(crate::auth::now_utc()),
        server_version: crate::VERSION.to_owned(),
        agent_version: agent_manifest.agent_version.clone(),
        agent_directory: AGENT_DIRECTORY.to_owned(),
        server_directory: SERVER_DIRECTORY.to_owned(),
        agent_manifest: AGENT_MANIFEST.to_owned(),
        agent_manifest_sha256,
        agent: agent_binding,
        server: snapshot_facts.into_binding(artifacts),
    };
    write_private(
        &output.join(CHECKPOINT_MANIFEST),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;

    Ok(summary_of(&manifest, agent_manifest.artifacts.len()))
}

/// Verify a coordinated checkpoint without touching any live database.
pub async fn verify_checkpoint(directory: &Path) -> Result<CheckpointSummary, CheckpointError> {
    let manifest_path = directory.join(CHECKPOINT_MANIFEST);
    let bytes = std::fs::read(&manifest_path).map_err(|error| {
        CheckpointError::Invalid(format!("cannot read {}: {error}", manifest_path.display()))
    })?;
    let manifest: CoordinationCheckpointManifest = serde_json::from_slice(&bytes)?;
    if manifest.kind != CHECKPOINT_KIND || manifest.format_version != CHECKPOINT_FORMAT_VERSION {
        return Err(CheckpointError::Invalid(format!(
            "unexpected checkpoint kind {} format {}",
            manifest.kind, manifest.format_version
        )));
    }
    let agent_directory = safe_child(directory, &manifest.agent_directory)?;
    let server_directory = safe_child(directory, &manifest.server_directory)?;

    let agent_manifest_path = safe_child(&agent_directory, &manifest.agent_manifest)?;
    let agent_manifest_bytes = std::fs::read(&agent_manifest_path)?;
    let observed_manifest_sha256 = sha256_bytes(&agent_manifest_bytes);
    if observed_manifest_sha256 != manifest.agent_manifest_sha256 {
        return Err(CheckpointError::Invalid(format!(
            "the Agent manifest does not match its recorded hash ({} vs {})",
            observed_manifest_sha256, manifest.agent_manifest_sha256
        )));
    }
    let (agent_manifest, agent_binding) = verify_agent_half(&agent_directory).await?;
    if agent_binding != manifest.agent {
        return Err(CheckpointError::Invalid(
            "the coordination manifest does not match the Agent manifest".to_owned(),
        ));
    }

    let server_db_path = artifact_path(
        &server_directory,
        &manifest.server.artifacts,
        ROLE_SERVER_DB,
    )?;
    let server_facts =
        collect_server_facts_in_file(&server_db_path, &manifest.agent.agent_id).await?;
    if server_facts != ServerFacts::from_binding(&manifest.server) {
        return Err(CheckpointError::Invalid(
            "the Server database snapshot does not match the coordination manifest".to_owned(),
        ));
    }
    verify_artifacts(&server_directory, &manifest.server.artifacts)?;
    cross_check(&agent_binding, &server_facts)?;
    if manifest.server.active_boot_status != "closed"
        || agent_manifest.pending_transition != "drained_previous"
    {
        return Err(CheckpointError::Invalid(
            "the checkpoint does not record a closed old Boot awaiting its new Boot linkage"
                .to_owned(),
        ));
    }

    Ok(summary_of(&manifest, agent_manifest.artifacts.len()))
}

/// Restore a verified checkpoint into an isolated directory and prove the
/// restored copy is the same closed-Boot checkpoint. No live database, no
/// collector, no ingestion, no Admin write and no external-effect worker is
/// touched.
pub async fn restore_checkpoint(
    checkpoint: &Path,
    restore_directory: &Path,
) -> Result<CheckpointSummary, CheckpointError> {
    let _verified = verify_checkpoint(checkpoint).await?;

    let absolute_checkpoint = std::path::absolute(checkpoint)?;
    let absolute_restore = std::path::absolute(restore_directory)?;
    if absolute_restore.starts_with(&absolute_checkpoint)
        || absolute_checkpoint.starts_with(&absolute_restore)
    {
        return Err(CheckpointError::UnsafeDirectory(
            "the restore directory must be separate from the checkpoint directory".to_owned(),
        ));
    }
    let created_directory = prepare_directory(restore_directory)?;
    if let Err(error) = copy_tree(checkpoint, restore_directory) {
        cleanup_directory(restore_directory, created_directory);
        return Err(error);
    }
    // Re-verify the restored copy in isolation: this is the proof that the
    // checkpoint restores the same closed old Boot and preserves the original
    // Report Receipt, deletion identity and declaration evidence.
    let restored = match verify_checkpoint(restore_directory).await {
        Ok(restored) => restored,
        Err(error) => {
            cleanup_directory(restore_directory, created_directory);
            return Err(error);
        }
    };
    // The preserved configuration files still name the original live paths.
    // Add self-contained isolated copies so the restored deployment can be
    // pointed at without reopening the production database.
    if let Err(error) = write_isolated_configs(restore_directory).await {
        cleanup_directory(restore_directory, created_directory);
        return Err(error);
    }
    Ok(restored)
}

/// Write path-rewritten config copies for the isolated restore, leaving the
/// preserved originals untouched.
async fn write_isolated_configs(restore_directory: &Path) -> Result<(), CheckpointError> {
    let manifest: CoordinationCheckpointManifest =
        serde_json::from_slice(&std::fs::read(restore_directory.join(CHECKPOINT_MANIFEST))?)?;
    let server_directory = safe_child(restore_directory, &manifest.server_directory)?;
    let server_isolated = format!(
        r#"state_dir = "{server}"
db_path = "{db}"
pepper_file = "{pepper}"
"#,
        server = server_directory.display(),
        db = server_directory.join("server.db").display(),
        pepper = server_directory.join("server-pepper").display(),
    );
    write_private(
        &server_directory.join("server.isolated.toml"),
        server_isolated.as_bytes(),
    )?;

    let agent_directory = safe_child(restore_directory, &manifest.agent_directory)?;
    let original = std::fs::read_to_string(agent_directory.join("agent.toml"))?;
    let mut value: toml::Value = toml::from_str(&original).map_err(|error| {
        CheckpointError::Invalid(format!("the preserved agent.toml is invalid: {error}"))
    })?;
    let table = value.as_table_mut().ok_or_else(|| {
        CheckpointError::Invalid("the preserved agent.toml is not a table".to_owned())
    })?;
    table.insert(
        "credential_file".to_owned(),
        toml::Value::String(agent_directory.join("credential").display().to_string()),
    );
    table.insert(
        "state_db".to_owned(),
        toml::Value::String(
            agent_directory
                .join("agent-store.sqlite")
                .display()
                .to_string(),
        ),
    );
    let agent_isolated = toml::to_string(&value).map_err(|error| {
        CheckpointError::Invalid(format!("cannot serialize the isolated agent.toml: {error}"))
    })?;
    write_private(
        &agent_directory.join("agent.isolated.toml"),
        agent_isolated.as_bytes(),
    )?;
    Ok(())
}

async fn verify_agent_half(
    directory: &Path,
) -> Result<(AgentCheckpointManifest, AgentBinding), CheckpointError> {
    let manifest_path = directory.join(AGENT_MANIFEST);
    let bytes = std::fs::read(&manifest_path).map_err(|error| {
        CheckpointError::Invalid(format!("cannot read {}: {error}", manifest_path.display()))
    })?;
    let manifest: AgentCheckpointManifest = serde_json::from_slice(&bytes)?;
    if manifest.kind != AGENT_MANIFEST_KIND || manifest.format_version != 1 {
        return Err(CheckpointError::Invalid(format!(
            "unexpected Agent checkpoint kind {} format {}",
            manifest.kind, manifest.format_version
        )));
    }
    if manifest.inventory_protocol_major != 1 || manifest.accepted_inventory_protocol_major != 1 {
        return Err(CheckpointError::Invalid(
            "the Agent checkpoint is not a frozen-v1 declaration".to_owned(),
        ));
    }
    if manifest.pending_transition != "drained_previous" {
        return Err(CheckpointError::Invalid(
            "the Agent checkpoint does not record a drained_previous transition".to_owned(),
        ));
    }
    if !matches!(
        manifest.closing_receipt_disposition.as_str(),
        "accepted" | "partially_accepted"
    ) {
        return Err(CheckpointError::Invalid(
            "the Agent checkpoint did not complete an accepted Closing".to_owned(),
        ));
    }
    if sha256_bytes(manifest.declaration_json.as_bytes()) != manifest.declaration_sha256 {
        return Err(CheckpointError::Invalid(
            "the declared frozen-v1 declaration does not match its recorded hash".to_owned(),
        ));
    }
    let declaration: NodeInventory = serde_json::from_str(&manifest.declaration_json)?;
    if declaration.content_sha256().to_string() != manifest.inventory_sha256 {
        return Err(CheckpointError::Invalid(
            "the declared Inventory does not hash to the recorded v1 hash".to_owned(),
        ));
    }
    if declaration.revision as i64 != manifest.inventory_revision {
        return Err(CheckpointError::Invalid(
            "the declared Inventory revision does not match the evidence".to_owned(),
        ));
    }
    if manifest.accepted_inventory_revision != manifest.inventory_revision
        || manifest.accepted_inventory_sha256 != manifest.inventory_sha256
    {
        return Err(CheckpointError::Invalid(
            "the Agent checkpoint's accepted baseline differs from its declaration".to_owned(),
        ));
    }
    verify_artifacts(directory, &manifest.artifacts)?;
    require_roles(
        &manifest.artifacts,
        &[AGENT_ROLE_STORE, AGENT_ROLE_CONFIG, AGENT_ROLE_CREDENTIAL],
    )?;
    let store_path = artifact_path(directory, &manifest.artifacts, AGENT_ROLE_STORE)?;
    verify_agent_store(&store_path, &manifest).await?;
    let binding = AgentBinding::of(&manifest);
    Ok((manifest, binding))
}

async fn verify_agent_store(
    path: &Path,
    manifest: &AgentCheckpointManifest,
) -> Result<(), CheckpointError> {
    let mut connection = open_read_only(path).await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await?;
    if integrity != "ok" {
        connection.close().await?;
        return Err(CheckpointError::Invalid(format!(
            "the Agent Store snapshot failed its integrity check: {integrity}"
        )));
    }
    let state: AgentStateRow = sqlx::query_as(AGENT_STATE_SELECT)
        .fetch_one(&mut connection)
        .await
        .map_err(|_| {
            CheckpointError::Invalid("the Agent Store snapshot has no enrollment state".to_owned())
        })?;
    let evidence: PreparationRow = sqlx::query_as(PREPARATION_SELECT)
        .fetch_one(&mut connection)
        .await
        .map_err(|_| {
            CheckpointError::Invalid(
                "the Agent Store snapshot has no verified preparation evidence".to_owned(),
            )
        })?;
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports")
        .fetch_one(&mut connection)
        .await?;
    connection.close().await?;

    // Issue #189 drains the immutable backlog; any remaining report means the
    // Agent produced new work after preparation.
    if queued != 0 {
        return Err(CheckpointError::Invalid(format!(
            "the Agent Store snapshot still holds {queued} undelivered report(s)"
        )));
    }
    if state.agent_id.as_deref() != Some(manifest.agent_id.as_str())
        || state.agent_epoch != manifest.agent_epoch
        || state.boot_id.as_deref() != Some(manifest.next_boot_id.as_str())
        || state.boot_state != "drained_pending"
        || state.previous_boot_id.as_deref() != Some(manifest.closed_boot_id.as_str())
        || state.pending_previous_boot_id.as_deref() != Some(manifest.closed_boot_id.as_str())
        || state.pending_transition.as_deref() != Some("drained_previous")
        || state.close_report_id.as_deref() != Some(manifest.closing_report_id.as_str())
    {
        return Err(CheckpointError::Invalid(
            "the Agent Store snapshot does not hold the prepared closed-Boot state".to_owned(),
        ));
    }
    let body = state.last_report_body.as_deref().ok_or_else(|| {
        CheckpointError::Invalid(
            "the Agent Store snapshot lost the original Closing report body".to_owned(),
        )
    })?;
    if format!("0x{}", sha256_bytes(body)) != manifest.closing_report_body_sha256 {
        return Err(CheckpointError::Invalid(
            "the Agent Store snapshot's original Closing report body does not match its hash"
                .to_owned(),
        ));
    }
    let report: platpulse_core::AgentReport = serde_json::from_slice(body)?;
    if report.report_id.to_string() != manifest.closing_report_id {
        return Err(CheckpointError::Invalid(
            "the Agent Store snapshot's preserved report is not the prepared Closing".to_owned(),
        ));
    }

    if evidence.agent_id != manifest.agent_id
        || evidence.agent_epoch != manifest.agent_epoch
        || evidence.closing_report_id != manifest.closing_report_id
        || evidence.closing_report_sequence != manifest.closing_report_sequence
        || evidence.closing_receipt_disposition != manifest.closing_receipt_disposition
        || evidence.inventory_revision != manifest.inventory_revision
        || evidence.inventory_sha256 != manifest.inventory_sha256
        || evidence.declaration_json != manifest.declaration_json
        || evidence.closed_boot_id != manifest.closed_boot_id
        || evidence.next_boot_id != manifest.next_boot_id
        || evidence.pending_transition != manifest.pending_transition
        || evidence.accepted_inventory_revision != manifest.accepted_inventory_revision
        || evidence.accepted_inventory_sha256 != manifest.accepted_inventory_sha256
        || evidence.accepted_inventory_protocol_major != manifest.accepted_inventory_protocol_major
    {
        return Err(CheckpointError::Invalid(
            "the Agent Store snapshot's preparation evidence does not match the manifest"
                .to_owned(),
        ));
    }
    Ok(())
}

async fn collect_server_facts(
    connection: &mut SqliteConnection,
    agent_id: &str,
) -> Result<ServerFacts, CheckpointError> {
    let row: Option<AgentRow> = sqlx::query_as(
        "SELECT agent_epoch, active_boot_id, active_boot_status, previous_boot_id, close_report_id, last_report_sequence, last_inventory_revision, inventory_sha256, inventory_protocol_major FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&mut *connection)
    .await?;
    let row = row.ok_or_else(|| {
        CheckpointError::Invalid(
            "the Agent is not a live Server identity; it may have been removed".to_owned(),
        )
    })?;

    let receipt = match row.close_report_id.as_deref() {
        Some(report_id) => {
            sqlx::query_as::<_, ReceiptRow>(
                "SELECT disposition, report_body_sha256, receipt_body FROM agent_report_receipts WHERE report_id = ?",
            )
            .bind(report_id)
            .fetch_optional(&mut *connection)
            .await?
        }
        None => None,
    };
    let closed_boot_row_status = match row.active_boot_id.as_deref() {
        Some(boot_id) => sqlx::query_scalar::<_, String>(
            "SELECT status FROM agent_boots WHERE agent_id = ? AND agent_epoch = ? AND boot_id = ?",
        )
        .bind(agent_id)
        .bind(row.agent_epoch)
        .bind(boot_id)
        .fetch_optional(&mut *connection)
        .await?,
        None => None,
    };
    let deleted_node_ids: Vec<String> =
        sqlx::query_scalar("SELECT node_id FROM deleted_nodes WHERE agent_id = ? ORDER BY node_id")
            .bind(agent_id)
            .fetch_all(&mut *connection)
            .await?;

    Ok(ServerFacts {
        agent_id: agent_id.to_owned(),
        agent_epoch: row.agent_epoch,
        accepted_inventory_revision: row.last_inventory_revision,
        accepted_inventory_sha256: row.inventory_sha256,
        inventory_protocol_major: row.inventory_protocol_major,
        active_boot_id: row.active_boot_id,
        active_boot_status: row.active_boot_status,
        previous_boot_id: row.previous_boot_id,
        close_report_id: row.close_report_id,
        close_report_disposition: receipt.as_ref().map(|value| value.disposition.clone()),
        last_report_sequence: row.last_report_sequence,
        closing_receipt_report_body_sha256: receipt
            .as_ref()
            .map(|value| value.report_body_sha256.clone()),
        closing_receipt_body_sha256: receipt
            .as_ref()
            .map(|value| sha256_bytes(&value.receipt_body)),
        closed_boot_row_status,
        deleted_node_ids,
    })
}

async fn collect_server_facts_in_file(
    path: &Path,
    agent_id: &str,
) -> Result<ServerFacts, CheckpointError> {
    let mut connection = open_read_only(path).await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await?;
    if integrity != "ok" {
        connection.close().await?;
        return Err(CheckpointError::Invalid(format!(
            "the Server database snapshot failed its integrity check: {integrity}"
        )));
    }
    let facts = collect_server_facts(&mut connection, agent_id).await?;
    connection.close().await?;
    Ok(facts)
}

fn cross_check(agent: &AgentBinding, server: &ServerFacts) -> Result<(), CheckpointError> {
    if server.agent_id != agent.agent_id {
        return Err(CheckpointError::Invalid(
            "the two halves describe different Agent identities".to_owned(),
        ));
    }
    if server.agent_epoch != agent.agent_epoch {
        return Err(CheckpointError::Invalid(
            "the two halves describe different Agent Epochs".to_owned(),
        ));
    }
    if server.accepted_inventory_revision != agent.inventory_revision
        || server.accepted_inventory_sha256.as_deref() != Some(agent.inventory_sha256.as_str())
    {
        return Err(CheckpointError::Invalid(
            "the Server's accepted Inventory does not match the Agent declaration".to_owned(),
        ));
    }
    if server.inventory_protocol_major != Some(agent.accepted_inventory_protocol_major) {
        return Err(CheckpointError::Invalid(
            "the Server's accepted Inventory protocol does not match the Agent evidence".to_owned(),
        ));
    }
    if server.active_boot_id.as_deref() != Some(agent.closed_boot_id.as_str())
        || server.active_boot_status != "closed"
    {
        return Err(CheckpointError::Invalid(
            "the Server's active Boot is not the Agent's closed Boot".to_owned(),
        ));
    }
    if server.close_report_id.as_deref() != Some(agent.closing_report_id.as_str()) {
        return Err(CheckpointError::Invalid(
            "the Server closed the Boot with a different report".to_owned(),
        ));
    }
    if !matches!(
        server.close_report_disposition.as_deref(),
        Some("accepted" | "partially_accepted")
    ) || server.close_report_disposition.as_deref()
        != Some(agent.closing_receipt_disposition.as_str())
    {
        return Err(CheckpointError::Invalid(
            "the Server's Closing receipt is not the Agent's accepted Closing".to_owned(),
        ));
    }
    if server.last_report_sequence != Some(agent.closing_report_sequence) {
        return Err(CheckpointError::Invalid(
            "the Server's last report sequence does not match the Closing".to_owned(),
        ));
    }
    if server.closing_receipt_report_body_sha256.as_deref()
        != Some(agent.closing_report_body_sha256.as_str())
    {
        return Err(CheckpointError::Invalid(
            "the Server's original Closing Report bytes differ from the Agent's".to_owned(),
        ));
    }
    if server.closing_receipt_body_sha256.is_none() {
        return Err(CheckpointError::Invalid(
            "the Server holds no original Report Receipt for the Closing".to_owned(),
        ));
    }
    if server.closed_boot_row_status.as_deref() != Some("closed") {
        return Err(CheckpointError::Invalid(
            "the Server's Boot record is not closed".to_owned(),
        ));
    }
    Ok(())
}

async fn open_live_database(path: &Path) -> Result<SqliteConnection, CheckpointError> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false);
    SqliteConnection::connect_with(&options)
        .await
        .map_err(CheckpointError::from)
}

async fn open_read_only(path: &Path) -> Result<SqliteConnection, CheckpointError> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    SqliteConnection::connect_with(&options)
        .await
        .map_err(CheckpointError::from)
}

fn require_roles(artifacts: &[CheckpointArtifact], roles: &[&str]) -> Result<(), CheckpointError> {
    for role in roles {
        if !artifacts.iter().any(|artifact| artifact.role == *role) {
            return Err(CheckpointError::Invalid(format!(
                "the checkpoint is missing its {role} artifact"
            )));
        }
    }
    Ok(())
}

fn verify_artifacts(base: &Path, artifacts: &[CheckpointArtifact]) -> Result<(), CheckpointError> {
    for artifact in artifacts {
        let path = safe_child(base, &artifact.file)?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| {
            CheckpointError::Invalid(format!("the {} artifact is missing", artifact.role))
        })?;
        if !metadata.is_file() {
            return Err(CheckpointError::Invalid(format!(
                "the {} artifact is not a regular file",
                artifact.role
            )));
        }
        if metadata.len() as i64 != artifact.bytes {
            return Err(CheckpointError::Invalid(format!(
                "the {} artifact size changed",
                artifact.role
            )));
        }
        let observed = sha256_file(&path)?;
        if observed != artifact.sha256 {
            return Err(CheckpointError::Invalid(format!(
                "the {} artifact content changed ({} vs {})",
                artifact.role, observed, artifact.sha256
            )));
        }
    }
    Ok(())
}

fn artifact_path(
    base: &Path,
    artifacts: &[CheckpointArtifact],
    role: &str,
) -> Result<PathBuf, CheckpointError> {
    let artifact = artifacts
        .iter()
        .find(|artifact| artifact.role == role)
        .ok_or_else(|| CheckpointError::Invalid(format!("the checkpoint is missing its {role}")))?;
    safe_child(base, &artifact.file)
}

fn safe_child(base: &Path, name: &str) -> Result<PathBuf, CheckpointError> {
    if !crate::file_security::is_safe_basename(name) {
        return Err(CheckpointError::Invalid(format!(
            "unsafe checkpoint entry name {name}"
        )));
    }
    Ok(base.join(name))
}

fn prepare_directory(path: &Path) -> Result<bool, CheckpointError> {
    if path.as_os_str().is_empty() {
        return Err(CheckpointError::UnsafeDirectory(
            "the directory must not be empty".to_owned(),
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(CheckpointError::UnsafeDirectory(format!(
                    "{} is not a directory",
                    path.display()
                )));
            }
            if std::fs::read_dir(path)?.next().is_some() {
                return Err(CheckpointError::UnsafeDirectory(format!(
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
        Err(error) => Err(CheckpointError::Io(error)),
    }
}

fn cleanup_directory(path: &Path, created: bool) {
    if created {
        let _ = std::fs::remove_dir_all(path);
    } else {
        for entry in [CHECKPOINT_MANIFEST, AGENT_DIRECTORY, SERVER_DIRECTORY] {
            let target = path.join(entry);
            if target.is_dir() {
                let _ = std::fs::remove_dir_all(&target);
            } else {
                let _ = std::fs::remove_file(&target);
            }
        }
    }
}

fn create_private_directory(path: &Path) -> Result<(), CheckpointError> {
    std::fs::create_dir_all(path)?;
    secure_directory(path)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), CheckpointError> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CheckpointError::Invalid(format!(
            "{} is not a directory",
            source.display()
        )));
    }
    create_private_directory(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if file_type.is_file() {
            let bytes = std::fs::read(entry.path())?;
            write_private(&target, &bytes)?;
        } else {
            return Err(CheckpointError::Invalid(format!(
                "unsupported file type in checkpoint: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn artifact(
    role: &str,
    original_path: &Path,
    file: &str,
    path: &Path,
) -> Result<CheckpointArtifact, CheckpointError> {
    let bytes = std::fs::metadata(path)?.len();
    let sha256 = sha256_file(path)?;
    Ok(CheckpointArtifact {
        role: role.to_owned(),
        original_path: original_path.display().to_string(),
        file: file.to_owned(),
        bytes: bytes as i64,
        sha256,
    })
}

fn sha256_file(path: &Path) -> Result<String, CheckpointError> {
    let mut reader = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut reader, &mut hasher)?;
    Ok(hex_digest(hasher.finalize()))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn copy_file_private(source: &Path, destination: &Path) -> Result<(), CheckpointError> {
    let bytes = std::fs::read(source)?;
    write_private(destination, &bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CheckpointError> {
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
fn secure_file(path: &Path) -> Result<(), CheckpointError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) -> Result<(), CheckpointError> {
    Ok(())
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> Result<(), CheckpointError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> Result<(), CheckpointError> {
    Ok(())
}

fn summary_of(
    manifest: &CoordinationCheckpointManifest,
    agent_artifacts: usize,
) -> CheckpointSummary {
    CheckpointSummary {
        agent_id: manifest.agent.agent_id.clone(),
        agent_epoch: manifest.agent.agent_epoch,
        closed_boot_id: manifest.agent.closed_boot_id.clone(),
        next_boot_id: manifest.agent.next_boot_id.clone(),
        inventory_revision: manifest.agent.inventory_revision,
        inventory_sha256: manifest.agent.inventory_sha256.clone(),
        closing_report_id: manifest.agent.closing_report_id.clone(),
        deleted_nodes: manifest.server.deleted_node_ids.len(),
        agent_artifacts,
        server_artifacts: manifest.server.artifacts.len(),
    }
}
