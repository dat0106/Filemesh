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

const PROTOCOL_VERSION: &str = "/filemesh/1.0.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionType {
    Direct,
    Relayed,
    HolePunched,
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

pub struct PeerInfo {
    pub name: Option<String>,
    pub connection_type: ConnectionType,
    pub addrs: Vec<Multiaddr>,
}

pub struct FileMeshPeer {
    swarm: Swarm<FileMeshBehaviour>,
    room: Room,
    peer_name: String,
    connected_peers: HashMap<PeerId, PeerInfo>,
    relay_peers: HashSet<PeerId>,
    has_broadcasted_join: bool,
}

impl FileMeshPeer {
    fn broadcast_join(&mut self) {
        let join_msg = format!("JOIN:{}", self.peer_name);
        if let Err(e) = self.room.broadcast(
            &mut self.swarm.behaviour_mut().gossipsub,
            join_msg.as_bytes(),
        ) {
            eprintln!("{} {}", "Failed to broadcast join:".red(), e);
        }
        self.has_broadcasted_join = true;
    }

    pub async fn new(peer_name: String, room_name: String) -> Result<Self> {
        let local_key = libp2p::identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());

        println!(
            "{} {} ({})",
            "Starting peer:".bright_cyan(),
            peer_name.bright_white(),
            local_peer_id.to_string().bright_black()
        );

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(1))
            .validation_mode(gossipsub::ValidationMode::Strict)
            .build()
            .context("Invalid gossipsub config")?;

        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create gossipsub behaviour: {}", e))?;

        let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)?;

        let identify = identify::Behaviour::new(identify::Config::new(
            PROTOCOL_VERSION.to_string(),
            local_key.public(),
        ));

        let ping = ping::Behaviour::new(ping::Config::new());

        let (mut _relay_transport, relay_client) = relay::client::new(local_peer_id);

        let dcutr = dcutr::Behaviour::new(local_peer_id);

        let autonat = autonat::Behaviour::new(
            local_peer_id,
            autonat::Config {
                retry_interval: Duration::from_secs(10),
                refresh_interval: Duration::from_secs(30),
                boot_delay: Duration::from_secs(5),
                throttle_server_period: Duration::from_secs(1),
                ..Default::default()
            },
        );

        let file_transfer = FileTransferBehaviour::new();

        // Combine relay transport with direct TCP transport so that relay client
        // stays alive and can establish relayed connections.
        let transport = _relay_transport
            .or_transport(tcp::tokio::Transport::default())
            .upgrade(libp2p::core::upgrade::Version::V1)
            .authenticate(noise::Config::new(&local_key)?)
            .multiplex(yamux::Config::default())
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

        let swarm = Swarm::new(
            transport,
            behaviour,
            local_peer_id,
            libp2p::swarm::Config::with_tokio_executor()
                .with_idle_connection_timeout(Duration::from_secs(60)),
        );

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

    pub async fn start(&mut self) -> Result<()> {
        self.swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

        self.room.join(&mut self.swarm.behaviour_mut().gossipsub)?;
        self.broadcast_join();
        println!(
            "{} {}",
            "Joined room:".bright_cyan(),
            self.room.name().bright_white()
        );
        println!();
        println!("{}", "Listening for connections...".bright_black());
        println!();

        Ok(())
    }

    pub fn list_peers(&self) {
        println!();
        println!(
            "{}",
            "╔════════════════════════════════════════════╗".bright_cyan()
        );
        println!(
            "{}",
            "║           Room Members                     ║".bright_cyan()
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
            println!("{}", "  No peers in the room yet.".bright_black());
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
            println!("     Connection Type: {}", info.connection_type);
            println!(
                "     Addresses: {}",
                info.addrs
                    .first()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "None".to_string())
                    .bright_black()
            );
            println!();
        }

        let _ = io::stdout().flush();
    }

    pub async fn send_file(&mut self, file_path: String, peer_id: Option<PeerId>, broadcast: bool) {
        if broadcast {
            println!(
                "{} {}",
                "Broadcasting file:".bright_cyan(),
                file_path.bright_white()
            );

            let peers: Vec<PeerId> = self
                .connected_peers
                .iter()
                .filter(|(_, info)| info.name.is_some())
                .map(|(id, _)| *id)
                .collect();
            if peers.is_empty() {
                println!("{}", "No peers in the room to broadcast to.".red());
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
                println!("{}", "Peer not connected.".red());
                return;
            }
            if self
                .connected_peers
                .get(&target_peer)
                .and_then(|info| info.name.as_ref())
                .is_none()
            {
                println!("{}", "Peer not in the room.".red());
                return;
            }

            println!(
                "{} {} {} {}",
                "Sending file:".bright_cyan(),
                file_path.bright_white(),
                "to".bright_black(),
                target_peer.to_string().bright_black()
            );

            self.swarm
                .behaviour_mut()
                .file_transfer
                .send_file(target_peer, file_path);
        }
    }

    fn update_connection_type(&mut self, peer_id: PeerId, conn_type: ConnectionType) {
        if let Some(info) = self.connected_peers.get_mut(&peer_id) {
            if info.connection_type != conn_type {
                info.connection_type = conn_type.clone();
                println!(
                    "{} {} {}",
                    "Connection upgraded:".bright_green(),
                    peer_id.to_string().bright_black(),
                    conn_type
                );
            }
        }
    }

    pub fn local_peer_id(&self) -> &PeerId {
        self.room.local_peer_id()
    }

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

