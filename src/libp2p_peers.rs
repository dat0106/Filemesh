// Import các module và thư viện cần thiết.
use crate::file_transfer::{FileTransferBehaviour};
use crate::room::Room;
use crate::UserCommand;
use anyhow::{Context, Result};
use colored::Colorize;
use futures::StreamExt;
use libp2p::{
    autonat, dcutr, gossipsub, identify, mdns, noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm, Transport,
};
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::time::Duration;
use tokio::sync::mpsc;

// Hằng số định nghĩa phiên bản giao thức của ứng dụng.
const PROTOCOL_VERSION: &str = "/filemesh/1.0.0";

// Danh sách các bootstrap node công khai của IPFS để khám phá peer.
const BOOTSTRAP_NODES: [&str; 4] = [
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTf5zyPAPRU8KqUiCoE",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
];

// Danh sách các relay node công khai (chỉ sử dụng TCP).
const RELAY_NODES: [&str; 1] =
    ["/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ"];

/// Enum `ConnectionType` biểu diễn trạng thái kết nối của một peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionType {
    Direct,      // Kết nối trực tiếp.
    Relayed,     // Kết nối thông qua một relay.
    HolePunched, // Kết nối đã được "đục lỗ" (hole-punched).
}

impl std::fmt::Display for ConnectionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionType::Direct => write!(f, "{}", "Direct".bright_green()),
            ConnectionType::Relayed => write!(f, "{}", "Relayed".bright_yellow()),
            ConnectionType::HolePunched => write!(f, "{}", "Hole-Punched".bright_cyan()),
        }
    }
}

/// `FileMeshBehaviour` kết hợp nhiều `NetworkBehaviour` khác nhau thành một.
#[derive(NetworkBehaviour)]
pub struct FileMeshBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub relay_client: relay::client::Behaviour,
    pub dcutr: dcutr::Behaviour,
    pub autonat: autonat::Behaviour,
    pub file_transfer: FileTransferBehaviour,
}

/// `PeerInfo` lưu trữ thông tin về một peer đã kết nối.
pub struct PeerInfo {
    pub name: Option<String>,
    pub connection_type: ConnectionType,
    pub addrs: Vec<Multiaddr>,
}

/// `FileMeshPeer` là cấu trúc chính quản lý trạng thái của một peer.
pub struct FileMeshPeer {
    swarm: Swarm<FileMeshBehaviour>,
    room: Room,
    peer_name: String,
    connected_peers: HashMap<PeerId, PeerInfo>,
    relay_peers: HashSet<PeerId>,
}

impl FileMeshPeer {
    /// Gửi tin nhắn quảng bá rằng peer này đã tham gia phòng.
    fn broadcast_join(&mut self) {
        let join_msg = format!("JOIN:{}", self.peer_name);
        if let Err(e) = self.room.broadcast(
            &mut self.swarm.behaviour_mut().gossipsub,
            join_msg.as_bytes(),
        ) {
            eprintln!("{} {}", "Không thể quảng bá tham gia:".red(), e);
        }
    }

    /// Tạo một `FileMeshPeer` mới.
    pub async fn new(peer_name: String, room_name: String) -> Result<Self> {
        let local_key = libp2p::identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());

