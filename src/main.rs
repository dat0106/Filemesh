mod file_transfer;
mod libp2p_peers;
mod room;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use libp2p::PeerId;
use std::io::Write;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(name = "FileMesh-RS")]
#[command(about = "Peer-to-peer file transfer using libp2p", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start a peer and join a room
    Start {
        /// Peer name
        #[arg(short, long)]
        name: String,

        /// Room name to join
        #[arg(short, long)]
        room: String,
    },
}

#[derive(Debug, Clone)]
pub enum UserCommand {
    ListPeers,
    SendFile {
        file_path: String,
        peer_id: Option<PeerId>,
        broadcast: bool,
    },
    Quit,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { name, room } => {
            start_peer(name, room).await?;
        }
    }

    Ok(())
}

async fn start_peer(name: String, room_name: String) -> Result<()> {
    println!(
        "{}",
        "╔══════════════════════════════════════════════════╗".bright_cyan()
    );
    println!(
        "{}",
        "║           FileMesh-RS - P2P File Transfer       ║".bright_cyan()
    );
    println!(
        "{}",
        "╚══════════════════════════════════════════════════╝".bright_cyan()
    );
    println!();

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

    let peer_handle = tokio::spawn(async move {
        if let Err(e) = libp2p_peers::run_peer(name, room_name, cmd_rx).await {
            eprintln!("{} {}", "Error running peer:".red(), e);
        }
    });

    tokio::spawn(async move {
        handle_user_input(cmd_tx).await;
    });

    peer_handle.await?;

    Ok(())
}

async fn handle_user_input(cmd_tx: mpsc::UnboundedSender<UserCommand>) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    // Show quick usage once
    print_help();

    // prompt helper
    fn show_prompt() {
        print!("{} ", ">".bright_cyan());
        let _ = std::io::stdout().flush();
    }

    show_prompt();

    while let Ok(Some(line)) = lines.next_line().await {
        let input = line.trim();
        if input.is_empty() {
            show_prompt();
            continue;
        }

        // split by whitespace, preserve existing behavior
        let parts: Vec<&str> = input.split_whitespace().collect();

        match parts.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("peers") => {
                let _ = cmd_tx.send(UserCommand::ListPeers);
            }
            Some("send") => {
                if let Some(cmd) = parse_send_command(&parts[1..]) {
                    let _ = cmd_tx.send(cmd);
                }
            }
            Some("quit") | Some("exit") => {
                let _ = cmd_tx.send(UserCommand::Quit);
                break;
            }
            Some("help") => {
                print_help();
            }
            _ => {
                println!(
                    "{} Use 'help' to see available commands",
                    "Unknown command.".red()
                );
                show_prompt();
            }
        }

        show_prompt();
    }
}

fn parse_send_command(args: &[&str]) -> Option<UserCommand> {
    let mut file_path: Option<String> = None;
    let mut peer_id: Option<PeerId> = None;
    let mut broadcast = false;

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--file" => {
                if i + 1 < args.len() {
                    file_path = Some(args[i + 1].to_string());
                    i += 2;
                } else {
                    println!("{}", "Missing file path after --file".red());
                    return None;
                }
            }
            "--to" => {
                if i + 1 < args.len() {
                    match args[i + 1].parse::<PeerId>() {
                        Ok(id) => {
                            peer_id = Some(id);
                            i += 2;
                        }
                        Err(_) => {
                            println!("{}", "Invalid peer ID format".red());
                            return None;
                        }
                    }
                } else {
                    println!("{}", "Missing peer ID after --to".red());
                    return None;
                }
            }
            "--broadcast" => {
                broadcast = true;
                i += 1;
            }
            _ => {
                println!("{} {}", "Unknown argument:".red(), args[i]);
                return None;
            }
        }
    }

    if file_path.is_none() {
        println!("{}", "Missing required --file argument".red());
        return None;
    }

    if !broadcast && peer_id.is_none() {
        println!(
            "{}",
            "Must specify either --to <PEER_ID> or --broadcast".red()
        );
        return None;
    }

    Some(UserCommand::SendFile {
        file_path: file_path.unwrap(),
        peer_id,
        broadcast,
    })
}

fn print_help() {
    println!();
    println!("{}", "Available commands:".bright_yellow());
    println!();
    println!("  {}", "peers".bright_green());
    println!("    List all connected peers with connection types");
    println!();
    println!("  {}", "send --file <FILE> --to <PEER_ID>".bright_green());
    println!("    Send a file to a specific peer");
    println!("    Example: send --file document.pdf --to 12D3KooW...");
    println!();
    println!("  {}", "send --file <FILE> --broadcast".bright_green());
    println!("    Broadcast a file to all peers in the room");
    println!("    Example: send --file photo.jpg --broadcast");
    println!();
    println!("  {}", "quit / exit".bright_green());
    println!("    Exit the application");
    println!();
}
