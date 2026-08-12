use clap::{CommandFactory, Parser};

use platpulse_agent::cli::{
    Cli, Command, run_agent, run_collect_report, run_enroll, run_generate_node_id,
    run_persist_report, run_shutdown, run_validate_config,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        let mut cmd = Cli::command();
        cmd.print_help()?;
        println!();
        return Err("no command given; see the commands above".into());
    };

    match command {
        Command::Enroll(args) => run_enroll(&args).await?,
        Command::GenerateNodeId => run_generate_node_id(),
        Command::ValidateConfig(args) => run_validate_config(&args)?,
        Command::CollectReport(args) => run_collect_report(&args).await?,
        Command::Run(args) => run_agent(&args).await?,
        Command::Shutdown(args) => run_shutdown(&args).await?,
        Command::PersistReport(args) => run_persist_report(&args).await?,
    }
    Ok(())
}