        println!(
            "{} {} ({})",
            "Khởi tạo peer:".bright_cyan(),
            peer_name.bright_white(),
            local_peer_id.to_string().bright_black()
        );

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10))
            .validation_mode(gossipsub::ValidationMode::Strict)
            .build()
            .context("Cấu hình gossipsub không hợp lệ")?;

        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        )
        .map_err(|e| anyhow::anyhow!(e))?;

        let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)?;
        let identify = identify::Behaviour::new(identify::Config::new(
            PROTOCOL_VERSION.to_string(),
            local_key.public(),
        ));
        
        let ping = ping::Behaviour::new(
            ping::Config::new()
                .with_interval(Duration::from_secs(15))
                .with_timeout(Duration::from_secs(20)),
        );

        let (relay_transport, relay_client) = relay::client::new(local_peer_id);
        let dcutr = dcutr::Behaviour::new(local_peer_id);
        let autonat = autonat::Behaviour::new(
            local_peer_id,
            autonat::Config {
                retry_interval: Duration::from_secs(10),
                refresh_interval: Duration::from_secs(30),
                boot_delay: Duration::from_secs(5),
                ..Default::default()
            },
        );
        let file_transfer = FileTransferBehaviour::new();

        let transport = relay_transport
            .or_transport(tcp::tokio::Transport::new(tcp::Config::new().nodelay(true)))
            .upgrade(libp2p::core::upgrade::Version::V1)
            .authenticate(noise::Config::new(&local_key)?)
            .multiplex(yamux::Config::default())
            .timeout(Duration::from_secs(20))
            .boxed();

        let behaviour = FileMeshBehaviour {
            gossipsub,
            mdns,
            identify,
            ping,
            relay_client,
            dcutr,
            autonat,
            file_transfer,
        };

        let mut swarm = Swarm::new(
            transport,
            behaviour,
            local_peer_id,
            libp2p::swarm::Config::with_tokio_executor()
                .with_idle_connection_timeout(Duration::from_secs(60)),
        );

        for addr in BOOTSTRAP_NODES.iter() {
            if let Ok(remote_addr) = addr.parse::<Multiaddr>() {
                let _ = swarm.dial(remote_addr);
            }
        }

        let room = Room::new(room_name, local_peer_id);

        Ok(FileMeshPeer {
            swarm,
            room,
            peer_name,
            connected_peers: HashMap::new(),
            relay_peers: HashSet::new(),
        })
    }

    /// Bắt đầu peer lắng nghe kết nối và tham gia phòng.
    pub async fn start(&mut self) -> Result<()> {
        self.swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        self.swarm.listen_on("/ip6/::/tcp/0".parse()?)?;

        for addr in RELAY_NODES.iter() {
            if let Ok(relay_addr) = addr.parse::<Multiaddr>() {
                self.swarm
                    .listen_on(relay_addr.with(libp2p::multiaddr::Protocol::P2pCircuit))
                    .expect("Listen on relay failed.");
            }
        }

        self.room.join(&mut self.swarm.behaviour_mut().gossipsub)?;
        self.broadcast_join();
        println!(
            "{} {}",
            "Đã tham gia phòng:".bright_cyan(),
            self.room.name().bright_white()
        );
        println!();
        println!("{}", "Đang lắng nghe kết nối...".bright_black());
        println!();

        Ok(())
    }

    /// Liệt kê các peer trong phòng.
    pub fn list_peers(&self) {
        println!();
        println!(
            "{}",
            "╔════════════════════════════════════════════╗".bright_cyan()
        );
        println!(
            "{}",
            "║           Thành viên trong phòng           ║".bright_cyan()
        );
        println!(
            "{}",
            "╚════════════════════════════════════════════╝".bright_cyan()
        );
        println!();

        if self.connected_peers.is_empty() {
            println!("{}", "  Chưa có peer nào trong phòng.".bright_black());
        } else {
            for (i, (peer_id, info)) in self.connected_peers.iter().enumerate() {
                let name = info.name.as_deref().unwrap_or("(Unknown)");
                println!(
                    "  {}. {} ({})",
                    (i + 1).to_string().bright_white(),
                    name.bright_yellow(),
                    peer_id.to_string().bright_black()
                );
                println!("     Loại kết nối: {}", info.connection_type);
                println!(
                    "     Địa chỉ: {}",
                    info.addrs
                        .first()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| "Không có".to_string())
                        .bright_black()
                );
                println!();
            }
        }
        let _ = io::stdout().flush();
    }

    /// Gửi một file đến một peer cụ thể hoặc quảng bá cho tất cả.
    pub async fn send_file(&mut self, file_path: String, peer_id: Option<PeerId>, broadcast: bool) {
        if broadcast {
            println!(
                "{} {}",
                "Đang quảng bá file:".bright_cyan(),
                file_path.bright_white()
            );
            let local_id = *self.swarm.local_peer_id();
            let peers: Vec<PeerId> = self
                .connected_peers
                .keys()
                .filter(|&&id| id != local_id && !self.relay_peers.contains(&id))
                .cloned()
                .collect();

            if peers.is_empty() {
                println!("{}", "Không có peer nào trong phòng để quảng bá.".red());
                return;
            }

            for peer in peers {
                self.swarm
                    .behaviour_mut()
                    .file_transfer
                    .send_file(peer, file_path.clone());
            }
        } else if let Some(target_peer) = peer_id {
            if !self.connected_peers.contains_key(&target_peer) {
                println!("{}", "Peer không được kết nối.".red());
                return;
            }
            println!(
                "{} {} {} {}",
                "Đang gửi file:".bright_cyan(),
                file_path.bright_white(),
                "tới".bright_black(),
                target_peer.to_string().bright_black()
            );
            self.swarm
                .behaviour_mut()
                .file_transfer
                .send_file(target_peer, file_path);
        }
    }

    /// Xử lý khi một peer mới được phát hiện hoặc kết nối.
    fn handle_new_peer(&mut self, peer_id: PeerId, addrs: &[Multiaddr]) {
        let is_relayed = addrs
            .iter()
            .any(|addr| addr.to_string().contains("/p2p-circuit"));
        let conn_type = if is_relayed {
            ConnectionType::Relayed
        } else {
            ConnectionType::Direct
        };
        if !self.connected_peers.contains_key(&peer_id) {
            println!(
                "{} {} ({})",
                "Phát hiện peer mới:".bright_blue(),
                peer_id,
                conn_type
            );
            self.connected_peers.insert(
                peer_id,
                PeerInfo {
                    name: None,
                    connection_type: conn_type,
                    addrs: addrs.to_vec(),
                },
            );
        }
    }
}

