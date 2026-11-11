//! `platpulse-agent` CLI (design §7.1).
use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use platpulse_core::identity::NodeId;
use thiserror::Error;

use crate::block::WebSocketBlockTransport;
use crate::config::{AgentConfig, AgentConfigFile, generate_node_id};
use crate::enroll::{EnrollError, enroll_agent_with_permit};
use crate::rpc::AlloyRpcAdapter;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "PlatPulse collector agent that monitors PlatON nodes on one Host"
)]
pub struct Cli {
    #[command(flatten)]
    pub log: crate::logging::LogArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Enroll this Agent with the Server and store its credential.
    Enroll(EnrollArgs),
    /// Generate and print a stable Node ID.
    GenerateNodeId,
    /// Validate the complete agent.toml and all Node declarations.
    ValidateConfig(ValidateConfigArgs),
    /// Collect one immutable report using independent per-Node WebSocket
    /// subscriptions and hash-only block resolution.
    CollectReport(CollectReportArgs),
    /// Run the long-lived Agent runtime. Shutdown is cancellation-safe and
    /// persists a final Closing report instead of exiting after one sample.
    Run(RunArgs),
    /// Gracefully shut down after persisting a final immutable Closing report.
    Shutdown(ShutdownArgs),
    /// Validate and persist one immutable report before delivery.
    PersistReport(PersistReportArgs),
    /// Take a consistent backup, diagnose, and optionally quarantine stale
    /// Closing reports. Refuses to run while another Agent runtime owns the
    /// Store.
    Recover(RecoverArgs),
    /// Prepare a frozen-v1 Agent for the coordinated Server-managed v2
    /// cutover: stop ordinary collection, drain the immutable backlog, complete
    /// the final Closing, and verify its original v1 declaration against the
    /// Server's last accepted revision and hash. Does not start v2 ingestion or
    /// resume collection.
    PrepareUpgrade(PrepareUpgradeArgs),
    /// Coordinated-checkpoint administration for the Server-managed v2
    /// cutover. The Agent half preserves the exact Agent Store, the original
    /// configuration, the credential, and the verified frozen-v1 declaration
    /// evidence; it never starts collection.
    #[command(subcommand)]
    Checkpoint(CheckpointCommand),
}

#[derive(Debug, Args)]
pub struct EnrollArgs {
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct ValidateConfigArgs {
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct CollectReportArgs {
    #[arg(long)]
    pub config: PathBuf,
}
#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long, default_value_t = 5000)]
    pub drain_deadline_ms: u64,
    #[arg(long, default_value_t = 5000)]
    pub sender_deadline_ms: u64,
}

#[derive(Debug, Args)]
pub struct ShutdownArgs {
    #[arg(long)]
    pub config: PathBuf,
    /// Total sender deadline after the final report is stored.
    #[arg(long, default_value_t = 5000)]
    pub deadline_ms: u64,
}
#[derive(Debug, Args)]
pub struct PersistReportArgs {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub report: PathBuf,
}

#[derive(Debug, Args)]
pub struct PrepareUpgradeArgs {
    #[arg(long)]
    pub config: PathBuf,
    /// Total sender deadline for the backlog and final Closing delivery.
    #[arg(long, default_value_t = 5000)]
    pub deadline_ms: u64,
}

#[derive(Debug, Subcommand)]
pub enum CheckpointCommand {
    /// Preserve the Agent half of the coordinated upgrade checkpoint.
    Create(CheckpointCreateArgs),
}

#[derive(Debug, Args)]
pub struct CheckpointCreateArgs {
    #[arg(long)]
    pub config: PathBuf,
    /// Empty or non-existent directory that receives the Agent checkpoint.
    /// Every artifact is written owner-only (0600) inside a 0700 directory.
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Args)]
pub struct RecoverArgs {
    #[arg(long)]
    pub config: PathBuf,
    /// Quarantine Closing reports that no longer belong to the current Agent
    /// state. Without this flag the command only backs up and diagnoses.
    #[arg(long, default_value_t = false)]
    pub drop_stale_closing: bool,
}

