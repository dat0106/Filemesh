// Import các thư viện và module cần thiết.
use anyhow::{Context, Result};
use colored::Colorize;
use futures::{AsyncReadExt, AsyncWriteExt, FutureExt};
use libp2p::{
    core::{upgrade::ReadyUpgrade, Multiaddr},
    swarm::{
        ConnectionHandler, ConnectionHandlerEvent, NetworkBehaviour, SubstreamProtocol,
        THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    PeerId, Stream, StreamProtocol,
};
use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
    task::{Context as TaskContext, Poll},
};
use tokio::io::{AsyncReadExt as TokioReadExt, AsyncWriteExt as TokioWriteExt};

// Định nghĩa một giao thức (protocol) duy nhất cho việc truyền file.
// Các node sẽ sử dụng protocol này để "nói chuyện" với nhau về việc truyền file.
const PROTOCOL: StreamProtocol = StreamProtocol::new("/filemesh/1.0.0");
// Kích thước của mỗi đoạn (chunk) file khi truyền.
const CHUNK_SIZE: usize = 65536; // 64KB

/// Enum `FileTransferEvent` đại diện cho các sự kiện cấp cao mà `FileTransferBehaviour`
/// sẽ gửi lên `Swarm` để thông báo cho ứng dụng.
#[derive(Debug)]
pub enum FileTransferEvent {
    /// Một file đã được nhận thành công.
    FileReceived {
        from: PeerId,
        file_name: String,
        size: u64,
    },
    /// Một file đã được gửi thành công.
    FileSent {
        to: PeerId,
        file_name: String,
    },
    /// Đã xảy ra lỗi trong quá trình truyền file.
    Error {
        error: String,
    },
}

/// `FileTransferBehaviour` là một `NetworkBehaviour` tùy chỉnh để quản lý logic truyền file.
pub struct FileTransferBehaviour {
    // Hàng đợi các sự kiện để gửi lên Swarm.
    events: VecDeque<ToSwarm<FileTransferEvent, HandlerRequest>>,
    // Hàng đợi các yêu cầu gửi file đang chờ xử lý.
    pending_requests: VecDeque<(PeerId, String)>,
}

impl FileTransferBehaviour {
    pub fn new() -> Self {
        FileTransferBehaviour {
            events: VecDeque::new(),
            pending_requests: VecDeque::new(),
        }
    }

    /// Thêm một yêu cầu gửi file vào hàng đợi.
    pub fn send_file(&mut self, peer: PeerId, file_path: String) {
        self.pending_requests.push_back((peer, file_path));
    }
}

/// Triển khai `NetworkBehaviour` cho `FileTransferBehaviour`.
impl NetworkBehaviour for FileTransferBehaviour {
    // `ConnectionHandler` sẽ quản lý các kết nối riêng lẻ.
    type ConnectionHandler = FileTransferHandler;
    // Loại sự kiện mà behaviour này tạo ra.
    type ToSwarm = FileTransferEvent;

    // Tạo một handler mới cho kết nối đến (inbound).
    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: libp2p::swarm::ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        Ok(FileTransferHandler::new())
    }

    // Tạo một handler mới cho kết nối đi (outbound).
    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: libp2p::swarm::ConnectionId,
        _peer: PeerId,
        _addr: &Multiaddr,
        _role_override: libp2p::core::Endpoint,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        Ok(FileTransferHandler::new())
    }

    fn on_swarm_event(&mut self, _event: libp2p::swarm::FromSwarm) {}

    /// Xử lý sự kiện từ `ConnectionHandler`.
    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        _connection_id: libp2p::swarm::ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        // Chuyển đổi sự kiện từ handler thành sự kiện của behaviour và đẩy vào hàng đợi.
        match event {
            HandlerEvent::FileReceived { file_name, size } => {
                self.events
                    .push_back(ToSwarm::GenerateEvent(FileTransferEvent::FileReceived {
                        from: peer_id,
                        file_name,
                        size,
                    }));
            }
            HandlerEvent::FileSent { file_name } => {
                self.events
                    .push_back(ToSwarm::GenerateEvent(FileTransferEvent::FileSent {
                        to: peer_id,
                        file_name,
                    }));
            }
            HandlerEvent::Error(error) => {
                self.events
                    .push_back(ToSwarm::GenerateEvent(FileTransferEvent::Error { error }));
            }
        }
    }

    /// `poll` được Swarm gọi định kỳ để xử lý các tác vụ của behaviour.
    fn poll(
        &mut self,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        // Nếu có sự kiện đang chờ, gửi nó lên Swarm.
        if let Some(event) = self.events.pop_front() {
            return Poll::Ready(event);
        }

        // Nếu có yêu cầu gửi file đang chờ, gửi nó xuống cho handler thích hợp.
        if let Some((peer, path)) = self.pending_requests.pop_front() {
            return Poll::Ready(ToSwarm::NotifyHandler {
                peer_id: peer,
                handler: libp2p::swarm::NotifyHandler::Any,
                event: HandlerRequest::SendFile(path),
            });
        }

        Poll::Pending
    }
}

