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

const PROTOCOL: StreamProtocol = StreamProtocol::new("/filemesh/1.0.0");
const CHUNK_SIZE: usize = 65536;

#[derive(Debug)]
pub enum FileTransferEvent {
    FileReceived {
        from: PeerId,
        file_name: String,
        size: u64,
    },
    FileSent {
        to: PeerId,
        file_name: String,
    },
    Error {
        error: String,
    },
}

#[derive(Debug)]
// pub enum FileTransferRequest {
//     SendFile { peer: PeerId, path: String },
// }

pub struct FileTransferBehaviour {
    events: VecDeque<ToSwarm<FileTransferEvent, HandlerRequest>>,
    pending_requests: VecDeque<(PeerId, String)>,
}

impl FileTransferBehaviour {
    pub fn new() -> Self {
        FileTransferBehaviour {
            events: VecDeque::new(),
            pending_requests: VecDeque::new(),
        }
    }

    pub fn send_file(&mut self, peer: PeerId, file_path: String) {
        // Read the file into memory
        let data = match std::fs::read(&file_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to read file {}: {}", file_path, e);
                return;
            }
        };

        let file_name = std::path::Path::new(&file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown_file".to_string());

        // Send the file bytes through your behaviour
        self.send_bytes(peer, file_name, data);
    }
    pub fn send_bytes(&mut self, peer: PeerId, file_name: String, _data: Vec<u8>) {
        // Here you would implement the logic to send the bytes to the peer.
        // For simplicity, we just enqueue a request.
        self.pending_requests.push_back((peer, file_name));
    }
}

impl NetworkBehaviour for FileTransferBehaviour {
    type ConnectionHandler = FileTransferHandler;
    type ToSwarm = FileTransferEvent;

    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: libp2p::swarm::ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        Ok(FileTransferHandler::new())
    }

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

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        _connection_id: libp2p::swarm::ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
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

    fn poll(
        &mut self,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        if let Some(event) = self.events.pop_front() {
            return Poll::Ready(event);
        }

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

#[derive(Debug)]
pub enum HandlerRequest {
    SendFile(String),
}

#[derive(Debug)]
pub enum HandlerEvent {
    FileReceived { file_name: String, size: u64 },
    FileSent { file_name: String },
    Error(String),
}

pub struct FileTransferHandler {
    inbound: Option<tokio::task::JoinHandle<Result<HandlerEvent>>>,
    outbound: VecDeque<String>,
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
    type OutboundOpenInfo = String;

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL), ())
    }

    fn on_behaviour_event(&mut self, event: Self::FromBehaviour) {
        match event {
            HandlerRequest::SendFile(path) => {
                self.outbound.push_back(path);
            }
        }
    }

    fn connection_keep_alive(&self) -> bool {
        true
    }

    fn poll(
        &mut self,
        cx: &mut TaskContext<'_>,
    ) -> Poll<
        ConnectionHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::ToBehaviour>,
    > {
        if let Some(ref mut task) = self.inbound {
            if let Poll::Ready(result) = task.poll_unpin(cx) {
                self.inbound = None;
                match result {
                    Ok(Ok(event)) => {
                        return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(event))
                    }
                    Ok(Err(e)) => {
                        return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(
                            HandlerEvent::Error(e.to_string()),
                        ))
                    }
                    Err(e) => {
                        return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(
                            HandlerEvent::Error(format!("Task error: {}", e)),
                        ))
                    }
                }
            }
        }

        if let Some(ref mut task) = self.outbound_task {
            if let Poll::Ready(result) = task.poll_unpin(cx) {
                self.outbound_task = None;
                match result {
                    Ok(Ok(event)) => {
                        return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(event))
                    }
                    Ok(Err(e)) => {
                        return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(
                            HandlerEvent::Error(e.to_string()),
                        ))
                    }
                    Err(e) => {
                        return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(
                            HandlerEvent::Error(format!("Task error: {}", e)),
                        ))
                    }
                }
            }
        }

        if let Some(path) = self.outbound.pop_front() {
            return Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest {
                protocol: SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL), path),
            });
        }

        Poll::Pending
    }

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
            libp2p::swarm::handler::ConnectionEvent::FullyNegotiatedInbound(
                libp2p::swarm::handler::FullyNegotiatedInbound {
                    protocol: stream, ..
                },
            ) => {
                if self.inbound.is_none() {
                    self.inbound = Some(tokio::spawn(async move { receive_file(stream).await }));
                }
            }
            libp2p::swarm::handler::ConnectionEvent::FullyNegotiatedOutbound(
                libp2p::swarm::handler::FullyNegotiatedOutbound {
                    protocol: stream,
                    info: file_path,
                },
            ) => {
                if self.outbound_task.is_none() {
                    self.outbound_task =
                        Some(tokio::spawn(
                            async move { send_file(stream, file_path).await },
                        ));
                }
            }
            _ => {}
        }
    }
}

