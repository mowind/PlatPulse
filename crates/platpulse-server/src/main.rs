use clap::{CommandFactory, Parser};

use platpulse_server::cli::{
    AgentCommand, CheckpointCommand, Cli, Command, CutoverCommand, NetworkCommand, OwnerCommand,
    ViewerCommand, resolve_serve_config, run_backup, run_checkpoint_convert, run_checkpoint_create,
    run_checkpoint_restore, run_checkpoint_verify, run_checkpoint_verify_conversion,
    run_create_enrollment_token, run_cutover_resume, run_cutover_rollback, run_cutover_status,
    run_network_create, run_owner_create, run_restore, run_serve, run_verify_integrity,
    run_viewer_create,
};
use platpulse_server::config::ServerConfig;
use platpulse_server::init::run_init;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // Returning the error from `main` would print its `Debug`, hiding
            // the typed stopped-Server guidance behind variant names. The
            // CLI's contract is the human-readable `Display` form.
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if cli.print_openapi {
        println!("{}", platpulse_server::openapi::spec_json());
        return Ok(());
    }

    let Some(command) = cli.command else {
        let mut cmd = Cli::command();
        cmd.print_help()?;
        println!();
        return Err("no command given; see the commands above".into());
    };

    match command {
        Command::Init(args) => {
            let config = ServerConfig::resolve_init(&args.config)?;
            let report = run_init(&config).await?;
            println!("PlatPulse Server state initialized.");
            for warning in &report.warnings {
                println!("warning: {warning}");
            }
            println!("Next steps:");
            println!(
                "  1. platpulse-server owner create --config {} --username <name>",
                config
                    .config_path
                    .as_deref()
                    .unwrap_or(&args.config)
                    .display()
            );
            println!(
                "  2. platpulse-server serve --config {}",
                config
                    .config_path
                    .as_deref()
                    .unwrap_or(&args.config)
                    .display()
            );
        }
        Command::Owner(OwnerCommand::Create(args)) => {
            let config = ServerConfig::resolve(Some(args.config.as_path()), &Default::default())?;
            run_owner_create(&config, &args.username).await?;
        }
        Command::Viewer(ViewerCommand::Create(args)) => {
            let config = ServerConfig::resolve(Some(args.config.as_path()), &Default::default())?;
            run_viewer_create(&config, &args.username).await?;
        }
        Command::Network(NetworkCommand::Create(args)) => {
            let config = ServerConfig::resolve(Some(args.config.as_path()), &Default::default())?;
            run_network_create(&config, &args).await?;
        }
        Command::Agent(AgentCommand::CreateEnrollmentToken(args)) => {
            let config = ServerConfig::resolve(Some(args.config.as_path()), &Default::default())?;
            run_create_enrollment_token(&config, &args).await?;
        }
        Command::Backup(args) => {
            let config = ServerConfig::resolve(Some(args.config.as_path()), &Default::default())?;
            let filename = run_backup(&config).await?;
            println!("Created sanitized backup '{filename}'.");
        }
        Command::VerifyIntegrity(args) => {
            let config = ServerConfig::resolve(Some(args.config.as_path()), &Default::default())?;
            run_verify_integrity(&config).await?;
        }
        Command::Restore(args) => {
            let config = ServerConfig::resolve(Some(args.config.as_path()), &Default::default())?;
            run_restore(&config, &args).await?;
        }
        Command::Checkpoint(CheckpointCommand::Create(args)) => {
            let config = ServerConfig::resolve(Some(args.config.as_path()), &Default::default())?;
            run_checkpoint_create(&config, &args).await?;
        }
        Command::Checkpoint(CheckpointCommand::Verify(args)) => {
            run_checkpoint_verify(&args).await?;
        }
        Command::Checkpoint(CheckpointCommand::Restore(args)) => {
            run_checkpoint_restore(&args).await?;
        }
        Command::Checkpoint(CheckpointCommand::Convert(args)) => {
            run_checkpoint_convert(&args).await?;
        }
        Command::Checkpoint(CheckpointCommand::VerifyConversion(args)) => {
            run_checkpoint_verify_conversion(&args).await?;
        }
        Command::Cutover(CutoverCommand::Status(args)) => {
            run_cutover_status(&args).await?;
        }
        Command::Cutover(CutoverCommand::Resume(args)) => {
            run_cutover_resume(&args).await?;
        }
        Command::Cutover(CutoverCommand::Rollback(args)) => {
            run_cutover_rollback(&args).await?;
        }
        Command::Serve(args) => {
            let config = resolve_serve_config(&args)?;
            run_serve(&config).await?;
        }
    }
    Ok(())
}
