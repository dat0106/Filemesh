// Import các module và thư viện cần thiết.
use crate::file_transfer::{FileTransferBehaviour, FileTransferEvent};
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
    has_broadcasted_join: bool,
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
        self.has_broadcasted_join = true;
    }

    /// Tạo một `FileMeshPeer` mới.
    pub async fn new(peer_name: String, room_name: String) -> Result<Self> {
        // Tạo cặp khóa định danh cho peer.
        let local_key = libp2p::identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());

        println!(
            "{} {} ({})",
            "Khởi tạo peer:".bright_cyan(),
            peer_name.bright_white(),
            local_peer_id.to_string().bright_black()
        );

        // Cấu hình Gossipsub.
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(1))
            .validation_mode(gossipsub::ValidationMode::Strict)
            .build()
            .context("Cấu hình gossipsub không hợp lệ")?;

        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        )
        .map_err(|e| anyhow::anyhow!("Không thể tạo gossipsub behaviour: {}", e))?;

        // Cấu hình các behaviour khác.
        let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)?;
        let identify = identify::Behaviour::new(identify::Config::new(
            PROTOCOL_VERSION.to_string(),
            local_key.public(),
        ));
        let ping = ping::Behaviour::new(ping::Config::new());

        // `relay::client::new` trả về một tuple (Transport, Behaviour).
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

        // Xây dựng transport layer (phiên bản đơn giản hóa, không có DNS để gỡ lỗi).
        let transport = relay_transport
            .or_transport(tcp::tokio::Transport::new(tcp::Config::new().nodelay(true)))
            .upgrade(libp2p::core::upgrade::Version::V1)
            .authenticate(noise::Config::new(&local_key)?)
            .multiplex(yamux::Config::default())
            .timeout(Duration::from_secs(20))
            .boxed();

        // Kết hợp các behaviour lại.
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

        // Tạo Swarm.
        let mut swarm = Swarm::new(
            transport,
            behaviour,
            local_peer_id,
            libp2p::swarm::Config::with_tokio_executor()
                .with_idle_connection_timeout(Duration::from_secs(60)),
        );

        // Quay số đến các bootstrap node để khám phá mạng lưới.
        // Lưu ý: Các địa chỉ DNS sẽ thất bại ở bước này, nhưng không làm crash chương trình.
        for addr in BOOTSTRAP_NODES.iter() {
            if let Ok(remote_addr) = addr.parse::<Multiaddr>() {
                swarm.dial(remote_addr)?;
            }
        }

        let room = Room::new(room_name, local_peer_id);

        Ok(FileMeshPeer {
            swarm,
            room,
            peer_name,
            connected_peers: HashMap::new(),
            relay_peers: HashSet::new(),
            has_broadcasted_join: false,
        })
    }

    /// Bắt đầu peer lắng nghe kết nối và tham gia phòng.
    pub async fn start(&mut self) -> Result<()> {
        // Lắng nghe trên tất cả các interface mạng (0.0.0.0) với một port ngẫu nhiên.
        self.swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        self.swarm.listen_on("/ip6/::/tcp/0".parse()?)?;

        // **QUAN TRỌNG**: Lắng nghe thông qua các relay công khai.
        // Điều này yêu cầu swarm kết nối đến relay và thiết lập một "địa chỉ chuyển tiếp".
        // Các peer khác sau đó có thể kết nối đến chúng ta thông qua địa chỉ này.
        for addr in RELAY_NODES.iter() {
            if let Ok(relay_addr) = addr.parse::<Multiaddr>() {
                self.swarm
                    .listen_on(relay_addr.with(libp2p::multiaddr::Protocol::P2pCircuit))
                    .expect("Listen on relay failed.");
            }
        }

        // Tham gia vào topic của phòng.
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

        let room_members: Vec<_> = self
            .connected_peers
            .iter()
            .filter(|(_, info)| info.name.is_some())
            .collect();

        if room_members.is_empty() {
            println!("{}", "  Chưa có peer nào trong phòng.".bright_black());
            println!();
            let _ = io::stdout().flush();
            return;
        }

        for (i, (peer_id, info)) in room_members.iter().enumerate() {
            let name = info.name.as_ref().unwrap();

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

            let peers: Vec<PeerId> = self
                .connected_peers
                .iter()
                .filter(|(_, info)| info.name.is_some())
                .map(|(id, _)| *id)
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
            if self
                .connected_peers
                .get(&target_peer)
                .and_then(|info| info.name.as_ref())
                .is_none()
            {
                println!("{}", "Peer không ở trong phòng.".red());
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

    /// Cập nhật loại kết nối của một peer.
    fn update_connection_type(&mut self, peer_id: PeerId, conn_type: ConnectionType) {
        if let Some(info) = self.connected_peers.get_mut(&peer_id) {
            if info.connection_type != conn_type {
                info.connection_type = conn_type.clone();
                println!(
                    "{} {} {}",
                    "Kết nối được nâng cấp:".bright_green(),
                    peer_id.to_string().bright_black(),
                    conn_type
                );
            }
        }
    }

    pub fn local_peer_id(&self) -> &PeerId {
        self.room.local_peer_id()
    }

    /// Xử lý khi một peer mới được phát hiện hoặc kết nối.
    fn handle_new_peer(&mut self, peer_id: PeerId, addrs: Vec<Multiaddr>) {
        let is_relayed = addrs
            .iter()
            .any(|addr| addr.to_string().contains("/p2p-circuit"));

        let conn_type = if is_relayed {
            ConnectionType::Relayed
        } else {
            ConnectionType::Direct
        };

        if !self.connected_peers.contains_key(&peer_id) {
            self.connected_peers.insert(
                peer_id,
                PeerInfo {
                    name: None,
                    connection_type: conn_type,
                    addrs,
                },
            );
            self.broadcast_join();
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
        // `tokio::select!` cho phép xử lý đồng thời nhiều sự kiện.
        // `biased` ưu tiên xử lý lệnh người dùng để giao diện luôn phản hồi nhanh.
        tokio::select! {
            biased;
            // Xử lý lệnh từ người dùng.
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    UserCommand::ListPeers => {
                        println!("{}", "Đang liệt kê peers...".bright_black());
                        peer.list_peers();
                    }
                    UserCommand::SendFile { file_path, peer_id, broadcast } => {
                        peer.send_file(file_path, peer_id, broadcast).await;
                    }
                    UserCommand::Quit => {
                        println!("{}", "Đang tắt peer...".yellow());
                        peer.room.leave(&mut peer.swarm.behaviour_mut().gossipsub).ok();
                        break;
                    }
                }
            }

            // Xử lý sự kiện từ Swarm.
            event = peer.swarm.select_next_some() => {
                match event {
                    // Sự kiện từ Relay Client, rất quan trọng để gỡ lỗi kết nối.
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::RelayClient(event)) => {
                        println!("{} {:?}", "Sự kiện Relay Client:".bright_yellow(), event);
                    }

                    // Nhận được tin nhắn Gossipsub.
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    })) => {
                        if message.topic == peer.room.topic().hash() && propagation_source != *peer.local_peer_id() {
                            if let Ok(msg) = String::from_utf8(message.data.clone()) {
                                // Nếu là tin nhắn "JOIN", cập nhật thông tin peer.
                                if msg.starts_with("JOIN:") {
                                    let name = msg.strip_prefix("JOIN:").unwrap_or("Unknown");
                                    if !peer.connected_peers.contains_key(&propagation_source) {
                                        peer.handle_new_peer(propagation_source, Vec::new());
                                    }
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
                                    let _ = io::stdout().flush();
                                }
                            }
                        }
                    }

                    // Khám phá peer mới qua mDNS.
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                        for (peer_id, addr) in list {
                            peer.swarm.dial(addr.clone()).ok();
                            peer.handle_new_peer(peer_id, vec![addr]);
                        }
                    }

                    // Nhận thông tin Identify từ peer khác.
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Identify(identify::Event::Received {
                        peer_id,
                        info,
                    })) => {
                        peer.handle_new_peer(peer_id, info.listen_addrs.clone());

                        for addr in &info.listen_addrs {
                            peer.swarm.add_external_address(addr.clone());
                        }

                        // Nếu peer hỗ trợ relay, thêm vào danh sách relay.
                        if info.protocols.iter().any(|p| p.as_ref() == "/libp2p/circuit/relay/0.2.0/hop") {
                            peer.relay_peers.insert(peer_id);
                            println!(
                                "{} {}",
                                "Đã khám phá relay peer:".bright_cyan(),
                                peer_id.to_string().bright_black()
                            );
                        }
                    }

                    // Kết nối được nâng cấp thành công qua hole punching.
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Dcutr(dcutr::Event {
                        remote_peer_id,
                        result: Ok(_),
                    })) => {
                        peer.update_connection_type(remote_peer_id, ConnectionType::HolePunched);
                    }

                    // Trạng thái NAT thay đổi.
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Autonat(autonat::Event::StatusChanged {
                        old,
                        new,
                    })) => {
                        println!(
                            "{} {:?} → {:?}",
                            "Trạng thái NAT đã thay đổi:".bright_cyan(),
                            old,
                            new
                        );
                    }

                    // Sự kiện từ behaviour truyền file.
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::FileTransfer(file_event)) => {
                        match file_event {
                            FileTransferEvent::FileReceived { from, file_name, size } => {
                                let peer_name = peer.connected_peers.get(&from)
                                    .and_then(|p| p.name.as_ref())
                                    .map(|n| n.as_str())
                                    .unwrap_or("Unknown");

                                println!(
                                    "{} {} {} {} ({})",
                                    "✓".bright_green(),
                                    "Đã nhận file:".bright_green(),
                                    file_name.bright_white(),
                                    format!("từ {}", peer_name).bright_black(),
                                    format!("{} bytes", size).bright_black()
                                );
                            }
                            FileTransferEvent::FileSent { to, file_name } => {
                                println!(
                                    "{} {} {} {}",
                                    "✓".bright_green(),
                                    "Đã gửi file:".bright_green(),
                                    file_name.bright_white(),
                                    format!("tới {}", to.to_string()).bright_black()
                                );
                            }
                            FileTransferEvent::Error { error } => {
                                println!(
                                    "{} {} {}",
                                    "✗".red(),
                                    "Lỗi truyền file:".red(),
                                    error
                                );
                            }
                        }
                    }

                    // Một địa chỉ lắng nghe mới được mở.
                    SwarmEvent::NewListenAddr { address, .. } => {
                        println!(
                            "{} {}",
                            "Đang lắng nghe trên:".bright_cyan(),
                            address.to_string().bright_black()
                        );
                    }

                    // Bắt lỗi khi không thể kết nối đến một peer khác (bao gồm cả relay).
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        println!(
                            "{} to {:?}: {}",
                            "Lỗi kết nối đi".red(),
                            peer_id,
                            error
                        );
                    }

                    // Thiết lập kết nối thành công.
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        let addrs = vec![endpoint.get_remote_address().clone()];
                        peer.handle_new_peer(peer_id, addrs);
                        peer.broadcast_join();
                    }

                    // Kết nối bị đóng.
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        if let Some(info) = peer.connected_peers.remove(&peer_id) {
                            let name = info.name.as_deref().unwrap_or("Unknown");
                            println!(
                                "{} {} {}",
                                "←".bright_red(),
                                name.bright_yellow(),
                                "đã rời phòng".bright_black()
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
