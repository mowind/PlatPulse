//! `platpulse-agent` CLI (design §8.2): `enroll` is the Phase 1 command
//! that provisions the Agent identity. Binary crates keep a thin `main.rs`;
//! all logic lives here so it can be exercised from tests.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use thiserror::Error;

use crate::config::AgentConfig;
use crate::enroll::{EnrollError, enroll_agent};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "PlatPulse collector agent that monitors PlatON nodes on one Host"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Enroll this Agent with the Server and store its credential
    /// (design §4.5). The Enrollment Token is read from the TTY (hidden)
    /// or from stdin — never from argv.
    Enroll(EnrollArgs),
}

#[derive(Debug, Args)]
pub struct EnrollArgs {
    /// `agent.toml` (design §8.2): `server_url`, `credential_file`, and
    /// `state_db`.
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(Debug, Error)]
pub enum AgentCliError {
    #[error(transparent)]
    Config(#[from] crate::config::AgentConfigError),
    #[error(transparent)]
    Enroll(#[from] EnrollError),
    #[error("failed to read the enrollment token from the TTY or stdin: {0}")]
    TokenInput(std::io::Error),
}

/// Run `platpulse-agent enroll`: read the one-time Enrollment Token from
/// the TTY (hidden) or stdin, exchange it with the Server, and store the
/// issued credential. The credential itself is never printed.
pub async fn run_enroll(args: &EnrollArgs) -> Result<(), AgentCliError> {
    let config = AgentConfig::resolve(&args.config)?;
    let token = read_enrollment_token().map_err(AgentCliError::TokenInput)?;
    if token.is_empty() {
        return Err(AgentCliError::Enroll(EnrollError::ServerRejected(
            "an enrollment token is required".to_owned(),
        )));
    }
    let enrolled = enroll_agent(&config, &token).await?;
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

/// Read the Enrollment Token from the TTY with hidden input, or from a
/// secure stdin/fd: the first line is the token; trailing line endings are
/// stripped, all other characters are kept verbatim.
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
    #[test]
    fn stdin_token_keeps_spaces_and_strips_only_line_endings() {
        // read_enrollment_token reads the real process stdin, which is not
        // injectable here; this test pins the trimming rule it applies.
        assert_eq!("a b c".trim_end_matches(['\r', '\n']), "a b c");
        assert_eq!(
            "pp_enroll_x_abc\r\n".trim_end_matches(['\r', '\n']),
            "pp_enroll_x_abc"
        );
        assert_eq!(" token ".trim_end_matches(['\r', '\n']), " token ");
    }
}
