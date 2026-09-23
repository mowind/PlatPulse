//! Coordinated-checkpoint Agent half (issue #190).
//!
//! [ADR 0007](../../docs/adr/0007-server-managed-inventory-revision.md) and
//! [main design 15.10.4](../../docs/design/platpulse.md#server-managed-inventory-revision-target)
//! require a coordinated v1 -> v2 cutover. Issue #189 delivers the preparation
//! bridge: stop ordinary reporting, drain the immutable backlog, complete the
//! final Closing, and prove its original v1 declaration against the Server's
//! last accepted revision/hash. This module delivers the Agent half of the
//! coordinated checkpoint that the conversion (a later ticket) must be able to
//! restore exactly: the complete Agent Store, the old configuration, the Agent
//! credential, and the frozen-v1 declaration evidence, all bound into a single
//! verifiable manifest.
//!
//! The checkpoint is deliberately not the sanitized Server Backup. It exists to
//! reproduce the old stopped state exactly, so it keeps the original
//! Report Receipts, Agent identity, credentials, deletion identity and Boot
//! linkage instead of redacting or rewriting them. Every artifact is written
//! owner-only and no credential value is ever printed.
//!
//! The Agent half never touches the Server database. The Server half
//! (platpulse-server checkpoint create) consumes this directory, verifies it
//! against its own accepted baseline, and binds both halves into one
//! coordinated manifest.

use std::path::{Path, PathBuf};

use platpulse_core::inventory::NodeInventory;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sqlx::Connection;
use thiserror::Error;

use crate::config::AgentConfig;
use crate::database::{
    AgentDatabaseConfig, AgentRuntimeLock, AgentStore, AgentStoreWritePermit, now_rfc3339,
};

/// Format version of the Agent checkpoint manifest. Bumped when the manifest
/// or artifact layout changes incompatibly.
pub const AGENT_CHECKPOINT_FORMAT_VERSION: u32 = 1;

/// Manifest kind discriminator, so a Server can refuse an unrelated JSON file.
pub const AGENT_CHECKPOINT_KIND: &str = "platpulse.upgrade-checkpoint.agent";

/// Manifest file written inside the Agent checkpoint directory.
pub const AGENT_CHECKPOINT_MANIFEST: &str = "agent-checkpoint.json";

/// Artifact role of the exact Agent Store snapshot.
pub const ROLE_AGENT_STORE: &str = "agent-store";
/// Artifact role of the original agent.toml.
pub const ROLE_AGENT_CONFIG: &str = "agent-config";
/// Artifact role of the Agent credential file.
pub const ROLE_AGENT_CREDENTIAL: &str = "agent-credential";

/// One file the checkpoint preserves, with the identity the verifier needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointArtifact {
    /// Stable role (agent-store, agent-config, agent-credential, ...).
    pub role: String,
    /// The path the artifact had in the live deployment.
    pub original_path: String,
    /// File name relative to the checkpoint directory.
    pub file: String,
    /// Exact byte length.
    pub bytes: i64,
    /// Lowercase hex SHA-256 of the exact bytes.
    pub sha256: String,
}

/// The Agent half of one coordinated checkpoint.
///
/// It carries both the preserved artifacts and the frozen-v1 migration
/// evidence, so a verifier can bind identity, Boot linkage and evidence without
/// trusting the live configuration or the Server's projections.
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
    /// SHA-256 of the exact declaration_json bytes below.
    pub declaration_sha256: String,
    /// Frozen v1 declaration exactly as hashed: revision plus declared order.
    pub declaration_json: String,
    /// SHA-256 of the original immutable Closing report body preserved in the
    /// Agent Store. The Server independently holds the same hash in its
    /// Report Receipt, binding the two halves to one exact report.
    pub closing_report_body_sha256: String,
    pub artifacts: Vec<CheckpointArtifact>,
}

impl AgentCheckpointManifest {
    /// The declaration re-parsed from the evidence, for independent hashing.
    pub fn declaration(&self) -> Result<NodeInventory, serde_json::Error> {
        serde_json::from_str(&self.declaration_json)
    }
}

/// Operator-facing summary of a completed Agent checkpoint (never a secret).
#[derive(Debug, Clone)]
pub struct AgentCheckpointOutcome {
    pub directory: PathBuf,
    pub manifest_path: PathBuf,
    pub agent_id: String,
    pub closed_boot_id: String,
    pub next_boot_id: String,
    pub inventory_revision: i64,
    pub inventory_sha256: String,
    pub artifacts: usize,
}

