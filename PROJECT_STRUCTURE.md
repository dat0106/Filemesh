# FileMesh-RS Project Structure

## Overview

```
filemesh-rs/
├── Cargo.toml              # Project dependencies and metadata
├── .gitignore              # Git ignore rules
├── README.md               # Comprehensive documentation
├── QUICKSTART.md           # Quick start guide
├── EXAMPLES.md             # Usage examples and scenarios
├── PROJECT_STRUCTURE.md    # This file
├── example.txt             # Test file for demos
├── src/
│   ├── main.rs             # CLI interface and command handling
│   ├── libp2p_peer.rs      # libp2p node setup and event handling
│   ├── file_transfer.rs    # Custom file transfer protocol
│   └── room.rs             # Gossipsub room management
└── received_files/         # Auto-created directory for received files
```

## Module Breakdown

### main.rs (348 lines)

**Purpose**: Entry point, CLI parsing, and user interaction

**Key Components**:
- `Cli` struct with clap for argument parsing
- `UserCommand` enum for internal commands
- `start_peer()` - Initializes peer and spawns async tasks
- `handle_user_input()` - Interactive command loop
- `parse_send_command()` - Parses send file commands

**Dependencies**:
- clap for CLI parsing
- tokio for async runtime
- colored for terminal output

**Flow**:
1. Parse command-line arguments
2. Start peer with name and room
3. Spawn async task for peer networking
4. Handle user input in separate task
5. Send commands via mpsc channel

---

### libp2p_peer.rs (424 lines)

**Purpose**: Core libp2p networking, swarm management, and event handling

**Key Components**:

#### Structs
- `FileMeshBehaviour` - NetworkBehaviour combining all protocols
- `PeerInfo` - Stores peer metadata and connection type
- `FileMeshPeer` - Main peer structure with swarm and state

#### Enums
- `ConnectionType` - Direct, Relayed, HolePunched, Unknown

#### Functions
- `FileMeshPeer::new()` - Creates and configures libp2p swarm
- `FileMeshPeer::start()` - Starts listening and joins room
- `FileMeshPeer::list_peers()` - Displays connected peers
- `FileMeshPeer::send_file()` - Initiates file transfer
- `run_peer()` - Main event loop processing swarm events

**Protocols Integrated**:
- **Gossipsub**: Room messaging and peer discovery
- **mDNS**: Local network peer discovery
- **Identify**: Peer information exchange
- **Ping**: Connection keep-alive
- **Relay Client**: Relayed connection support
- **DCUtR**: Hole-punching for NAT traversal
- **AutoNAT**: NAT status detection
- **FileTransfer**: Custom file transfer protocol

**Event Handling**:
- Gossipsub messages (JOIN announcements)
- mDNS discoveries
- Identify responses
- DCUtR upgrades
- AutoNAT status changes
- File transfer events
- Connection establishment/closure

---

### file_transfer.rs (402 lines)

**Purpose**: Custom libp2p protocol for streaming file transfers

**Key Components**:

#### Behaviour
- `FileTransferBehaviour` - NetworkBehaviour implementation
- `FileTransferHandler` - ConnectionHandler for streams

#### Events
- `FileTransferEvent::FileReceived` - File successfully received
- `FileTransferEvent::FileSent` - File successfully sent
- `FileTransferEvent::Error` - Transfer error

#### Protocol
- Protocol ID: `/filemesh/1.0.0`
- Chunk size: 64KB for streaming

#### Transfer Flow
1. **Handshake**: Open stream between peers
2. **Metadata**:
   - Send filename length (4 bytes, big-endian)
   - Send filename (UTF-8 bytes)
   - Send file size (8 bytes, big-endian)
3. **Streaming**:
   - Transfer in 64KB chunks
   - Show progress every 10 chunks
4. **Completion**:
   - Flush and close stream
   - Emit event

#### Functions
- `send_file()` - Async function to send file over stream
- `receive_file()` - Async function to receive file from stream

**Features**:
- Progress indicators
- Error handling
- Automatic directory creation
- Original filename preservation

---

### room.rs (42 lines)

**Purpose**: Gossipsub topic management for rooms

**Key Components**:

#### Struct
- `Room` - Represents a chat room using gossipsub topics

#### Methods
- `new()` - Create room with name and peer ID
- `name()` - Get room name
- `topic()` - Get gossipsub topic
- `join()` - Subscribe to room topic
- `broadcast()` - Publish message to room
- `leave()` - Unsubscribe from room topic

**Design**:
- Room name is embedded in gossipsub topic: `filemesh-room-{name}`
- Peers in same room subscribe to same topic
- Only subscribers receive room messages
- Enables isolation between different groups

---

## Data Flow

### Starting a Peer

```
User runs CLI
    ↓
main.rs parses args
    ↓
start_peer() called
    ↓
FileMeshPeer::new() creates swarm
    ↓
Configure transports (TCP + Noise + Yamux)
    ↓
Initialize all protocols
    ↓
FileMeshPeer::start() begins listening
    ↓
Join gossipsub room
    ↓
Event loop starts (run_peer)
```

### Peer Discovery

```
Peer A starts
    ↓
Listens on local addresses
    ↓
mDNS announces presence (local)
Gossipsub subscribes to room (global)
    ↓
Peer B starts
    ↓
mDNS discovers Peer A
    ↓
Peer B dials Peer A
    ↓
Connection established
    ↓
Identify exchange
    ↓
Both peers update connection type
    ↓
Peer A broadcasts JOIN message
Peer B broadcasts JOIN message
```

### File Transfer

```
User: send --file test.txt --to <PEER_ID>
    ↓
parse_send_command() validates
    ↓
UserCommand::SendFile sent via channel
    ↓
run_peer() receives command
    ↓
FileMeshPeer::send_file() called
    ↓
FileTransferBehaviour queues request
    ↓
Handler opens outbound stream
    ↓
send_file() async task:
  - Read file metadata
  - Send filename length
  - Send filename
  - Send file size
  - Stream file in 64KB chunks
  - Show progress
  - Close stream
    ↓
Receiver's Handler gets inbound stream
    ↓
receive_file() async task:
  - Read filename length
  - Read filename
  - Read file size
  - Create received_files/ directory
  - Stream chunks to file
  - Show progress
  - Close and flush file
    ↓
FileTransferEvent::FileSent on sender
FileTransferEvent::FileReceived on receiver
    ↓
Events displayed to users
```

### NAT Traversal

```
Peer A (behind NAT)
    ↓
AutoNAT detects Private status
    ↓
Discovers relay peer via Identify
    ↓
Peer B (behind different NAT)
    ↓
Also discovers relay peer
    ↓
Connection attempt: A → B
    ↓
Direct connection fails
    ↓
Falls back to relay
    ↓
Connection established (Relayed)
    ↓
DCUtR attempts hole-punching
    ↓
If successful: Upgrade to Hole-Punched
If failed: Stay on Relayed
```

---

## Protocol Stack

```
┌─────────────────────────────────────────┐
│          Application Layer              │
│  (CLI commands, file operations)        │
└─────────────────────────────────────────┘
                  ↓
┌─────────────────────────────────────────┐
│     Custom Protocol Layer               │
│  /filemesh/1.0.0 (file transfer)        │
└─────────────────────────────────────────┘
                  ↓
┌─────────────────────────────────────────┐
│       libp2p Protocol Layer             │
│  Gossipsub, mDNS, Identify, etc.        │
└─────────────────────────────────────────┘
                  ↓
┌─────────────────────────────────────────┐
│      Multiplexing Layer                 │
│         Yamux (stream muxing)           │
└─────────────────────────────────────────┘
                  ↓
┌─────────────────────────────────────────┐
│       Encryption Layer                  │
│      Noise Protocol (encryption)        │
└─────────────────────────────────────────┘
                  ↓
┌─────────────────────────────────────────┐
│       Transport Layer                   │
│            TCP/WebSocket                │
└─────────────────────────────────────────┘
```

---

## Key Design Decisions

### 1. Async/Await with Tokio
- Non-blocking I/O for multiple concurrent connections
- Efficient handling of multiple file transfers
- Responsive CLI during network operations