/// Yêu cầu được gửi từ `FileTransferBehaviour` xuống `FileTransferHandler`.
#[derive(Debug)]
pub enum HandlerRequest {
    SendFile(String),
}

/// Sự kiện được gửi từ `FileTransferHandler` lên `FileTransferBehaviour`.
#[derive(Debug)]
pub enum HandlerEvent {
    FileReceived { file_name: String, size: u64 },
    FileSent { file_name: String },
    Error(String),
}

/// `FileTransferHandler` quản lý một kết nối duy nhất với một peer.
/// Nó xử lý việc mở các substream để gửi và nhận file.
pub struct FileTransferHandler {
    // Task bất đồng bộ để nhận file.
    inbound: Option<tokio::task::JoinHandle<Result<HandlerEvent>>>,
    // Hàng đợi các file cần gửi đi.
    outbound: VecDeque<String>,
    // Task bất đồng bộ để gửi file.
    outbound_task: Option<tokio::task::JoinHandle<Result<HandlerEvent>>>,
}

impl FileTransferHandler {
    pub fn new() -> Self {
        FileTransferHandler {
            inbound: None,
            outbound: VecDeque::new(),
            outbound_task: None,
        }
    }
}

impl ConnectionHandler for FileTransferHandler {
    type FromBehaviour = HandlerRequest;
    type ToBehaviour = HandlerEvent;
    type InboundProtocol = ReadyUpgrade<StreamProtocol>;
    type OutboundProtocol = ReadyUpgrade<StreamProtocol>;
    type InboundOpenInfo = ();
    type OutboundOpenInfo = String; // Đường dẫn file

    // Lắng nghe trên giao thức truyền file của chúng ta.
    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL), ())
    }

    /// Xử lý yêu cầu từ `FileTransferBehaviour`.
    fn on_behaviour_event(&mut self, event: Self::FromBehaviour) {
        match event {
            HandlerRequest::SendFile(path) => {
                // Thêm file vào hàng đợi để gửi đi.
                self.outbound.push_back(path);
            }
        }
    }

    fn connection_keep_alive(&self) -> bool {
        true
    }

    /// `poll` được gọi định kỳ để xử lý các tác vụ của handler.
    fn poll(
        &mut self,
        cx: &mut TaskContext<'_>,
    ) -> Poll<
        ConnectionHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::ToBehaviour>,
    > {
        // Kiểm tra xem task nhận file đã hoàn thành chưa.
        if let Some(ref mut task) = self.inbound {
            if let Poll::Ready(result) = task.poll_unpin(cx) {
                self.inbound = None;
                match result {
                    Ok(Ok(event)) => return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(event)),
                    Ok(Err(e)) => return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(HandlerEvent::Error(e.to_string()))),
                    Err(e) => return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(HandlerEvent::Error(format!("Lỗi task: {}", e)))),
                }
            }
        }

        // Kiểm tra xem task gửi file đã hoàn thành chưa.
        if let Some(ref mut task) = self.outbound_task {
            if let Poll::Ready(result) = task.poll_unpin(cx) {
                self.outbound_task = None;
                match result {
                    Ok(Ok(event)) => return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(event)),
                    Ok(Err(e)) => return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(HandlerEvent::Error(e.to_string()))),
                    Err(e) => return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(HandlerEvent::Error(format!("Lỗi task: {}", e)))),
                }
            }
        }

        // Nếu có file trong hàng đợi gửi đi, yêu cầu mở một substream mới.
        if let Some(path) = self.outbound.pop_front() {
            return Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest {
                protocol: SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL), path),
            });
        }

        Poll::Pending
    }

    /// Xử lý các sự kiện liên quan đến kết nối và substream.
    fn on_connection_event(
        &mut self,
        event: libp2p::swarm::handler::ConnectionEvent<
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
        match event {
            // Một substream đến đã được thương lượng thành công.
            libp2p::swarm::handler::ConnectionEvent::FullyNegotiatedInbound(
                libp2p::swarm::handler::FullyNegotiatedInbound { protocol: stream, .. },
            ) => {
                // Nếu chưa có task nhận file nào đang chạy, tạo một task mới.
                if self.inbound.is_none() {
                    self.inbound = Some(tokio::spawn(async move { receive_file(stream).await }));
                }
            }
            // Một substream đi đã được thương lượng thành công.
            libp2p::swarm::handler::ConnectionEvent::FullyNegotiatedOutbound(
                libp2p::swarm::handler::FullyNegotiatedOutbound {
                    protocol: stream,
                    info: file_path,
                },
            ) => {
                // Nếu chưa có task gửi file nào đang chạy, tạo một task mới.
                if self.outbound_task.is_none() {
                    self.outbound_task =
                        Some(tokio::spawn(async move { send_file(stream, file_path).await }));
                }
            }
            _ => {}
        }
    }
}