#[derive(Debug, Error)]
pub enum AgentCheckpointError {
    #[error(transparent)]
    Config(#[from] crate::config::AgentConfigError),
    #[error(
        "the Agent runtime owns this Store; stop platpulse-agent run before taking the upgrade checkpoint ({0})"
    )]
    RuntimeOwned(String),
    #[error("{0}")]
    NotPrepared(String),
    #[error("Agent state changed during the checkpoint: {0}")]
    StateChanged(String),
    #[error("Agent Store initialization failed: {0}")]
    Store(#[from] crate::database::AgentDatabaseError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("checkpoint IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checkpoint serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("checkpoint directory is unsafe or already exists: {0}")]
    UnsafeDirectory(String),
    #[error("the prepared migration evidence is invalid: {0}")]
    InvalidEvidence(String),
    #[error("the Agent credential file is invalid: {0}")]
    Credential(String),
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct StoredState {
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

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct StoredEvidence {
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

/// Take the Agent half of the coordinated upgrade checkpoint.
///
/// Refuses while another Agent runtime owns the Store (checked before SQLite
/// opens), refuses when issue #189's verified preparation evidence is missing
/// or no longer matches the stored Boot, and never leaves a partial checkpoint
/// behind on failure.
pub async fn create_agent_checkpoint(
    config: &AgentConfig,
    output: &Path,
) -> Result<AgentCheckpointOutcome, AgentCheckpointError> {
    // Read the frozen-v1 configuration exactly once, then validate and preserve
    // those same bytes: an edit between validation and copying could otherwise
    // leave a checkpoint whose original configuration disagrees with the
    // declaration evidence it was verified against.
    let config_bytes = std::fs::read(&config.config_path)?;
    let text = std::str::from_utf8(&config_bytes).map_err(|_| {
        AgentCheckpointError::NotPrepared("agent.toml is not valid UTF-8".to_owned())
    })?;
    let parsed: crate::config::AgentConfigFile = toml::from_str(text).map_err(|source| {
        AgentCheckpointError::Config(crate::config::AgentConfigError::Parse {
            path: config.config_path.clone(),
            source,
        })
    })?;
    if parsed.state_db != config.state_db || parsed.credential_file != config.credential_file {
        return Err(AgentCheckpointError::StateChanged(
            "agent.toml changed while it was being read; retry the checkpoint".to_owned(),
        ));
    }
    let validated = parsed.validate()?;
    if validated.server_managed_inventory {
        return Err(AgentCheckpointError::NotPrepared(
            "this configuration is already Server-managed (v2); there is no frozen v1 checkpoint to take"
                .to_owned(),
        ));
    }

    // Process ownership/offline mutex: refuse before SQLite opens.
    let _runtime_lock = AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| AgentCheckpointError::RuntimeOwned(error.to_string()))?;

    let created_directory = prepare_output_directory(output)?;

    let result = create_inner(config, &config_bytes, &validated.inventory, output).await;
    if result.is_err() {
        // Never leave a partial artifact set that could be mistaken for a
        // valid checkpoint; the live Store is untouched either way.
        cleanup_output(output, created_directory);
    }
    result
}

fn prepare_output_directory(output: &Path) -> Result<bool, AgentCheckpointError> {
    if output.as_os_str().is_empty() {
        return Err(AgentCheckpointError::UnsafeDirectory(
            "the checkpoint directory must not be empty".to_owned(),
        ));
    }
    match std::fs::symlink_metadata(output) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AgentCheckpointError::UnsafeDirectory(format!(
                    "{} is not a directory",
                    output.display()
                )));
            }
            let mut entries = std::fs::read_dir(output)?;
            if entries.next().is_some() {
                return Err(AgentCheckpointError::UnsafeDirectory(format!(
                    "{} already exists and is not empty",
                    output.display()
                )));
            }
            secure_directory(output)?;
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(output)?;
            secure_directory(output)?;
            Ok(true)
        }
        Err(error) => Err(AgentCheckpointError::Io(error)),
    }
}

fn cleanup_output(output: &Path, created_directory: bool) {
    if created_directory {
        let _ = std::fs::remove_dir_all(output);
    } else {
        for entry in [
            AGENT_CHECKPOINT_MANIFEST,
            "agent-store.sqlite",
            "agent.toml",
            "credential",
        ] {
            let _ = std::fs::remove_file(output.join(entry));
        }
    }
}