pub async fn run_peer(
    peer_name: String,
    room_name: String,
    mut cmd_rx: mpsc::UnboundedReceiver<UserCommand>,
) -> Result<()> {
    let mut peer = FileMeshPeer::new(peer_name.clone(), room_name).await?;
    peer.start().await?;

    loop {
        // Prioritize handling user commands to keep the UI responsive.
        tokio::select! {
            biased;
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    UserCommand::ListPeers => {
                        println!("{}", "Listing peers...".bright_black());
                        peer.list_peers();
                    }
                    UserCommand::SendFile { file_path, peer_id, broadcast } => {
                        peer.send_file(file_path, peer_id, broadcast).await;
                    }
                    UserCommand::Quit => {
                        println!("{}", "Shutting down peer...".yellow());
                        peer.room.leave(&mut peer.swarm.behaviour_mut().gossipsub).ok();
                        break;
                    }
                }
            }

            event = peer.swarm.select_next_some() => {
                match event {
                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    })) => {
                        if message.topic == peer.room.topic().hash() && propagation_source != *peer.local_peer_id() {
                            if let Ok(msg) = String::from_utf8(message.data.clone()) {
                                if msg.starts_with("JOIN:") {
                                    let name = msg.strip_prefix("JOIN:").unwrap_or("Unknown");
                                    if !peer.connected_peers.contains_key(&propagation_source) {
                                        // Ensure we track peers even if we only see their gossipsub join first.
                                        peer.handle_new_peer(propagation_source, Vec::new());
                                    }
                                    if let Some(info) = peer.connected_peers.get_mut(&propagation_source) {
                                        if info.name.is_none() {
                                            info.name = Some(name.to_string());
                                            println!(
                                                "{} {} {}",
                                                "→".bright_green(),
                                                name.bright_yellow(),
                                                "joined the room".bright_black()
                                            );
                                        }
                                    }
                                    let _ = io::stdout().flush();
                                }
                            }
                        }
                    }

                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                        for (peer_id, addr) in list {
                            peer.swarm.dial(addr.clone()).ok();
                            peer.handle_new_peer(peer_id, vec![addr]);
                        }
                    }

                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Identify(identify::Event::Received {
                        peer_id,
                        info,
                    })) => {
                        peer.handle_new_peer(peer_id, info.listen_addrs.clone());

                        for addr in &info.listen_addrs {
                            peer.swarm.add_external_address(addr.clone());
                        }

                        if info.protocols.iter().any(|p| p.as_ref() == "/libp2p/circuit/relay/0.2.0/hop") {
                            peer.relay_peers.insert(peer_id);
                            println!(
                                "{} {}",
                                "Discovered relay peer:".bright_cyan(),
                                peer_id.to_string().bright_black()
                            );
                        }
                    }

                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Dcutr(dcutr::Event {
                        remote_peer_id,
                        result: Ok(_connection_id),
                    })) => {
                        peer.update_connection_type(remote_peer_id, ConnectionType::HolePunched);
                    }

                    SwarmEvent::Behaviour(FileMeshBehaviourEvent::Autonat(autonat::Event::StatusChanged {
                        old,
                        new,
                    })) => {
                        println!(
                            "{} {:?} → {:?}",
                            "NAT status changed:".bright_cyan(),
                            old,
                            new
                        );
                    }

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
                                    "Received file:".bright_green(),
                                    file_name.bright_white(),
                                    format!("from {}", peer_name).bright_black(),
                                    format!("{} bytes", size).bright_black()
                                );

                            }
                            FileTransferEvent::FileSent { to, file_name } => {
                                println!(
                                    "{} {} {} {}",
                                    "✓".bright_green(),
                                    "Sent file:".bright_green(),
                                    file_name.bright_white(),
                                    format!("to {}", to.to_string()).bright_black()
                                );
                            }
                            FileTransferEvent::Error { error } => {
                                println!(
                                    "{} {} {}",
                                    "✗".red(),
                                    "File transfer error:".red(),
                                    error
                                );
                            }
                        }
                    }

                    SwarmEvent::NewListenAddr { address, .. } => {
                        println!(
                            "{} {}",
                            "Listening on:".bright_cyan(),
                            address.to_string().bright_black()
                        );
                    }

                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        let addrs = vec![endpoint.get_remote_address().clone()];
                        peer.handle_new_peer(peer_id, addrs);
                        peer.broadcast_join();
                    }

                    SwarmEvent::ConnectionClosed { peer_id, cause: _, .. } => {
                        if let Some(info) = peer.connected_peers.remove(&peer_id) {
                            let name = info.name.as_deref().unwrap_or("Unknown");
                            println!(
                                "{} {} {}",
                                "←".bright_red(),
                                name.bright_yellow(),
                                "left the room".bright_black()
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