#[derive(Debug, Error)]
pub enum AgentCliError {
    #[error(transparent)]
    Config(#[from] Box<crate::config::AgentConfigError>),
    #[error(transparent)]
    Enroll(#[from] Box<EnrollError>),
    #[error("failed to read the enrollment token from the TTY or stdin: {0}")]
    TokenInput(std::io::Error),
    #[error("collection failed: {0}")]
    Collection(String),
    #[error("{0}")]
    Checkpoint(String),
    #[error("cannot install the {signal} handler: {source}")]
    SignalHandler {
        signal: &'static str,
        source: std::io::Error,
    },
}

impl From<crate::config::AgentConfigError> for AgentCliError {
    fn from(error: crate::config::AgentConfigError) -> Self {
        Self::Config(Box::new(error))
    }
}

impl From<EnrollError> for AgentCliError {
    fn from(error: EnrollError) -> Self {
        Self::Enroll(Box::new(error))
    }
}

pub fn run_generate_node_id() {
    println!("{}", generate_node_id());
}

/// Refuse to start collection after an offline v1 -> v2 conversion when
/// agent.toml still declares a local `inventory_revision`.
///
/// The check runs before boot recovery and before any business write, so a
/// leftover field produces the actionable "the Server assigns it now, remove
/// it" message instead of a generic declaration conflict. It is strictly
/// read-only: it never creates or migrates the Agent Store.
async fn guard_migration_configuration(
    state_db: &std::path::Path,
    validated: &crate::config::ValidatedAgentConfig,
) -> Result<(), AgentCliError> {
    let config_revision =
        (!validated.server_managed_inventory).then_some(validated.inventory.revision);
    crate::inventory_declaration::check_migration_config_read_only(state_db, config_revision)
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))
}

pub async fn run_validate_config(args: &ValidateConfigArgs) -> Result<(), Box<AgentCliError>> {
    let file = AgentConfigFile::load(&args.config).map_err(|e| Box::new(AgentCliError::from(e)))?;
    let validated = file
        .validate()
        .map_err(|e| Box::new(AgentCliError::from(e)))?;
    // A converted (v2) Store must not be started with a leftover v1 revision.
    guard_migration_configuration(&file.state_db, &validated).await?;
    // The Agent Store holds the only local record of the Inventory the Server
    // effectively accepted, and `validate-config` is where an operator checks a
    // configuration edit before restarting. The check is read-only: this command
    // must not create or migrate the Agent Store (issue #181).
    if validated.server_managed_inventory {
        println!(
            "Validated {} Node(s), Server-managed inventory revision.",
            validated.inventory.nodes.len()
        );
    } else {
        // A v1 configuration still compares against the last accepted Inventory.
        crate::inventory_declaration::check_declaration_read_only(
            &file.state_db,
            &validated.inventory,
        )
        .await
        .map_err(|conflict| {
            Box::new(AgentCliError::from(
                crate::config::AgentConfigError::InventoryDeclaration(conflict.to_string()),
            ))
        })?;
        println!(
            "Validated {} Node(s), inventory revision {}.",
            validated.inventory.nodes.len(),
            validated.inventory.revision
        );
    }
    Ok(())
}

pub async fn run_recover(args: &RecoverArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let outcome = crate::recover::recover_store(&config, args.drop_stale_closing)
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!(
        "Consistent backup written to {}",
        outcome.backup_path.display()
    );
    println!("Stale Closing reports: {}", outcome.stale.len());
    for report in &outcome.stale {
        println!(
            "  {} (agent_epoch={}, boot_id={})",
            report.report_id, report.agent_epoch, report.boot_id
        );
    }
    if args.drop_stale_closing {
        println!("Quarantined {} stale Closing report(s).", outcome.dropped);
    } else if !outcome.stale.is_empty() {
        println!("Re-run with --drop-stale-closing to quarantine these reports.");
    }
    Ok(())
}