### 2. Custom Protocol vs Built-in
- Implemented custom `/filemesh/1.0.0` for flexibility
- Full control over file transfer semantics
- Streaming support with progress tracking

### 3. Gossipsub for Rooms
- Decentralized room management
- No central server required
- Scalable to many peers
- Message propagation for JOIN/LEAVE events

### 4. Connection Handler Pattern
- Separates protocol negotiation from business logic
- Async tasks for file operations
- Clean separation of concerns

### 5. MPSC Channel for Commands
- Decouples user input from network events
- Thread-safe command passing
- Non-blocking user interaction

### 6. Progress Indicators
- Real-time feedback during transfers
- Chunk-based progress calculation
- User-friendly terminal output

---

## Extension Points

### Adding New Commands
1. Add variant to `UserCommand` enum in main.rs
2. Add parsing logic in `handle_user_input()`
3. Handle command in `run_peer()` event loop

### Adding New Protocols
1. Add dependency to Cargo.toml
2. Add field to `FileMeshBehaviour` struct
3. Handle events in `run_peer()` match statement

### Custom File Protocols
1. Modify `send_file()` and `receive_file()` in file_transfer.rs
2. Update metadata format
3. Add compression, encryption, or checksums

### Different Transport Layers
1. Add transport in `FileMeshPeer::new()`
2. Configure in transport builder chain
3. Update listen addresses in `start()`

---

## Testing Strategy

### Unit Tests
- Protocol message parsing
- Command parsing logic
- Room topic generation

### Integration Tests
- Two peers on same machine
- File transfer completion
- Connection type detection

### Manual Testing
- Same network (Direct)
- Different networks (Relayed)
- Hole-punching scenarios
- Multiple peers broadcasting
- Large file transfers

---

## Performance Characteristics

### Memory Usage
- ~50MB base (libp2p runtime)
- +64KB per active transfer (chunk buffer)
- Scales with number of connections

### CPU Usage
- Low during idle (event-driven)
- Moderate during file transfer (encryption)
- Network I/O is non-blocking

### Network Usage
- 64KB chunks for optimal throughput
- Progress updates don't block transfer
- Gossipsub heartbeats every 1 second

---

## Security Considerations

### Encryption
- All connections use Noise protocol
- End-to-end encryption for file transfers
- No plaintext data over network

### Authentication
- Peer IDs derived from public keys
- Cryptographic verification of identity
- No spoofing possible

### Isolation
- Rooms are logically isolated
- No cross-room communication
- Gossipsub topic-based separation

### File Safety
- Files saved to dedicated directory
- Original filenames preserved
- No arbitrary file writes

---

## Future Enhancements

### High Priority
- [ ] Implement file checksums (SHA-256)
- [ ] Add transfer resume capability
- [ ] Implement file compression (optional)

### Medium Priority
- [ ] Add end-to-end file encryption
- [ ] Implement transfer rate limiting
- [ ] Add file metadata (timestamps, permissions)

### Low Priority
- [ ] Web UI for browser-based usage
- [ ] DHT for peer discovery without relay
- [ ] IPv6 support
- [ ] File preview before accepting

---

## Dependencies

### Core Dependencies
- **tokio**: Async runtime and I/O
- **libp2p**: All networking protocols
- **clap**: CLI argument parsing
- **anyhow**: Error handling

### Utility Dependencies
- **colored**: Terminal colors
- **indicatif**: Progress bars
- **serde**: Serialization (future use)
- **bytes**: Byte buffer manipulation

---

## Build Configuration

### Debug Build
```bash
cargo build
```
- Fast compilation
- Debug symbols included
- No optimizations
- Good for development

### Release Build
```bash
cargo build --release
```
- Slower compilation
- Optimized binary
- LTO enabled
- Production-ready

---

## Contributing Guide

### Adding Features
1. Create feature branch
2. Implement in appropriate module
3. Add tests if applicable
4. Update documentation
5. Submit pull request

### Code Style
- Follow Rust conventions
- Use rustfmt for formatting
- Run clippy for lints
- Document public APIs

### Commit Messages
- Use conventional commits
- Reference issue numbers
- Keep commits focused

---

This structure document should help developers understand the codebase and contribute effectively!
