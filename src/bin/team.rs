use clap::{Parser, Subcommand};
use rive::output::Envelope;
use serde::Serialize;

#[derive(Parser)]
#[command(name = "team")]
#[command(version)]
#[command(about = "Rive agent-facing CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    SelfCheck,
}

#[derive(Debug, Serialize)]
struct SelfCheckProtocol {
    ok: bool,
    missing_env: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct SelfCheckDisplay {
    summary: String,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::SelfCheck => {
            let required = ["RIVE_WORKSPACE", "RIVE_AGENT_ID", "RIVE_AGENT_TOKEN"];
            let missing_env: Vec<&'static str> = required
                .iter()
                .copied()
                .filter(|key| {
                    std::env::var(key)
                        .ok()
                        .filter(|value| !value.is_empty())
                        .is_none()
                })
                .collect();
            let ok = missing_env.is_empty();
            let envelope = Envelope::new(
                SelfCheckProtocol { ok, missing_env },
                SelfCheckDisplay {
                    summary: if ok {
                        "team environment is ready".to_string()
                    } else {
                        "team environment is missing required variables".to_string()
                    },
                },
            );
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            if !ok {
                std::process::exit(1);
            }
        }
    }
}
