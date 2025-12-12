// Khai báo các module con của project.
// `file_transfer`: Xử lý logic truyền file.
// `libp2p_peers`: Quản lý các kết nối mạng P2P bằng libp2p.
// `room`: Quản lý phòng chat và các chủ đề (topic) gossipsub.
mod file_transfer;
mod libp2p_peers;
mod room;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use libp2p::PeerId;
use std::io::Write;
use tokio::sync::mpsc;

/// Cấu trúc `Cli` sử dụng `clap` để phân tích các đối số dòng lệnh.
#[derive(Parser, Debug)]
#[command(name = "FileMesh-RS")]
#[command(about = "Truyền file P2P sử dụng libp2p", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Enum `Commands` định nghĩa các lệnh con mà ứng dụng hỗ trợ.
#[derive(Subcommand, Debug)]
enum Commands {
    /// Bắt đầu một peer và tham gia vào một phòng.
    Start {
        /// Tên của peer
        #[arg(short, long)]
        name: String,

        /// Tên phòng để tham gia
        #[arg(short, long)]
        room: String,
    },
}

/// Enum đại diện cho các lệnh mà người dùng có thể nhập.
#[derive(Debug)]
pub enum UserCommand {
    ListPeers,
    SendFile {
        file_path: String,
        peer_id: Option<PeerId>,
        broadcast: bool,
    },
    Dial(String), // Lệnh mới để quay số thủ công
    Quit,
}

/// Hàm `main` là điểm khởi đầu của chương trình.
/// Nó sử dụng `tokio::main` để chạy trong một môi trường bất đồng bộ.
#[tokio::main]
async fn main() -> Result<()> {
    // Phân tích các đối số dòng lệnh.
    let cli = Cli::parse();

    // Xử lý lệnh con được cung cấp.
    match cli.command {
        Commands::Start { name, room } => {
            start_peer(name, room).await?;
        }
    }

    Ok(())
}

/// Hàm `start_peer` khởi tạo và chạy một node P2P.
async fn start_peer(name: String, room_name: String) -> Result<()> {
    // In banner chào mừng.
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

    // Tạo một kênh (channel) để giao tiếp giữa luồng xử lý input của người dùng và luồng mạng.
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

    // Spawn một task mới để chạy logic của peer.
    let peer_handle = tokio::spawn(async move {
        if let Err(e) = libp2p_peers::run_peer(name, room_name, cmd_rx).await {
            eprintln!("{} {}", "Lỗi khi chạy peer:".red(), e);
        }
    });

    // Spawn một task khác để xử lý input từ người dùng.
    tokio::spawn(async move {
        handle_user_input(cmd_tx).await;
    });

    // Chờ cho đến khi task của peer kết thúc.
    peer_handle.await?;

    Ok(())
}

/// Hàm `handle_user_input` đọc và xử lý các lệnh từ người dùng.
async fn handle_user_input(cmd_tx: mpsc::UnboundedSender<UserCommand>) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use std::io;

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    // Hiển thị hướng dẫn sử dụng ngắn gọn khi bắt đầu.
    print_help();

    // Hàm trợ giúp để hiển thị dấu nhắc lệnh.
    fn show_prompt() {
        print!("{} ", ">".bright_cyan());
        let _ = io::stdout().flush();
    }

    show_prompt();

    // Vòng lặp để đọc từng dòng input từ người dùng.
    while let Ok(Some(line)) = lines.next_line().await {
        let input = line.trim();
        if input.is_empty() {
            show_prompt();
            continue;
        }

        // Tách input thành các phần dựa trên khoảng trắng.
        let parts: Vec<&str> = input.split_whitespace().collect();

        // Xử lý lệnh đầu tiên.
        let command_sent = match parts.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("peers") => cmd_tx.send(UserCommand::ListPeers).is_ok(),
            Some("send") => {
                if let Some(cmd) = parse_send_command(&parts[1..]) {
                    cmd_tx.send(cmd).is_ok()
                } else {
                    true // Don't break loop if parsing fails
                }
            }
            Some("dial") => {
                if let Some(addr) = parts.get(1) {
                    cmd_tx.send(UserCommand::Dial(addr.to_string())).is_ok()
                } else {
                    println!("{}", "Lệnh 'dial' yêu cầu một địa chỉ Multiaddr.".red());
                    true // Don't break loop
                }
            }
            Some("quit") | Some("exit") => {
                let _ = cmd_tx.send(UserCommand::Quit);
                false // Break loop
            }
            Some("help") => {
                print_help();
                true // Don't break loop
            }
            _ => {
                println!(
                    "{} Sử dụng 'help' để xem các lệnh có sẵn",
                    "Lệnh không xác định.".red()
                );
                true // Don't break loop
            }
        };

        if !command_sent {
            break;
        }

        show_prompt();
    }
}

/// Hàm `parse_send_command` phân tích các đối số của lệnh `send`.
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
                    println!("{}", "Thiếu đường dẫn file sau --file".red());
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
                            println!("{}", "Định dạng Peer ID không hợp lệ".red());
                            return None;
                        }
                    }
                } else {
                    println!("{}", "Thiếu Peer ID sau --to".red());
                    return None;
                }
            }
            "--broadcast" => {
                broadcast = true;
                i += 1;
            }
            _ => {
                println!("{} {}", "Đối số không xác định:".red(), args[i]);
                return None;
            }
        }
    }

    // Kiểm tra các đối số bắt buộc.
    if file_path.is_none() {
        println!("{}", "Thiếu đối số --file bắt buộc".red());
        return None;
    }

    if !broadcast && peer_id.is_none() {
        println!(
            "{}",
            "Phải chỉ định --to <PEER_ID> hoặc --broadcast".red()
        );
        return None;
    }

    // Trả về lệnh `SendFile` đã được phân tích.
    Some(UserCommand::SendFile {
        file_path: file_path.unwrap(),
        peer_id,
        broadcast,
    })
}

/// Hàm `print_help` in ra thông tin hướng dẫn sử dụng các lệnh.
fn print_help() {
    println!();
    println!("{}", "Các lệnh có sẵn:".bright_yellow());
    println!();
    println!("  {}", "peers".bright_green());
    println!("    Liệt kê tất cả các peer đã kết nối và loại kết nối.");
    println!();
    println!("  {}", "send --file <FILE> --to <PEER_ID>".bright_green());
    println!("    Gửi một file đến một peer cụ thể.");
    println!("    Ví dụ: send --file document.pdf --to 12D3KooW...");
    println!();
    println!("  {}", "send --file <FILE> --broadcast".bright_green());
    println!("    Gửi một file đến tất cả các peer trong phòng.");
    println!("    Ví dụ: send --file photo.jpg --broadcast");
    println!();
    println!("  {}", "quit / exit".bright_green());
    println!("    Thoát ứng dụng.");
    println!();
}