/// Vòng lặp chính của peer, xử lý các sự kiện từ Swarm và lệnh từ người dùng.
pub async fn run_peer(
    peer_name: String,
    room_name: String,
    mut cmd_rx: mpsc::UnboundedReceiver<UserCommand>,
) -> Result<()> {
    let mut peer = FileMeshPeer::new(peer_name.clone(), room_name).await?;
    peer.start().await?;

    loop {
        tokio::select! {
            biased;
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    UserCommand::ListPeers => {
                        peer.list_peers();
                    }
                    UserCommand::SendFile { file_path, peer_id, broadcast } => {
                        peer.send_file(file_path, peer_id, broadcast).await;
                    }
                    UserCommand::Dial(addr) => {
                        if let Ok(multiaddr) = addr.parse::<Multiaddr>() {
                            if let Err(e) = peer.swarm.dial(multiaddr.clone()) {
                                println!("{} {}: {}", "Lỗi khi quay số".red(), addr, e);
                            } else {
                                println!("{} {}", "Đang quay số đến:".cyan(), multiaddr);
                            }
                        } else {
                            println!("{} '{}': {}", "Địa chỉ Multiaddr không hợp lệ".red(), addr, "Lỗi parse");
                        }
                    }
                    UserCommand::Quit => {
                        println!("{}", "Đang tắt peer...".yellow());
                        let _ = peer.room.leave(&mut peer.swarm.behaviour_mut().gossipsub);
                        break;
                    }
                }
            }
            event = peer.swarm.select_next_some() => {
                match event {
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::RelayClient(event)) => {
                        println!("{} {:?}", "Sự kiện Relay Client:".bright_yellow(), event);
                    }
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    })) => {
                        if message.topic == peer.room.topic().hash() && propagation_source != *peer.swarm.local_peer_id() {
                            if let Ok(msg) = String::from_utf8(message.data.clone()) {
                                if let Some(name) = msg.strip_prefix("JOIN:") {
                                    // Nếu chưa biết peer này, hãy thêm vào danh sách.
                                    if !peer.connected_peers.contains_key(&propagation_source) {
                                        peer.handle_new_peer(propagation_source, &[]);
                                    }
                                    // Bây giờ, cập nhật tên cho peer.
                                    if let Some(info) = peer.connected_peers.get_mut(&propagation_source) {
                                        if info.name.is_none() {
                                            info.name = Some(name.to_string());
                                            println!(
                                                "{} {} {}",
                                                "→".bright_green(),
                                                name.bright_yellow(),
                                                "đã tham gia phòng".bright_black()
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                        for (peer_id, addr) in list {
                            let _ = peer.swarm.dial(addr.clone());
                            peer.handle_new_peer(peer_id, &[addr]);
                        }
                    }
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Identify(identify::Event::Received {
                        peer_id,
                        info,
                    })) => {
                        peer.handle_new_peer(peer_id, &info.listen_addrs);
                        for addr in &info.listen_addrs {
                            peer.swarm.add_external_address(addr.clone());
                        }
                        if info.protocols.iter().any(|p| p.as_ref().contains("relay")) {
                            peer.relay_peers.insert(peer_id);
                        }
                    }
                     SwarmEvent::Behaviour(FileMeshBehaviourEvent::Dcutr(dcutr::Event {
                        remote_peer_id,
                        result: Ok(_),
                    })) => {
                        if let Some(info) = peer.connected_peers.get_mut(&remote_peer_id) {
                            info.connection_type = ConnectionType::HolePunched;
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        if let Some(p) = peer_id {
                             println!("{} to {}: {}", "Lỗi kết nối đi".red(), p, error);
                        } else {
                             println!("{} {}", "Lỗi kết nối đi đến peer không xác định:".red(), error);
                        }
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        println!("{} {}", "✓ Kết nối đã được thiết lập với:".green(), peer_id);
                        peer.handle_new_peer(peer_id, &[endpoint.get_remote_address().clone()]);
                        peer.broadcast_join();
                    }
                    SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                        if let Some(info) = peer.connected_peers.remove(&peer_id) {
                            let name = info.name.as_deref().unwrap_or("Unknown");
                            println!(
                                "{} {} {} (Reason: {:?})",
                                "←".bright_red(),
                                name.bright_yellow(),
                                "đã rời phòng".bright_black(),
                                cause
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}