async fn send_file(mut stream: Stream, file_path: String) -> Result<HandlerEvent> {
    let path = Path::new(&file_path);

    if !path.exists() {
        return Err(anyhow::anyhow!("File not found: {}", file_path));
    }

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid file name")?
        .to_string();

    let metadata = fs::metadata(path)?;
    let file_size = metadata.len();

    let file_name_bytes = file_name.as_bytes();
    let file_name_len = file_name_bytes.len() as u32;

    AsyncWriteExt::write_all(&mut stream, &file_name_len.to_be_bytes()).await?;
    AsyncWriteExt::write_all(&mut stream, file_name_bytes).await?;
    AsyncWriteExt::write_all(&mut stream, &file_size.to_be_bytes()).await?;

    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut total_sent = 0u64;

    loop {
        let n = <tokio::fs::File as TokioReadExt>::read(&mut file, &mut buffer).await?;
        if n == 0 {
            break;
        }

        AsyncWriteExt::write_all(&mut stream, &buffer[..n]).await?;
        total_sent += n as u64;

        let progress = (total_sent as f64 / file_size as f64 * 100.0) as u32;
        if total_sent % (CHUNK_SIZE as u64 * 10) == 0 || total_sent == file_size {
            print!("\r{} {}%", "Sending...".bright_cyan(), progress);
            io::Write::flush(&mut io::stdout()).ok();
        }
    }

    AsyncWriteExt::flush(&mut stream).await?;
    AsyncWriteExt::close(&mut stream).await?;

    println!();

    Ok(HandlerEvent::FileSent { file_name })
}

async fn receive_file(mut stream: Stream) -> Result<HandlerEvent> {
    let mut len_bytes = [0u8; 4];
    AsyncReadExt::read_exact(&mut stream, &mut len_bytes).await?;
    let file_name_len = u32::from_be_bytes(len_bytes) as usize;

    let mut file_name_bytes = vec![0u8; file_name_len];
    AsyncReadExt::read_exact(&mut stream, &mut file_name_bytes).await?;
    let file_name = String::from_utf8(file_name_bytes)?;

    let mut size_bytes = [0u8; 8];
    AsyncReadExt::read_exact(&mut stream, &mut size_bytes).await?;
    let file_size = u64::from_be_bytes(size_bytes);

    let received_dir = PathBuf::from("received_files");
    tokio::fs::create_dir_all(&received_dir).await?;

    let file_path = received_dir.join(&file_name);
    let mut file = tokio::fs::File::create(&file_path).await?;

    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut total_received = 0u64;

    while total_received < file_size {
        let to_read = std::cmp::min(CHUNK_SIZE, (file_size - total_received) as usize);
        let n = AsyncReadExt::read(&mut stream, &mut buffer[..to_read]).await?;

        if n == 0 {
            break;
        }

        <tokio::fs::File as TokioWriteExt>::write_all(&mut file, &buffer[..n]).await?;
        total_received += n as u64;

        let progress = (total_received as f64 / file_size as f64 * 100.0) as u32;
        if total_received % (CHUNK_SIZE as u64 * 10) == 0 || total_received == file_size {
            print!("\r{} {}%", "Receiving...".bright_cyan(), progress);
            io::Write::flush(&mut io::stdout()).ok();
        }
    }

    println!();

    <tokio::fs::File as TokioWriteExt>::flush(&mut file).await?;

    Ok(HandlerEvent::FileReceived {
        file_name,
        size: total_received,
    })
}