pub async fn run_prepare_upgrade(args: &PrepareUpgradeArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let outcome = crate::preparation::prepare_v1_upgrade(
        &config,
        std::time::Duration::from_millis(args.deadline_ms),
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!("Upgrade preparation verified.");
    println!("  Closing report: {}", outcome.closing_report_id);
    println!(
        "  Frozen v1 Inventory: revision {} sha256 {}",
        outcome.inventory_revision, outcome.inventory_sha256
    );
    println!(
        "  Closing receipt: {} (Inventory accepted)",
        outcome.closing_receipt_disposition
    );
    println!("  Closed Boot: {}", outcome.closed_boot_id);
    println!(
        "  Next Boot: {} (pending {})",
        outcome.next_boot_id, outcome.pending_transition
    );
    println!("  Verified at: {}", outcome.verified_at);
    println!(
        "Normal collection stays stopped: do not start 'run' until the coordinated cutover is authorized."
    );
    Ok(())
}

/// Preserve the Agent half of the coordinated upgrade checkpoint. This is a
/// read-only operator command over the stopped Agent Store: it never writes to
/// the live Store and never resumes collection.
pub async fn run_checkpoint_create(args: &CheckpointCreateArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let outcome = crate::checkpoint::create_agent_checkpoint(&config, &args.output)
        .await
        .map_err(|error| AgentCliError::Checkpoint(error.to_string()))?;
    println!("Agent upgrade checkpoint written.");
    println!("  Directory: {}", outcome.directory.display());
    println!("  Manifest: {}", outcome.manifest_path.display());
    println!("  Agent: {}", outcome.agent_id);
    println!("  Closed Boot: {}", outcome.closed_boot_id);
    println!("  Pending Boot: {}", outcome.next_boot_id);
    println!(
        "  Frozen v1 Inventory: revision {} sha256 {}",
        outcome.inventory_revision, outcome.inventory_sha256
    );
    println!("  Preserved artifacts: {}", outcome.artifacts);
    println!(
        "The Server half must be created from this directory with 'platpulse-server checkpoint create --agent-checkpoint <directory>'."
    );
    Ok(())
}

pub async fn run_collect_report(args: &CollectReportArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let adapter = AlloyRpcAdapter;
    let write_permit = crate::database::AgentStoreWritePermit::new();
    let validated = config
        .validated_inventory()
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    // A converted Store must not collect with a leftover v1 revision.
    guard_migration_configuration(&config.state_db, &validated).await?;
    // Same startup refusal as `run`, and for the same reason: this command
    // would otherwise recover the previous boot with a Closing report the
    // Server refuses (issue #181).
    if !validated.server_managed_inventory {
        crate::collector::guard_startup_inventory_declaration(
            &config,
            &validated.inventory,
            write_permit.clone(),
        )
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    }
    crate::collector::recover_previous_boot_with_permit(&config, &adapter, write_permit.clone())
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let mut subscriptions = validated
        .inventory
        .nodes
        .iter()
        .map(|node| crate::block::HeadSubscription::new(node.node_id, 32))
        .collect::<Vec<_>>();
    let digest = crate::collector::collect_and_persist_with_blocks_with_permit(
        &config,
        &adapter,
        &WebSocketBlockTransport::default(),
        &mut subscriptions,
        write_permit,
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!("Persisted immutable collected report (sha256 {digest}).");
    Ok(())
}

pub async fn run_agent(args: &RunArgs) -> Result<crate::runtime::RuntimeExit, AgentCliError> {
    // The stop signals are taken over before anything else, so a SIGTERM or
    // SIGINT that arrives while the Agent is still starting is held and
    // answered by the same graceful shutdown once startup finishes, instead of
    // falling through to the default disposition that kills the process with
    // no Closing report and drops the queued log lines.
    let mut shutdown_signals = crate::runtime::ShutdownSignals::install()?;
    tracing::info!("{} starting", crate::startup_version_line());
    let config = AgentConfig::resolve(&args.config)?;
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let write_permit = crate::database::AgentStoreWritePermit::new();
    let validated = config
        .validated_inventory()
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    // A converted Store must not start with a leftover v1 revision: report the
    // migration residue before boot recovery writes anything.
    guard_migration_configuration(&config.state_db, &validated).await?;
    // Refuse to start on an Inventory the Server would refuse, *before* boot
    // recovery (issue #181): recovering first would emit a Closing report for
    // the previous boot that is doomed to the same refusal, leaving the
    // operator with a frozen projection instead of an actionable message.
    // The signature reads the same for a startup refusal and a runtime drift;
    // only the duration differs — the Agent refuses to start here, and the
    // collection loop refuses to declare while staying alive.
    if !validated.server_managed_inventory {
        crate::collector::guard_startup_inventory_declaration(
            &config,
            &validated.inventory,
            write_permit.clone(),
        )
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    }
    let adapter = AlloyRpcAdapter;
    crate::collector::recover_previous_boot_with_permit(&config, &adapter, write_permit.clone())
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let node_ids = validated
        .inventory
        .nodes
        .iter()
        .map(|node| node.node_id)
        .collect::<Vec<_>>();
    // Poisoned rows are dropped before any worker starts so the failure cannot
    // recur on every collection tick (issue #243). Only the Nodes this Agent
    // declares are swept: a History Gap for any other Node is refused by the
    // contract, so those rows wait until the Node is declared again.
    purge_out_of_contract_block_summaries(&config, &node_ids, write_permit.clone()).await?;
    let report_transport: std::sync::Arc<dyn crate::reporting::ReportTransport> =
        std::sync::Arc::new(
            crate::reporting::HttpReportTransport::from_config(&config)
                .map_err(|error| AgentCliError::Collection(error.to_string()))?,
        );
    crate::runtime::run(
        &crate::runtime::ProductionWorkerFactory,
        shutdown_signals.recv(),
        crate::runtime::RuntimeInput {
            config,
            validated,
            node_ids,
            write_permit,
            drain_deadline: std::time::Duration::from_millis(args.drain_deadline_ms),
            sender_deadline: std::time::Duration::from_millis(args.sender_deadline_ms),
        },
        report_transport,
    )
    .await
}

/// Remove Block Summaries whose chain timestamp is outside the AgentReport v1
/// contract before any worker starts (issue #243). A Store poisoned while the
/// Node answered with native millisecond timestamps would otherwise fail every
/// collection forever, which a supervising systemd unit turns into a restart
/// storm. This is the same repair an operator would run by hand against
/// agent.db, plus the History Gap declaration the dropped heights need: the
/// samples are discarded deliberately, so the loss is stated to the Server
/// instead of leaving a hole that reads as continuous history.
async fn purge_out_of_contract_block_summaries(
    config: &AgentConfig,
    monitored_nodes: &[NodeId],
    write_permit: crate::database::AgentStoreWritePermit,
) -> Result<(), AgentCliError> {
    let mut store = crate::database::AgentStore::open_with_write_permit(
        crate::database::AgentDatabaseConfig::new(&config.state_db),
        write_permit,
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let purge = crate::block::purge_out_of_contract_block_summaries(&mut store, monitored_nodes)
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    if !purge.is_empty() {
        let per_node = purge
            .purged_by_node
            .iter()
            .map(|(node_id, count)| format!("{node_id}: {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        tracing::warn!(
            "Purged {} Block Summary sample(s) outside the AgentReport v1 contract ({}), declared as {} History Gap range(s). Check that every Node runs with --http.ethcompatible.",
            purge.purged,
            per_node,
            purge.declared_gaps
        );
    }
    store
        .close()
        .await
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    Ok(())
}

pub async fn run_shutdown(args: &ShutdownArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let validated = config
        .validated_inventory()
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    guard_migration_configuration(&config.state_db, &validated).await?;
    let sender_deadline = std::time::Duration::from_millis(args.deadline_ms);
    let sender = crate::reporting::HttpReportTransport::from_config(&config)
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let deadline = crate::shutdown::ShutdownDeadline::start(sender_deadline);
    let outcome = crate::shutdown::graceful_shutdown_with_permit(
        &config,
        &mut [],
        deadline,
        sender_deadline,
        crate::database::AgentStoreWritePermit::new(),
        &sender,
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!(
        "Stored final shutdown report {} (sequence {}, state {}).",
        outcome.report_id, outcome.report_sequence, outcome.shutdown_state
    );
    Ok(())
}

pub async fn run_persist_report(args: &PersistReportArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let validated = config
        .validated_inventory()
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    guard_migration_configuration(&config.state_db, &validated).await?;
    let digest = crate::reporting::persist_report_from_config_with_permit(
        &config,
        &args.report,
        crate::database::AgentStoreWritePermit::new(),
    )
    .await
    .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    println!("Persisted immutable report (sha256 {digest}).");
    Ok(())
}

pub async fn run_enroll(args: &EnrollArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let _runtime_lock = crate::database::AgentRuntimeLock::acquire(&config.state_db)
        .map_err(|error| AgentCliError::Collection(error.to_string()))?;
    let token = read_enrollment_token().map_err(AgentCliError::TokenInput)?;
    if token.is_empty() {
        return Err(AgentCliError::Enroll(Box::new(
            EnrollError::ServerRejected("an enrollment token is required".to_owned()),
        )));
    }
    let enrolled = enroll_agent_with_permit(
        &config,
        &token,
        crate::database::AgentStoreWritePermit::new(),
    )
    .await?;
    println!(
        "Enrolled agent {} (epoch {}).",
        enrolled.agent_id, enrolled.agent_epoch
    );
    println!(
        "Credential stored at {}.",
        enrolled.credential_path.display()
    );
    Ok(())
}

fn read_enrollment_token() -> Result<String, std::io::Error> {
    if std::io::stdin().is_terminal() {
        rpassword::prompt_password("Enrollment token: ")
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_token_keeps_spaces_and_strips_only_line_endings() {
        assert_eq!("a b c".trim_end_matches(['\r', '\n']), "a b c");
        assert_eq!(
            "pp_enroll_x_abc\r\n".trim_end_matches(['\r', '\n']),
            "pp_enroll_x_abc"
        );
    }

    #[test]
    fn the_validate_config_conflict_keeps_its_operator_remedy() {
        // `main` prints errors with `{:?}`, so the remedy must survive the
        // Debug form of the CLI error: a structured variant would print the
        // two hashes and hide the sentence telling the operator what to do.
        let conflict = crate::inventory_declaration::InventoryDeclarationConflict::ContentChanged {
            revision: 4,
            recorded_sha256: "0xaa".to_owned(),
            declared_sha256: "0xbb".to_owned(),
        };
        let error = AgentCliError::from(crate::config::AgentConfigError::InventoryDeclaration(
            conflict.to_string(),
        ));
        let printed = format!("{error:?}");
        assert!(printed.contains("bump inventory_revision"), "{printed}");
    }
}
