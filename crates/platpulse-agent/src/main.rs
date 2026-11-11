use clap::{CommandFactory, Parser};

use platpulse_agent::cli::{
    CheckpointCommand, Cli, Command, run_agent, run_checkpoint_create, run_collect_report,
    run_enroll, run_generate_node_id, run_persist_report, run_prepare_upgrade, run_recover,
    run_shutdown, run_validate_config,
};
use platpulse_agent::runtime::RuntimeExit;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    // Diagnostics go to stderr; stdout stays for CLI results.
    platpulse_agent::logging::init(&(&cli.log).into());
    let result = run(cli).await;
    // Flush queued diagnostics before the final error is printed.
    platpulse_agent::logging::flush();
    match result {
        Ok(Some(RuntimeExit::FailClosed { .. })) => {
            // A fail-closed worker (for example a data-directory scan blocked
            // in a thread that cannot be interrupted) must not be waited on by
            // the Tokio runtime destructor. Leave the process immediately after
            // the log flush above.
            std::process::exit(1);
        }
        Ok(_) => Ok(()),
        Err(error) => Err(error),
    }
}

async fn run(cli: Cli) -> Result<Option<RuntimeExit>, Box<dyn std::error::Error>> {
    let Some(command) = cli.command else {
        let mut cmd = Cli::command();
        cmd.print_help()?;
        println!();
        return Err("no command given; see the commands above".into());
    };

    match command {
        Command::Enroll(args) => run_enroll(&args).await?,
        Command::GenerateNodeId => run_generate_node_id(),
        Command::ValidateConfig(args) => run_validate_config(&args).await?,
        Command::CollectReport(args) => run_collect_report(&args).await?,
        Command::Run(args) => return Ok(Some(run_agent(&args).await?)),
        Command::Shutdown(args) => run_shutdown(&args).await?,
        Command::PersistReport(args) => run_persist_report(&args).await?,
        Command::Recover(args) => run_recover(&args).await?,
        Command::PrepareUpgrade(args) => run_prepare_upgrade(&args).await?,
        Command::Checkpoint(CheckpointCommand::Create(args)) => {
            run_checkpoint_create(&args).await?
        }
    }
    Ok(None)
}
