use clap::{CommandFactory, Parser};

use platpulse_agent::cli::{Cli, Command, run_enroll};

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
    }
    Ok(())
}
