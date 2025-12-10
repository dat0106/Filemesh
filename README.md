# FileMesh-RS

A peer-to-peer file transfer CLI application built with Rust and libp2p, featuring NAT traversal, relay fallback, and universal connectivity.

## Features

- **Peer-to-peer file transfer** using libp2p custom protocol `/filemesh/1.0.0`
- **NAT traversal detection** with AutoNAT
- **Relay fallback** for connectivity in restrictive network environments
- **DCUtR (Direct Connection Upgrade through Relay)** for hole-punching
- **Gossipsub** for peer discovery and room management
- **mDNS** for local network peer discovery
- **Streaming file transfer** with progress indicators
- **Connection type tracking**: Direct, Relayed, or Hole-Punched
- **Broadcast or targeted file transfer**

## Architecture

```
src/
├── main.rs              # CLI interface and command handling
├── libp2p_peer.rs       # libp2p node setup, swarm management, and event handling
├── file_transfer.rs     # Custom file transfer protocol implementation
└── room.rs              # Gossipsub room management
```

## Installation

### Prerequisites

- Rust 1.70+ (install from https://rustup.rs)

### Build

```bash
cargo build --release
```

The binary will be available at `target/release/filemesh-rs`

## Usage

### Starting a Peer

Open a terminal and start the first peer:

```bash
cargo run -- start --name Alice --room myroom
```

Open another terminal and start a second peer:

```bash
cargo run -- start --name Bob --room myroom
```

### Commands

Once a peer is running, you can use these commands:

#### List Connected Peers

```bash
> peers
```

This displays all connected peers with their:
- Peer name
- Peer ID
- Connection type (Direct, Relayed, or Hole-Punched)
- Network addresses

#### Send File to Specific Peer

```bash
> send --file document.pdf --to 12D3KooWAbc123...
```

Replace `12D3KooWAbc123...` with the actual peer ID from the `peers` command.

#### Broadcast File to All Peers

```bash
> send --file photo.jpg --broadcast
```

This sends the file to all connected peers in the room.

#### Get Help

```bash
> help
```

#### Exit

```bash
> quit
```

## Demo Scenario

### Terminal 1: Alice

```bash
$ cargo run -- start --name Alice --room demo

╔══════════════════════════════════════════════════╗
║           FileMesh-RS - P2P File Transfer       ║
╚══════════════════════════════════════════════════╝

Starting peer: Alice (12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Bm4YPz...)
Joined room: demo

Listening on: /ip4/127.0.0.1/tcp/52847
Listening on: /ip4/192.168.1.100/tcp/52847

Available commands:
  peers - List all connected peers with connection types
  send --file <FILE> --to <PEER_ID> - Send file to specific peer
  send --file <FILE> --broadcast - Broadcast file to all peers
  quit - Exit the application

→ Bob joined the room
New peer connected: 12D3KooWQXyzAbcd... (Direct)

> peers

╔════════════════════════════════════════════╗
║           Connected Peers                  ║
╚════════════════════════════════════════════╝

  1. Bob (12D3KooWQXyzAbcd...)
     Connection Type: Direct
     Addresses: /ip4/192.168.1.101/tcp/52848

> send --file example.txt --to 12D3KooWQXyzAbcd...
Sending file: example.txt to 12D3KooWQXyzAbcd...
Sending... 100%
✓ Sent file: example.txt to 12D3KooWQXyzAbcd...
```

### Terminal 2: Bob

```bash
$ cargo run -- start --name Bob --room demo

╔══════════════════════════════════════════════════╗
║           FileMesh-RS - P2P File Transfer       ║
╚══════════════════════════════════════════════════╝

Starting peer: Bob (12D3KooWQXyzAbcd...)
Joined room: demo

Listening on: /ip4/127.0.0.1/tcp/52848
Listening on: /ip4/192.168.1.101/tcp/52848

Available commands:
  peers - List all connected peers with connection types
  send --file <FILE> --to <PEER_ID> - Send file to specific peer
  send --file <FILE> --broadcast - Broadcast file to all peers
  quit - Exit the application

→ Alice joined the room
New peer connected: 12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Bm4YPz... (Direct)

Receiving... 100%
✓ Received file: example.txt from Alice (1024 bytes)

> peers

╔════════════════════════════════════════════╗
║           Connected Peers                  ║
╚════════════════════════════════════════════╝

  1. Alice (12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Bm4YPz...)
     Connection Type: Direct
     Addresses: /ip4/192.168.1.100/tcp/52847
```

## Universal Connectivity

FileMesh-RS demonstrates three types of connections:

### 1. Direct Connection
When peers are on the same network or have public IPs, they establish direct connections.

**Log Example:**
```
New peer connected: 12D3KooW... (Direct)
```

### 2. Relayed Connection
When direct connection fails due to NAT, peers communicate through a relay server.

**Log Example:**
```
Discovered relay peer: 12D3KooW...
New peer connected: 12D3KooW... (Relayed)
```

### 3. Hole-Punched Connection
DCUtR upgrades relayed connections to direct connections when possible.

**Log Example:**
```
New peer connected: 12D3KooW... (Relayed)
Connection upgraded: 12D3KooW... Hole-Punched
```

## File Transfer Protocol

The custom protocol `/filemesh/1.0.0` works as follows:

1. **Handshake**: Sender opens a stream to receiver
2. **Metadata**: Send file name length (4 bytes), file name, and file size (8 bytes)
3. **Streaming**: Transfer file in 64KB chunks with progress tracking
4. **Completion**: Close stream and confirm transfer

Received files are automatically saved to the `received_files/` directory.

## Network Protocols Used

- **Transport**: TCP with Noise encryption and Yamux multiplexing
- **Peer Discovery**: mDNS (local) and Gossipsub (global)
- **Identity**: Identify protocol for peer information exchange
- **NAT**: AutoNAT for NAT status detection
- **Relay**: Circuit Relay v2 for relayed connections
- **DCUtR**: Direct Connection Upgrade through Relay for hole-punching
- **Ping**: Keep-alive and connectivity monitoring

## Testing on Different Networks

### Same Local Network (Direct)
Run both peers on the same WiFi network. They should discover each other via mDNS and establish a direct connection.

### Different Networks (Relayed/Hole-Punched)
To test NAT traversal:
1. Run one peer on your local network
2. Run another peer on a different network (e.g., mobile hotspot, VPS, or different WiFi)
3. Use a public relay server or run your own relay node
4. Peers will connect via relay, then attempt hole-punching

## Configuration

### Custom Ports

The application automatically selects available ports. To listen on specific ports, modify the `start()` function in `libp2p_peer.rs`:

```rust
self.swarm.listen_on("/ip4/0.0.0.0/tcp/YOUR_PORT".parse()?)?;
```

### Relay Servers

The code currently relies on discovered relay peers. To use specific relay servers, add them in `libp2p_peer.rs`:

```rust
let relay_addr: Multiaddr = "/ip4/RELAY_IP/tcp/PORT/p2p/RELAY_PEER_ID".parse()?;
self.swarm.dial(relay_addr)?;
```

## Performance

- **Chunk Size**: 64KB for optimal streaming performance
- **Connection Timeout**: 60 seconds for idle connections
- **Heartbeat**: 1 second for gossipsub message propagation
- **AutoNAT**: Status refresh every 30 seconds

## Troubleshooting

### Peers Not Discovering Each Other

- Ensure both peers are in the same room
- Check firewall settings
- Verify network connectivity
- Try on the same local network first (mDNS should work)

### File Transfer Fails

- Verify the file path is correct
- Check file permissions
- Ensure sufficient disk space in `received_files/` directory
- Confirm peer is still connected with `peers` command

### Connection Always Relayed

- This is normal on restrictive NATs
- Hole-punching attempts automatically via DCUtR
- Direct connections work best on permissive networks

## Development

### Running Tests

```bash
cargo test
```

### Debug Logging

For detailed libp2p logs, set the `RUST_LOG` environment variable:

```bash
RUST_LOG=debug cargo run -- start --name Alice --room myroom
```

### Code Structure

- **main.rs**: CLI parsing and user command handling
- **libp2p_peer.rs**: Core libp2p functionality, swarm events, connection management
- **file_transfer.rs**: Custom NetworkBehaviour for file transfer protocol
- **room.rs**: Gossipsub topic management for rooms

## Security Considerations

- All connections use Noise protocol for encryption
- Peer authentication via cryptographic identities
- No central server required
- Files are transferred directly between peers

## Future Enhancements

- File encryption before transfer
- Resume capability for interrupted transfers
- Compression for text files
- Web UI for easier interaction
- DHT for persistent peer discovery
- IPv6 support

## License

MIT License

## Contributing

Contributions welcome! Please open issues or pull requests on the repository.

## Credits

Built with:
- [libp2p](https://libp2p.io/) - Peer-to-peer networking library
- [tokio](https://tokio.rs/) - Async runtime
- [clap](https://github.com/clap-rs/clap) - CLI argument parsing
- [colored](https://github.com/mackwic/colored) - Terminal colors