async fn create_inner(
    config: &AgentConfig,
    config_bytes: &[u8],
    inventory: &NodeInventory,
    output: &Path,
) -> Result<AgentCheckpointOutcome, AgentCheckpointError> {
    let write_permit = AgentStoreWritePermit::new();
    let mut store = AgentStore::open_with_write_permit(
        AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await?;

    let state = load_state(&mut store).await?;
    let evidence = load_evidence(&mut store).await?;
    require_prepared_state(&state, &evidence, inventory)?;
    // Issue #189 drains the immutable backlog and completes the Closing; a
    // non-empty spool means the Agent produced new work after preparation and
    // the two halves no longer describe one quiesced checkpoint.
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports")
        .fetch_one(store.connection())
        .await?;
    if queued != 0 {
        return Err(AgentCheckpointError::NotPrepared(format!(
            "the Agent Store still holds {queued} undelivered report(s); finish the preparation drain first"
        )));
    }
    let closing_report_body_sha256 = closing_report_hash(&state, &evidence)?;

    // Exact consistent snapshot of the live Store. VACUUM INTO reads the
    // committed logical content (including any valid uncheckpointed WAL pages)
    // and never rewrites the live WAL/SHM.
    let store_path = output.join("agent-store.sqlite");
    vacuum_into(&mut store, &store_path).await?;

    let config_path = output.join("agent.toml");
    // Preserve exactly the bytes that were validated above.
    write_private(&config_path, config_bytes)?;
    let credential_path = output.join("credential");
    // Validate the credential file's safety/ownership before it is preserved.
    crate::credential::load_credential_file(&config.credential_file)
        .map_err(|error| AgentCheckpointError::Credential(error.to_string()))?;
    copy_artifact(&config.credential_file, &credential_path)?;

    // Confirm the snapshot really reflects the state that was just verified,
    // so a late writer cannot be silently captured mid-change.
    verify_snapshot(&store_path, &state, &evidence).await?;

    let declaration_sha256 = hex_sha256(evidence.declaration_json.as_bytes());
    let mut artifacts = vec![
        artifact(
            ROLE_AGENT_STORE,
            &config.state_db,
            "agent-store.sqlite",
            &store_path,
        )?,
        artifact(
            ROLE_AGENT_CONFIG,
            &config.config_path,
            "agent.toml",
            &config_path,
        )?,
        artifact(
            ROLE_AGENT_CREDENTIAL,
            &config.credential_file,
            "credential",
            &credential_path,
        )?,
    ];
    artifacts.sort_by(|left, right| left.role.cmp(&right.role));

    let manifest = AgentCheckpointManifest {
        kind: AGENT_CHECKPOINT_KIND.to_owned(),
        format_version: AGENT_CHECKPOINT_FORMAT_VERSION,
        agent_version: crate::VERSION.to_owned(),
        created_at: now_rfc3339(),
        agent_id: evidence.agent_id.clone(),
        agent_epoch: evidence.agent_epoch,
        inventory_protocol_major: 1,
        closed_boot_id: evidence.closed_boot_id.clone(),
        next_boot_id: evidence.next_boot_id.clone(),
        pending_transition: evidence.pending_transition.clone(),
        closing_report_id: evidence.closing_report_id.clone(),
        closing_report_sequence: evidence.closing_report_sequence,
        closing_receipt_disposition: evidence.closing_receipt_disposition.clone(),
        inventory_revision: evidence.inventory_revision,
        inventory_sha256: evidence.inventory_sha256.clone(),
        accepted_inventory_revision: evidence.accepted_inventory_revision,
        accepted_inventory_sha256: evidence.accepted_inventory_sha256.clone(),
        accepted_inventory_protocol_major: evidence.accepted_inventory_protocol_major,
        declaration_sha256,
        declaration_json: evidence.declaration_json.clone(),
        closing_report_body_sha256,
        artifacts,
    };
    let manifest_path = output.join(AGENT_CHECKPOINT_MANIFEST);
    write_private(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
    store.close().await?;

    Ok(AgentCheckpointOutcome {
        directory: output.to_path_buf(),
        manifest_path,
        agent_id: manifest.agent_id,
        closed_boot_id: manifest.closed_boot_id,
        next_boot_id: manifest.next_boot_id,
        inventory_revision: manifest.inventory_revision,
        inventory_sha256: manifest.inventory_sha256,
        artifacts: manifest.artifacts.len(),
    })
}

async fn load_state(store: &mut AgentStore) -> Result<StoredState, AgentCheckpointError> {
    sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id, last_report_body FROM agent_state WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?
    .ok_or_else(|| AgentCheckpointError::NotPrepared("the Agent Store is not enrolled".to_owned()))
}

async fn load_evidence(store: &mut AgentStore) -> Result<StoredEvidence, AgentCheckpointError> {
    sqlx::query_as(
        "SELECT agent_id, agent_epoch, closing_report_id, closing_report_sequence, closing_receipt_disposition, inventory_revision, inventory_sha256, declaration_json, closed_boot_id, next_boot_id, pending_transition, accepted_inventory_revision, accepted_inventory_sha256, accepted_inventory_protocol_major FROM upgrade_preparation WHERE singleton=1",
    )
    .fetch_optional(store.connection())
    .await?
    .ok_or_else(|| {
        AgentCheckpointError::NotPrepared(
            "no verified upgrade preparation exists; run platpulse-agent prepare-upgrade first"
                .to_owned(),
        )
    })
}

fn require_prepared_state(
    state: &StoredState,
    evidence: &StoredEvidence,
    inventory: &NodeInventory,
) -> Result<(), AgentCheckpointError> {
    if state.boot_state != "drained_pending"
        || state.pending_transition.as_deref() != Some("drained_previous")
    {
        return Err(AgentCheckpointError::NotPrepared(format!(
            "the Agent Boot is {} rather than the post-Closing drained_pending checkpoint; run platpulse-agent prepare-upgrade first",
            state.boot_state
        )));
    }
    let agent_id = state.agent_id.as_deref().ok_or_else(|| {
        AgentCheckpointError::NotPrepared("the Agent Store is not enrolled".to_owned())
    })?;
    if agent_id != evidence.agent_id || state.agent_epoch != evidence.agent_epoch {
        return Err(AgentCheckpointError::InvalidEvidence(
            "the preparation evidence belongs to another Agent identity or Epoch".to_owned(),
        ));
    }
    if state.boot_id.as_deref() != Some(evidence.next_boot_id.as_str()) {
        return Err(AgentCheckpointError::StateChanged(
            "the current Boot is not the preparation's pending successor Boot".to_owned(),
        ));
    }
    if state.previous_boot_id.as_deref() != Some(evidence.closed_boot_id.as_str())
        || state.pending_previous_boot_id.as_deref() != Some(evidence.closed_boot_id.as_str())
    {
        return Err(AgentCheckpointError::StateChanged(
            "the closed Boot linkage no longer matches the preparation evidence".to_owned(),
        ));
    }
    if state.close_report_id.as_deref() != Some(evidence.closing_report_id.as_str()) {
        return Err(AgentCheckpointError::StateChanged(
            "the stored Closing report no longer matches the preparation evidence".to_owned(),
        ));
    }
    if evidence.pending_transition != "drained_previous" {
        return Err(AgentCheckpointError::InvalidEvidence(
            "the preparation evidence does not record a drained_previous transition".to_owned(),
        ));
    }
    // The configuration that will be preserved must be exactly the frozen v1
    // declaration the Server accepted; a later edit would make the checkpoint
    // describe a state that never existed.
    let current = inventory.content_sha256().to_string();
    if inventory.revision as i64 != evidence.inventory_revision
        || current != evidence.inventory_sha256
    {
        return Err(AgentCheckpointError::StateChanged(
            "agent.toml changed after preparation; re-run platpulse-agent prepare-upgrade before taking the checkpoint"
                .to_owned(),
        ));
    }
    // The preserved declaration must re-hash to the evidence it claims.
    let declared: NodeInventory = serde_json::from_str(&evidence.declaration_json)
        .map_err(|error| AgentCheckpointError::InvalidEvidence(error.to_string()))?;
    if declared.content_sha256().to_string() != evidence.inventory_sha256 {
        return Err(AgentCheckpointError::InvalidEvidence(
            "the stored declaration does not hash to the recorded v1 inventory hash".to_owned(),
        ));
    }
    Ok(())
}

/// Hash the original immutable Closing report preserved in the Agent Store.
///
/// The closing Report itself leaves the delivery spool once its receipt is
/// applied, so this preserved body is the Agent-side copy of the exact bytes
/// the Server acknowledged. The Server compares this hash with the
/// report_body_sha256 recorded in its own Report Receipt.
fn closing_report_hash(
    state: &StoredState,
    evidence: &StoredEvidence,
) -> Result<String, AgentCheckpointError> {
    let body = state.last_report_body.as_deref().ok_or_else(|| {
        AgentCheckpointError::InvalidEvidence(
            "the Agent Store no longer holds the original Closing report body".to_owned(),
        )
    })?;
    let report: platpulse_core::AgentReport = serde_json::from_slice(body)
        .map_err(|error| AgentCheckpointError::InvalidEvidence(error.to_string()))?;
    if report.report_id.to_string() != evidence.closing_report_id {
        return Err(AgentCheckpointError::InvalidEvidence(
            "the preserved Closing report is not the prepared Closing".to_owned(),
        ));
    }
    // The core wire encoding for report hashes is 0x-prefixed lowercase hex
    // (Sha256Hex), which is exactly what the Server stores as
    // agent_report_receipts.report_body_sha256.
    Ok(format!("0x{}", hex_sha256(body)))
}

async fn vacuum_into(
    store: &mut AgentStore,
    destination: &Path,
) -> Result<(), AgentCheckpointError> {
    if std::fs::symlink_metadata(destination).is_ok() {
        return Err(AgentCheckpointError::UnsafeDirectory(format!(
            "{} already exists",
            destination.display()
        )));
    }
    let text = destination
        .to_str()
        .ok_or_else(|| AgentCheckpointError::UnsafeDirectory("path is not UTF-8".to_owned()))?;
    let literal = format!("'{}'", text.replace('\'', "''"));
    let _write_permit = store.acquire_write().await;
    sqlx::query(&format!("VACUUM INTO {literal}"))
        .execute(store.connection())
        .await?;
    crate::database::secure_store_file(destination)?;
    Ok(())
}

async fn verify_snapshot(
    path: &Path,
    state: &StoredState,
    evidence: &StoredEvidence,
) -> Result<(), AgentCheckpointError> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options).await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await?;
    if integrity != "ok" {
        connection.close().await?;
        return Err(AgentCheckpointError::StateChanged(format!(
            "the Agent Store snapshot failed its integrity check: {integrity}"
        )));
    }
    let snapshot_state: StoredState = sqlx::query_as(
        "SELECT agent_id, agent_epoch, boot_id, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id, last_report_body FROM agent_state WHERE singleton=1",
    )
    .fetch_one(&mut connection)
    .await?;
    let snapshot_evidence: StoredEvidence = sqlx::query_as(
        "SELECT agent_id, agent_epoch, closing_report_id, closing_report_sequence, closing_receipt_disposition, inventory_revision, inventory_sha256, declaration_json, closed_boot_id, next_boot_id, pending_transition, accepted_inventory_revision, accepted_inventory_sha256, accepted_inventory_protocol_major FROM upgrade_preparation WHERE singleton=1",
    )
    .fetch_one(&mut connection)
    .await?;
    connection.close().await?;
    // Compare every captured field, not a hand-picked subset: a late writer on
    // any Boot, evidence or receipt field invalidates the snapshot.
    if &snapshot_state != state || &snapshot_evidence != evidence {
        return Err(AgentCheckpointError::StateChanged(
            "the Agent Store changed while it was being checkpointed".to_owned(),
        ));
    }
    Ok(())
}

