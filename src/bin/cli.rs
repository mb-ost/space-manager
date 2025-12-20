use anyhow::{Context, Result};
use browser_spaces::ipc::IpcClient;
use browser_spaces::types::{Command, Response};
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    let command = match args[1].as_str() {
        "next" => Command::Next,
        "prev" => Command::Prev,
        "spawn" => {
            if args.len() < 3 {
                eprintln!("Error: spawn requires a command argument");
                eprintln!("Usage: spacectl spawn \"<command>\"");
                std::process::exit(1);
            }
            Command::Spawn(args[2..].join(" "))
        }
        "list" => Command::List,
        "cleanup" => Command::Cleanup,
        _ => {
            eprintln!("Error: unknown command '{}'", args[1]);
            print_usage();
            std::process::exit(1);
        }
    };

    let mut client = IpcClient::connect()
        .await
        .context("Failed to connect to daemon")?;

    client
        .send_command(&command)
        .await
        .context("Failed to send command")?;

    let response = client
        .recv_response()
        .await
        .context("Failed to receive response")?;

    match response {
        Response::Ok => {
            // Silent success
        }
        Response::Error(msg) => {
            eprintln!("Error: {}", msg);
            std::process::exit(1);
        }
        Response::Windows(windows) => {
            if windows.is_empty() {
                println!("No managed windows");
            } else {
                println!("Managed windows:");
                for (i, window) in windows.iter().enumerate() {
                    println!(
                        "  [{}] {} - {} ({})",
                        i + 1,
                        window.class,
                        window.title,
                        window.address
                    );
                }
            }
        }
    }

    Ok(())
}

fn print_usage() {
    eprintln!("Usage: spacectl <command> [args]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  next              Switch to next space");
    eprintln!("  prev              Switch to previous space");
    eprintln!("  spawn \"<cmd>\"     Spawn a new application instance");
    eprintln!("  list              List all managed windows");
    eprintln!("  cleanup           Close all hidden windows in special:spaces");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  spacectl next");
    eprintln!("  spacectl spawn \"brave --user-data-dir=~/.config/brave-work\"");
    eprintln!("  spacectl cleanup");
}
