use clap::{CommandFactory, Parser};

use platpulse_server::cli::{
    Cli, Command, OwnerCommand, resolve_serve_config, run_owner_create, run_serve,
};
use platpulse_server::config::ServerConfig;
use platpulse_server::init::run_init;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        Command::Serve(args) => {
            let config = resolve_serve_config(&args)?;
            run_serve(&config).await?;
        }
    }
    Ok(())
}