fn artifact(
    role: &str,
    original_path: &Path,
    file: &str,
    path: &Path,
) -> Result<CheckpointArtifact, AgentCheckpointError> {
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

/// SHA-256 of a file's exact bytes, as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String, AgentCheckpointError> {
    let mut reader = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut reader, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// SHA-256 of one byte slice, as lowercase hex.
pub fn hex_sha256(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn copy_artifact(source: &Path, destination: &Path) -> Result<(), AgentCheckpointError> {
    let bytes = std::fs::read(source)?;
    write_private(destination, &bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), AgentCheckpointError> {
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
fn secure_directory(path: &Path) -> Result<(), AgentCheckpointError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> Result<(), AgentCheckpointError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;
    use crate::database::{AgentDatabaseConfig, AgentStore, AgentStoreWritePermit};
    use tempfile::TempDir;

    const NODE_ID: &str = "0195f2a1-0014-4014-8014-000000000014";
    const AGENT_ID: &str = "0195f2a1-0011-4011-8011-000000000011";
    const CLOSED_BOOT: &str = "0195f2a1-0012-4012-8012-000000000012";
    const NEXT_BOOT: &str = "0195f2a1-0013-4013-8013-000000000013";
    const CLOSING_ID: &str = "0195f2a1-0061-4061-8061-000000000061";
    const TOKEN: &str = "pp_agent_0195f2a1-0011-4011-8011-000000000011_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn write_config(dir: &Path) -> PathBuf {
        let path = dir.join("agent.toml");
        std::fs::write(
            &path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"{NODE_ID}\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:6790\"}}]\n",
                dir.join("credential").display(),
                dir.join("agent.db").display(),
            ),
        )
        .unwrap();
        path
    }

    fn closing_body() -> Vec<u8> {
        let mut report: serde_json::Value = serde_json::from_str(include_str!(
            "../../platpulse-core/tests/fixtures/report_v1_minimal.json"
        ))
        .unwrap();
        report["agent_id"] = serde_json::Value::String(AGENT_ID.to_owned());
        report["report_id"] = serde_json::Value::String(CLOSING_ID.to_owned());
        report["boot_id"] = serde_json::Value::String(CLOSED_BOOT.to_owned());
        report["report_sequence"] = serde_json::json!(4);
        report["boot_transition"] = serde_json::json!("closing");
        serde_json::to_vec(&report).unwrap()
    }

    async fn seed_prepared(config: &AgentConfig, declaration_json: &str, inventory_sha256: &str) {
        let store = AgentStore::open_with_write_permit(
            AgentDatabaseConfig::new(&config.state_db),
            AgentStoreWritePermit::new(),
        )
        .await
        .unwrap();
        let mut store = store;
        let body = closing_body();
        sqlx::query(
            "INSERT INTO agent_state (singleton, agent_id, agent_epoch, boot_id, report_sequence, inventory_revision, boot_state, previous_boot_id, pending_transition, pending_previous_boot_id, close_report_id, last_report_body, updated_at) VALUES (1, ?, 1, ?, 0, 1, 'drained_pending', ?, 'drained_previous', ?, ?, ?, ?)",
        )
        .bind(AGENT_ID)
        .bind(NEXT_BOOT)
        .bind(CLOSED_BOOT)
        .bind(CLOSED_BOOT)
        .bind(CLOSING_ID)
        .bind(&body)
        .bind(now_rfc3339())
        .execute(store.connection())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO upgrade_preparation (singleton, agent_id, agent_epoch, closing_report_id, closing_report_sequence, closing_receipt_disposition, inventory_revision, inventory_sha256, declaration_json, closed_boot_id, next_boot_id, pending_transition, accepted_inventory_revision, accepted_inventory_sha256, accepted_inventory_protocol_major, server_active_boot_id, server_active_boot_status, server_close_report_id, verified_at) VALUES (1, ?, 1, ?, 4, 'accepted', 1, ?, ?, ?, ?, 'drained_previous', 1, ?, 1, ?, 'closed', ?, ?)",
        )
        .bind(AGENT_ID)
        .bind(CLOSING_ID)
        .bind(inventory_sha256)
        .bind(declaration_json)
        .bind(CLOSED_BOOT)
        .bind(NEXT_BOOT)
        .bind(inventory_sha256)
        .bind(CLOSED_BOOT)
        .bind(CLOSING_ID)
        .bind(now_rfc3339())
        .execute(store.connection())
        .await
        .unwrap();
        store.close().await.unwrap();
    }

    async fn prepared(dir: &Path) -> (AgentConfig, String) {
        let path = write_config(dir);
        crate::credential::write_credential_file(&dir.join("credential"), TOKEN).unwrap();
        let config = AgentConfig::resolve(&path).unwrap();
        let inventory = config.validated_inventory().unwrap().inventory;
        let declaration_json = serde_json::to_string(&inventory).unwrap();
        let inventory_sha256 = inventory.content_sha256().to_string();
        seed_prepared(&config, &declaration_json, &inventory_sha256).await;
        (config, inventory_sha256)
    }

    #[tokio::test]
    async fn checkpoint_preserves_the_prepared_state_and_credential() {
        let dir = TempDir::new().unwrap();
        let (config, inventory_sha256) = prepared(dir.path()).await;
        let output = dir.path().join("checkpoint");

        let outcome = create_agent_checkpoint(&config, &output).await.unwrap();
        assert_eq!(outcome.agent_id, AGENT_ID);
        assert_eq!(outcome.closed_boot_id, CLOSED_BOOT);
        assert_eq!(outcome.next_boot_id, NEXT_BOOT);
        assert_eq!(outcome.inventory_revision, 1);
        assert_eq!(outcome.inventory_sha256, inventory_sha256);
        assert_eq!(outcome.artifacts, 3);

        let manifest_bytes = std::fs::read(output.join(AGENT_CHECKPOINT_MANIFEST)).unwrap();
        let manifest: AgentCheckpointManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest.kind, AGENT_CHECKPOINT_KIND);
        assert_eq!(manifest.pending_transition, "drained_previous");
        assert_eq!(
            manifest.closing_report_body_sha256,
            format!("0x{}", hex_sha256(&closing_body()))
        );
        assert_eq!(manifest.declaration().unwrap().revision, 1);

        // The credential is preserved verbatim but never appears in the
        // manifest or command output.
        let credential = std::fs::read_to_string(output.join("credential")).unwrap();
        assert_eq!(credential.trim_end(), TOKEN);
        let manifest_text = String::from_utf8(manifest_bytes).unwrap();
        assert!(!manifest_text.contains(TOKEN));

        for artifact in &manifest.artifacts {
            let path = output.join(&artifact.file);
            assert_eq!(
                sha256_file(&path).unwrap(),
                artifact.sha256,
                "hash for {}",
                artifact.role
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().len(),
                artifact.bytes as u64
            );
        }
        assert!(
            manifest
                .artifacts
                .iter()
                .any(|artifact| artifact.role == ROLE_AGENT_STORE)
        );
    }

    #[tokio::test]
    async fn checkpoint_refuses_without_preparation_evidence() {
        let dir = TempDir::new().unwrap();
        let path = write_config(dir.path());
        crate::credential::write_credential_file(&dir.path().join("credential"), TOKEN).unwrap();
        let config = AgentConfig::resolve(&path).unwrap();
        let inventory = config.validated_inventory().unwrap().inventory;
        let declaration_json = serde_json::to_string(&inventory).unwrap();
        let inventory_sha256 = inventory.content_sha256().to_string();
        seed_prepared(&config, &declaration_json, &inventory_sha256).await;
        {
            let mut store = AgentStore::open_with_write_permit(
                AgentDatabaseConfig::new(&config.state_db),
                AgentStoreWritePermit::new(),
            )
            .await
            .unwrap();
            sqlx::query("DELETE FROM upgrade_preparation")
                .execute(store.connection())
                .await
                .unwrap();
            store.close().await.unwrap();
        }
        let output = dir.path().join("checkpoint");
        let error = create_agent_checkpoint(&config, &output).await.unwrap_err();
        assert!(matches!(error, AgentCheckpointError::NotPrepared(_)));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn checkpoint_refuses_while_the_runtime_owns_the_store() {
        let dir = TempDir::new().unwrap();
        let (config, _) = prepared(dir.path()).await;
        let output = dir.path().join("checkpoint");
        let _held = AgentRuntimeLock::acquire(&config.state_db).unwrap();
        let error = create_agent_checkpoint(&config, &output).await.unwrap_err();
        assert!(matches!(error, AgentCheckpointError::RuntimeOwned(_)));
        assert!(!output.exists(), "a refused checkpoint created output");
    }

    #[tokio::test]
    async fn checkpoint_refuses_a_configuration_changed_after_preparation() {
        let dir = TempDir::new().unwrap();
        let (config, _) = prepared(dir.path()).await;
        // A later edit would make the preserved configuration describe a state
        // the Server never accepted.
        std::fs::write(
            &config.config_path,
            format!(
                "server_url=\"https://example.com\"\ncredential_file=\"{}\"\nstate_db=\"{}\"\ninventory_revision=1\nnodes=[{{node_id=\"{NODE_ID}\",network_key=\"platon-mainnet\",rpc_endpoint=\"ws://127.0.0.1:9999\"}}]\n",
                dir.path().join("credential").display(),
                dir.path().join("agent.db").display(),
            ),
        )
        .unwrap();
        let output = dir.path().join("checkpoint");
        let error = create_agent_checkpoint(&config, &output).await.unwrap_err();
        assert!(matches!(error, AgentCheckpointError::StateChanged(_)));
        assert!(!output.exists());
    }

    async fn read_receipt_body(path: &Path, report_id: &str) -> Vec<u8> {
        use sqlx::Connection;
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .read_only(true);
        let mut connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let body: Vec<u8> = sqlx::query_scalar(
            "SELECT receipt_body FROM agent_report_receipts WHERE report_id = ?",
        )
        .bind(report_id)
        .fetch_one(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();
        body
    }

    fn copy_tree(source: &Path, destination: &Path) {
        std::fs::create_dir_all(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn coordinated_checkpoint_binds_both_halves_and_restores_in_isolation() {
        use platpulse_server::database::{ServerDatabaseConfig, initialize};
        use platpulse_server::secrets::create_pepper_file;

        // The Agent half, produced by the real Agent operator entry point.
        let agent_dir = TempDir::new().unwrap();
        let (agent_config, inventory_sha256) = prepared(agent_dir.path()).await;
        let agent_checkpoint = agent_dir.path().join("agent-checkpoint");
        create_agent_checkpoint(&agent_config, &agent_checkpoint)
            .await
            .unwrap();

        // An independent temporary Server deployment with the prepared v1 state.
        let server_dir = TempDir::new().unwrap();
        let server_db = server_dir.path().join("server.db");
        let pepper = server_dir.path().join("server-pepper");
        let config_path = server_dir.path().join("server.toml");
        std::fs::write(
            &config_path,
            format!(
                "state_dir=\"{}\"\ndb_path=\"{}\"\npepper_file=\"{}\"\n",
                server_dir.path().display(),
                server_db.display(),
                pepper.display()
            ),
        )
        .unwrap();
        create_pepper_file(&pepper).unwrap();
        let db = initialize(ServerDatabaseConfig::for_deployment(&server_db, false))
            .await
            .unwrap();
        let body = closing_body();
        // The Server stores the wire encoding (0x-prefixed lowercase hex).
        let body_sha256 = format!("0x{}", hex_sha256(&body));
        let now = "2026-08-12T09:00:00Z";
        sqlx::query(
            "INSERT INTO agents (agent_id, agent_epoch, active_boot_id, active_boot_status, close_report_id, last_report_sequence, last_inventory_revision, inventory_sha256, inventory_protocol_major, created_at, updated_at) VALUES (?, 1, ?, 'closed', ?, 4, 1, ?, 1, ?, ?)",
        )
        .bind(AGENT_ID)
        .bind(CLOSED_BOOT)
        .bind(CLOSING_ID)
        .bind(&inventory_sha256)
        .bind(now)
        .bind(now)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_boots (agent_id, agent_epoch, boot_id, status, last_sequence, close_report_id, closed_at, created_at, updated_at) VALUES (?, 1, ?, 'closed', 4, ?, ?, ?, ?)",
        )
        .bind(AGENT_ID)
        .bind(CLOSED_BOOT)
        .bind(CLOSING_ID)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(db.pool())
        .await
        .unwrap();
        let sensitive = br#"{"report_id":"x","credential":"pp_agent_topsecretvalue"}"#.to_vec();
        sqlx::query(
            "INSERT INTO agent_report_receipts (report_id, agent_id, agent_epoch, boot_id, report_sequence, report_body_sha256, disposition, receipt_body, received_at) VALUES (?, ?, 1, ?, 4, ?, 'accepted', ?, ?)",
        )
        .bind(CLOSING_ID)
        .bind(AGENT_ID)
        .bind(CLOSED_BOOT)
        .bind(&body_sha256)
        .bind(&sensitive)
        .bind(now)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO deleted_nodes (node_id, agent_id, network_key, display_name, deleted_by_user_id, deleted_at) VALUES (?, ?, 'platon-mainnet', 'purged', NULL, ?)",
        )
        .bind(NODE_ID)
        .bind(AGENT_ID)
        .bind(now)
        .execute(db.pool())
        .await
        .unwrap();
        db.close().await;

        let server_config = platpulse_server::config::ServerConfig::resolve(
            Some(&config_path),
            &Default::default(),
        )
        .unwrap();

        let output = server_dir.path().join("coordinated");
        let summary = platpulse_server::checkpoint::create_checkpoint(
            &server_config,
            &agent_checkpoint,
            &output,
        )
        .await
        .unwrap();
        assert_eq!(summary.agent_id, AGENT_ID);
        assert_eq!(summary.closed_boot_id, CLOSED_BOOT);
        assert_eq!(summary.next_boot_id, NEXT_BOOT);
        assert_eq!(summary.inventory_revision, 1);
        assert_eq!(summary.inventory_sha256, inventory_sha256);
        assert_eq!(summary.closing_report_id, CLOSING_ID);
        assert_eq!(summary.deleted_nodes, 1);

        platpulse_server::checkpoint::verify_checkpoint(&output)
            .await
            .unwrap();

        // The original Report Receipt survives byte-for-byte, including the
        // sensitive value a sanitized backup would rewrite.
        let checkpoint_db = output.join("server").join("server.db");
        assert_eq!(
            read_receipt_body(&checkpoint_db, CLOSING_ID).await,
            sensitive
        );

        // The isolated restore reproduces the same closed-Boot checkpoint.
        let restore = server_dir.path().join("restore");
        let restored = platpulse_server::checkpoint::restore_checkpoint(&output, &restore)
            .await
            .unwrap();
        assert_eq!(restored.closing_report_id, CLOSING_ID);
        assert_eq!(restored.inventory_sha256, inventory_sha256);
        assert_eq!(
            read_receipt_body(&restore.join("server").join("server.db"), CLOSING_ID).await,
            sensitive
        );

        // A byte-level corruption is refused, and a refused restore leaves no
        // partial destination.
        let corrupted = server_dir.path().join("corrupted");
        copy_tree(&output, &corrupted);
        let corrupted_db = corrupted.join("server").join("server.db");
        let mut bytes = std::fs::read(&corrupted_db).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&corrupted_db, &bytes).unwrap();
        assert!(
            platpulse_server::checkpoint::verify_checkpoint(&corrupted)
                .await
                .is_err()
        );
        let refused = server_dir.path().join("corrupted-restore");
        assert!(
            platpulse_server::checkpoint::restore_checkpoint(&corrupted, &refused)
                .await
                .is_err()
        );
        assert!(!refused.exists());
    }
}
