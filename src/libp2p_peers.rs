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

/// Helper function to parse peer name from agent version string.
fn parse_peer_name_from_agent_version(agent_version: &str) -> Option<&str> {
    agent_version
        .strip_prefix("filemesh-rs/0.1.0 (peer_name: ")
        .and_then(|s| s.strip_suffix(')'))
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
        let identify = identify::Behaviour::new(
            identify::Config::new(PROTOCOL_VERSION.to_string(), local_key.public())
                .with_agent_version(format!("filemesh-rs/0.1.0 (peer_name: {})", peer_name)),
        );
        let ping = ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(5)));

        // `relay::client::new` trả về một tuple (Transport, Behaviour).
        let (relay_transport, relay_client) = relay::client::new(local_peer_id);

        let dcutr = dcutr::Behaviour::new(local_peer_id);
        let autonat = autonat::Behaviour::new(
            local_peer_id,
            autonat::Config {
                retry_interval: Duration::from_secs(5),
                refresh_interval: Duration::from_secs(20),
                boot_delay: Duration::from_secs(2),
                throttle_server_period: Duration::ZERO,
                only_global_ips: false,
                ..Default::default()
            },
        );
        let file_transfer = FileTransferBehaviour::new();

        // Xây dựng transport layer với TCP keepalive.
        let tcp_config = tcp::Config::new()
            .nodelay(true)
            .port_reuse(true);
        
        let transport = relay_transport
            .or_transport(tcp::tokio::Transport::new(tcp_config))
            .upgrade(libp2p::core::upgrade::Version::V1)
            .authenticate(noise::Config::new(&local_key)?)
            .multiplex(yamux::Config::default())
            .timeout(Duration::from_secs(60))
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
                .with_idle_connection_timeout(Duration::from_secs(300)),
        );

        // Quay số đến các bootstrap node để khám phá mạng lưới.
        for addr in BOOTSTRAP_NODES.iter() {
            // Chúng ta bỏ qua lỗi parse vì các địa chỉ DNS sẽ thất bại, nhưng điều này là chấp nhận được.
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
            has_broadcasted_join: false,
        })
    }

    /// Bắt đầu peer lắng nghe kết nối và tham gia phòng.
    pub async fn start(&mut self) -> Result<()> {
        // Lắng nghe trên tất cả các interface mạng (0.0.0.0) với một port ngẫu nhiên.
        self.swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        self.swarm.listen_on("/ip6/::/tcp/0".parse()?)?;

        // **QUAN TRỌNG**: Lắng nghe thông qua các relay công khai.
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

        if self.connected_peers.is_empty() {
            println!("{}", "  Chưa có peer nào trong phòng.".bright_black());
            println!();
            let _ = io::stdout().flush();
            return;
        }

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

            // Lấy ID của tất cả các peer đã kết nối, trừ chính nó.
            let local_id = *self.swarm.local_peer_id();
            let peers: Vec<PeerId> = self
                .connected_peers
                .keys()
                .filter(|&&id| id != local_id)
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
                        UserCommand::Dial(addr) => {
                            match addr.parse::<Multiaddr>() {
                                Ok(multiaddr) => {
                                    if let Err(e) = peer.swarm.dial(multiaddr.clone()) {
                                        println!("{} {}: {}", "Lỗi khi quay số".red(), addr, e);
                                    } else {
                                        println!("{} {}", "Đang quay số đến:".cyan(), multiaddr);
                                    }
                                }
                                Err(e) => {
                                    println!("{} '{}': {}", "Địa chỉ Multiaddr không hợp lệ".red(), addr, e);
                                }
                            }
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

                        // Xử lý sự kiện Ping.
                        SwarmEvent::Behaviour(FileMeshBehaviourEvent::Ping(event)) => match event {
                            ping::Event {
                                peer,
                                result: Result::Ok(rtt),
                                ..
                            } => {
                                println!(
                                    "{} {} {} {:?}",
                                    "Ping thành công tới".bright_green(),
                                    peer.to_string().bright_black(),
                                    "trong".bright_black(),
                                    rtt
                                );
                            }
                            ping::Event {
                                peer,
                                result: Result::Err(err),
                                ..
                            } => {
                                println!(
                                    "{} {} {} {}",
                                    "Ping thất bại tới".red(),
                                    peer.to_string().bright_black(),
                                    ":".red(),
                                    err
                                );
                            }
                        },

                        // Nhận được tin nhắn Gossipsub.
                        SwarmEvent::Behaviour(FileMeshBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                            propagation_source,
                            message,
                            ..
                        })) => {
                            if message.topic == peer.room.topic().hash() && propagation_source != *peer.local_peer_id() {
                                if let Ok(msg) = String::from_utf8(message.data.clone()) {
                                    // Nếu là tin nhắn "JOIN", cập nhật thông tin peer.
                                    if let Some(name) = msg.strip_prefix("JOIN:") {
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
                            if let Some(name) = parse_peer_name_from_agent_version(&info.agent_version) {
                                if let Some(peer_info) = peer.connected_peers.get_mut(&peer_id) {
                                    if peer_info.name.is_none() {
                                        peer_info.name = Some(name.to_string());
                                    }
                                } else {
                                    peer.handle_new_peer(peer_id, info.listen_addrs.clone());
                                    if let Some(peer_info) = peer.connected_peers.get_mut(&peer_id) {
                                        peer_info.name = Some(name.to_string());
                                    }
                                }
                            } else {
                                peer.handle_new_peer(peer_id, info.listen_addrs.clone());
                            }

                            for addr in &info.listen_addrs {
                                peer.swarm.add_external_address(addr.clone());
                            }

                            // Thêm peer này làm autonat server để probe NAT
                            peer.swarm.behaviour_mut().autonat.add_server(peer_id, info.listen_addrs.first().cloned());

                            // Nếu peer hỗ trợ relay, thêm vào danh sách relay.
                            if info.protocols.iter().any(|p| p.as_ref() == "/libp2p/circuit/relay/0.2.0/hop") {
                                peer.relay_peers.insert(peer_id);
                                println!(
                                    "{} {}",
                                    "Đã khám phá relay peer:".bright_cyan(),
                                    peer_id.to_string().bright_black()
                                );
                            }
                            
                            // Nếu đang có kết nối relayed, thử dial trực tiếp đến các địa chỉ public
                            if let Some(peer_info) = peer.connected_peers.get(&peer_id) {
                                if peer_info.connection_type == ConnectionType::Relayed {
                                    // Lọc ra các địa chỉ có thể dial trực tiếp (không phải relay)
                                    let direct_addrs: Vec<_> = info.listen_addrs.iter()
                                        .filter(|addr| {
                                            let addr_str = addr.to_string();
                                            !addr_str.contains("/p2p-circuit") && 
                                            (addr_str.contains("/ip4/") || addr_str.contains("/ip6/"))
                                        })
                                        .cloned()
                                        .collect();
                                    
                                    if !direct_addrs.is_empty() {
                                        println!(
                                            "{} {} {}",
                                            "[→]".bright_cyan(),
                                            "Đã nhận địa chỉ công khai, thử kết nối trực tiếp đến:".bright_cyan(),
                                            peer_id.to_string().bright_black()
                                        );
                                        
                                        // Thử dial đến các địa chỉ (libp2p tự động thêm peer_id nếu cần)
                                        for addr in direct_addrs.iter().take(2) {
                                            let dial_addr = if addr.to_string().contains(&format!("/p2p/{}", peer_id)) {
                                                // Địa chỉ đã có peer_id
                                                addr.clone()
                                            } else {
                                                // Thêm peer_id vào địa chỉ
                                                addr.clone().with(libp2p::multiaddr::Protocol::P2p(peer_id))
                                            };
                                            
                                            println!(
                                                "    Thử dial: {}",
                                                dial_addr.to_string().bright_black()
                                            );
                                            
                                            if let Err(e) = peer.swarm.dial(dial_addr.clone()) {
                                                println!(
                                                    "    [!] {} - {}",
                                                    "Lỗi dial:".bright_yellow(),
                                                    e.to_string().bright_black()
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Kết nối được nâng cấp thành công qua hole punching.
                        SwarmEvent::Behaviour(FileMeshBehaviourEvent::Dcutr(dcutr::Event {
                            remote_peer_id,
                            result: Ok(_),
                        })) => {
                            println!(
                                "{} {} {}",
                                "✓".bright_green(),
                                "DCUtR thành công! Đã nâng cấp lên kết nối trực tiếp:".bright_green(),
                                remote_peer_id.to_string().bright_black()
                            );
                            peer.update_connection_type(remote_peer_id, ConnectionType::HolePunched);
                        }
                        
                        // DCUtR thất bại.
                        SwarmEvent::Behaviour(FileMeshBehaviourEvent::Dcutr(dcutr::Event {
                            remote_peer_id,
                            result: Err(err),
                        })) => {
                            println!(
                                "{} {} {}",
                                "✗".bright_red(),
                                "DCUtR thất bại:".bright_red(),
                                remote_peer_id.to_string().bright_black()
                            );
                            println!(
                                "    {} {}",
                                "Lý do:".bright_black(),
                                err.to_string().bright_black()
                            );
                            println!(
                                "    {} {}",
                                "ℹ".bright_blue(),
                                "Kết nối sẽ tiếp tục qua relay server. Cả hai peer có thể đang sau NAT.".bright_black()
                            );
                            
                            // Đảm bảo connection type vẫn là Relayed
                            if let Some(peer_info) = peer.connected_peers.get_mut(&remote_peer_id) {
                                peer_info.connection_type = ConnectionType::Relayed;
                            }
                        }

                        // Trạng thái NAT thay đổi.
                        SwarmEvent::Behaviour(FileMeshBehaviourEvent::Autonat(autonat::Event::StatusChanged {
                            old,
                            new,
                        })) => {
                            println!(
                                "{} {:?} → {:?}",
                                "[NAT] Trạng thái NAT đã thay đổi:".bright_cyan(),
                                old,
                                new
                            );
                            
                            // Hiển thị gợi ý nếu ở sau NAT
                            if matches!(new, autonat::NatStatus::Private) {
                                println!(
                                    "    [i] {}",
                                    "Bạn đang ở sau NAT. Kết nối sẽ sử dụng relay và DCUtR để thiết lập.".bright_black()
                                );
                            } else if matches!(new, autonat::NatStatus::Public(_)) {
                                println!(
                                    "    [OK] {}",
                                    "Bạn có địa chỉ công khai. Có thể nhận kết nối trực tiếp.".bright_black()
                                );
                            }
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
                                // Chỉ hiển thị lỗi quan trọng, bỏ qua timeout từ private IP
                                let error_str = error.to_string();
                                let is_private_timeout = error_str.contains("Timeout has been reached") && 
                                    (error_str.contains("/ip4/10.") || 
                                     error_str.contains("/ip4/172.") || 
                                     error_str.contains("/ip4/192.168."));
                                
                                if !is_private_timeout {
                                    println!(
                                        "{} {:?}: {}",
                                        "Lỗi kết nối".red(),
                                        peer_id,
                                        error
                                    );
                                }
        }
                        // Thiết lập kết nối thành công.
                        SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                            let addrs = vec![endpoint.get_remote_address().clone()];
                            let is_relayed = endpoint.get_remote_address().to_string().contains("/p2p-circuit");
                            
                            peer.handle_new_peer(peer_id, addrs);
                            peer.broadcast_join();
                            
                            // Tự động thử DCUtR nếu kết nối là relayed
                            if is_relayed && !peer.relay_peers.contains(&peer_id) {
                                println!(
                                    "{} {} - {}",
                                    "[↻]".bright_yellow(),
                                    "Đang thử nâng cấp kết nối relayed lên direct (DCUtR)...".bright_yellow(),
                                    peer_id.to_string().bright_black()
                                );
                            }
                        }

                        // Kết nối bị đóng.
                        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                            if let Some(info) = peer.connected_peers.get(&peer_id) {
                                let name = info.name.as_deref().unwrap_or("Unknown");
                                let was_relayed = info.connection_type == ConnectionType::Relayed;
                                
                                println!(
                                    "{} {} {} {}",
                                    "←".bright_red(),
                                    name.bright_yellow(),
                                    "ngắt kết nối".bright_black(),
                                    if let Some(err) = cause {
                                        format!("({})", err).bright_black().to_string()
                                    } else {
                                        "".to_string()
                                    }
                                );
                                
                                // Nếu là kết nối relayed, thử reconnect qua relay
                                if was_relayed && !peer.relay_peers.contains(&peer_id) {
                                    if let Some(relay_addr) = info.addrs.iter()
                                        .find(|addr| addr.to_string().contains("/p2p-circuit")) {
                                        println!(
                                            "    [↻] {}",
                                            "Thử kết nối lại qua relay...".bright_cyan()
                                        );
                                        
                                        if let Err(e) = peer.swarm.dial(relay_addr.clone()) {
                                            println!(
                                                "    [!] {} - {}",
                                                "Không thể reconnect:".bright_yellow(),
                                                e.to_string().bright_black()
                                            );
                                        }
                                    }
                                }
                                
                                // Chỉ xóa khỏi danh sách nếu không phải relay peer
                                if !peer.relay_peers.contains(&peer_id) {
                                    peer.connected_peers.remove(&peer_id);
                                }
                            }
                        }

                        _ => {}
                    }
                }
            }
    }

    Ok(())
}