use anyhow::{Context, Result};
use space_manager::ipc::IpcClient;
use space_manager::types::{Command, Response};
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
        "shutdown" => Command::Shutdown,
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

    // The daemon exits while handling Shutdown, so a missing response is expected there.
    let response = match client.recv_response().await {
        Ok(r) => r,
        Err(_) if matches!(command, Command::Shutdown) => Response::Ok,
        Err(e) => return Err(e).context("Failed to receive response"),
    };

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
        Response::Templates(templates) => {
            if templates.is_empty() {
                println!("No command templates");
            } else {
                println!("Command templates:");
                for template in templates.iter() {
                    println!(
                        "  - {}: {}",
                        template["name"].as_str().unwrap_or("?"),
                        template["command"].as_str().unwrap_or("?")
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
    eprintln!("  shutdown          Gracefully stop the daemon (saves state, closes tracked windows)");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  spacectl next");
    eprintln!("  spacectl spawn \"brave --user-data-dir=~/.config/brave-work\"");
    eprintln!("  spacectl cleanup");
}