/// Hàm bất đồng bộ để gửi một file qua một `Stream`.
async fn send_file(mut stream: Stream, file_path: String) -> Result<HandlerEvent> {
    let path = Path::new(&file_path);

    if !path.exists() {
        return Err(anyhow::anyhow!("File không tồn tại: {}", file_path));
    }

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Tên file không hợp lệ")?
        .to_string();

    let metadata = fs::metadata(path)?;
    let file_size = metadata.len();

    // Gửi metadata: độ dài tên file, tên file, kích thước file.
    let file_name_bytes = file_name.as_bytes();
    let file_name_len = file_name_bytes.len() as u32;

    stream.write_all(&file_name_len.to_be_bytes()).await?;
    stream.write_all(file_name_bytes).await?;
    stream.write_all(&file_size.to_be_bytes()).await?;

    // Mở file và bắt đầu gửi nội dung theo từng đoạn (chunk).
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut total_sent = 0u64;

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break; // Hết file
        }

        stream.write_all(&buffer[..n]).await?;
        total_sent += n as u64;

        // In tiến trình gửi file.
        let progress = (total_sent as f64 / file_size as f64 * 100.0) as u32;
        print!("\r{} {}%", "Đang gửi...".bright_cyan(), progress);
        io::Write::flush(&mut io::stdout()).ok();
    }

    // Đảm bảo tất cả dữ liệu đã được gửi và đóng stream.
    stream.flush().await?;
    stream.close().await?;

    println!();

    Ok(HandlerEvent::FileSent { file_name })
}

/// Hàm bất đồng bộ để nhận một file từ một `Stream`.
async fn receive_file(mut stream: Stream) -> Result<HandlerEvent> {
    // Đọc metadata: độ dài tên file, tên file, kích thước file.
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes).await?;
    let file_name_len = u32::from_be_bytes(len_bytes) as usize;

    let mut file_name_bytes = vec![0u8; file_name_len];
    stream.read_exact(&mut file_name_bytes).await?;
    let file_name = String::from_utf8(file_name_bytes)?;

    let mut size_bytes = [0u8; 8];
    stream.read_exact(&mut size_bytes).await?;
    let file_size = u64::from_be_bytes(size_bytes);

    // Chuẩn bị để lưu file nhận được.
    let received_dir = PathBuf::from("received_files");
    tokio::fs::create_dir_all(&received_dir).await?;

    let file_path = received_dir.join(&file_name);
    let mut file = tokio::fs::File::create(&file_path).await?;

    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut total_received = 0u64;

    // Bắt đầu nhận nội dung file theo từng đoạn.
    while total_received < file_size {
        let to_read = std::cmp::min(CHUNK_SIZE, (file_size - total_received) as usize);
        let n = stream.read(&mut buffer[..to_read]).await?;

        if n == 0 {
            break; // Stream đã đóng
        }

        file.write_all(&buffer[..n]).await?;
        total_received += n as u64;

        // In tiến trình nhận file.
        let progress = (total_received as f64 / file_size as f64 * 100.0) as u32;
        print!("\r{} {}%", "Đang nhận...".bright_cyan(), progress);
        io::Write::flush(&mut io::stdout()).ok();
    }

    println!();

    file.flush().await?;

    Ok(HandlerEvent::FileReceived {
        file_name,
        size: total_received,
    })
}
