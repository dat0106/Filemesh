# FileMesh-RS Examples

## Example 1: Two Peers on Same Network

This demonstrates **direct connections** (best performance).

### Terminal 1 - Alice
```bash
$ cargo run -- start --name Alice --room study-group

╔══════════════════════════════════════════════════╗
║           FileMesh-RS - P2P File Transfer       ║
╚══════════════════════════════════════════════════╝

Starting peer: Alice (12D3KooWPjc...)
Joined room: study-group

Listening on: /ip4/127.0.0.1/tcp/52847
Listening on: /ip4/192.168.1.100/tcp/52847

→ Bob joined the room
New peer connected: 12D3KooWQXy... (Direct)

> send --file notes.pdf --to 12D3KooWQXy...
Sending file: notes.pdf to 12D3KooWQXy...
Sending... 100%
✓ Sent file: notes.pdf to 12D3KooWQXy...
```

### Terminal 2 - Bob
```bash
$ cargo run -- start --name Bob --room study-group

Starting peer: Bob (12D3KooWQXy...)
Joined room: study-group

→ Alice joined the room
New peer connected: 12D3KooWPjc... (Direct)

Receiving... 100%
✓ Received file: notes.pdf from Alice (2458624 bytes)
```

---

## Example 2: Broadcasting to Multiple Peers

### Terminal 1 - Alice
```bash
$ cargo run -- start --name Alice --room team

> peers

╔════════════════════════════════════════════╗
║           Connected Peers                  ║
╚════════════════════════════════════════════╝

  1. Bob (12D3KooWQXy...)
     Connection Type: Direct

  2. Charlie (12D3KooWAbc...)
     Connection Type: Direct

> send --file presentation.pptx --broadcast
Broadcasting file: presentation.pptx
Sending... 100%
✓ Sent file: presentation.pptx to 12D3KooWQXy...
Sending... 100%
✓ Sent file: presentation.pptx to 12D3KooWAbc...
```

### Terminal 2 - Bob
```bash
$ cargo run -- start --name Bob --room team

→ Alice joined the room
→ Charlie joined the room

Receiving... 100%
✓ Received file: presentation.pptx from Alice (5242880 bytes)
```

### Terminal 3 - Charlie
```bash
$ cargo run -- start --name Charlie --room team

→ Alice joined the room
→ Bob joined the room

Receiving... 100%
✓ Received file: presentation.pptx from Alice (5242880 bytes)
```

---

## Example 3: NAT Traversal with Relay

This demonstrates **relayed connections** when direct connection is not possible.

### Terminal 1 - Alice (Behind NAT)
```bash
$ cargo run -- start --name Alice --room remote-work

Starting peer: Alice (12D3KooWPjc...)
NAT status changed: Unknown → Private
Discovered relay peer: 12D3KooWRel...

→ Bob joined the room
New peer connected: 12D3KooWQXy... (Relayed)

> send --file report.docx --to 12D3KooWQXy...
Sending file: report.docx to 12D3KooWQXy...
Sending... 100%
✓ Sent file: report.docx to 12D3KooWQXy...
```

### Terminal 2 - Bob (Behind Different NAT)
```bash
$ cargo run -- start --name Bob --room remote-work

NAT status changed: Unknown → Private
Discovered relay peer: 12D3KooWRel...

→ Alice joined the room
New peer connected: 12D3KooWPjc... (Relayed)

Receiving... 100%
✓ Received file: report.docx from Alice (1048576 bytes)
```

---

## Example 4: Hole-Punching (DCUtR)

This demonstrates connection upgrade from **relayed** to **hole-punched**.

### Terminal 1 - Alice
```bash
$ cargo run -- start --name Alice --room gaming

→ Bob joined the room
New peer connected: 12D3KooWQXy... (Relayed)
Connection upgraded: 12D3KooWQXy... Hole-Punched

> peers

╔════════════════════════════════════════════╗
║           Connected Peers                  ║
╚════════════════════════════════════════════╝

  1. Bob (12D3KooWQXy...)
     Connection Type: Hole-Punched

> send --file game-save.dat --to 12D3KooWQXy...
```

---

## Example 5: Large File Transfer with Progress

### Sending a Large File
```bash
> send --file movie.mp4 --to 12D3KooWQXy...
Sending file: movie.mp4 to 12D3KooWQXy...
Sending... 23%
```

The progress indicator updates in real-time as the file streams.

---

## Example 6: Multiple Rooms

Peers in different rooms cannot see or communicate with each other.

### Terminal 1 - Room A
```bash
$ cargo run -- start --name Alice --room room-a

> peers
  No peers connected yet.
```

### Terminal 2 - Room B
```bash
$ cargo run -- start --name Bob --room room-b

> peers
  No peers connected yet.
```

### Terminal 3 - Room A
```bash
$ cargo run -- start --name Charlie --room room-a

→ Alice joined the room
New peer connected: 12D3KooWPjc... (Direct)
```

Charlie connects to Alice (both in room-a), but not to Bob (in room-b).

---

## Command Examples

### List Peers
```bash
> peers
```

### Send to Specific Peer
```bash
> send --file document.pdf --to 12D3KooWPjceQrSwdWXPyLLeABRXmuqt69Bm4YPz
```

### Broadcast to All
```bash
> send --file image.png --broadcast
```

### Get Help
```bash
> help
```

### Exit
```bash
> quit
```

---

## Connection Type Summary

| Type | Symbol | Description | Use Case |
|------|--------|-------------|----------|
| Direct | 🟢 | Peer-to-peer connection | Same network or public IPs |
| Relayed | 🟡 | Through relay server | Behind NAT/firewall |
| Hole-Punched | 🔵 | Upgraded relayed connection | NAT traversal success |

---

## File Naming Tips

Files are saved with their original names in `received_files/`:
- `document.pdf` → `received_files/document.pdf`
- `photo.jpg` → `received_files/photo.jpg`
- `archive.zip` → `received_files/archive.zip`

---

## Performance Notes

- **Direct connections**: Lowest latency, highest throughput
- **Hole-punched connections**: Good latency, good throughput
- **Relayed connections**: Higher latency due to relay hop

Chunk size is 64KB for optimal streaming performance across all connection types.

---

## Testing Scenarios

### Local Network Testing
1. Connect both peers to same WiFi
2. Expect: Direct connections
3. Best for: Development and testing

### Internet Testing
1. Connect peers on different networks
2. Expect: Relayed or Hole-Punched
3. Best for: Real-world scenarios

### Multi-Peer Testing
1. Start 3+ peers in same room
2. Test broadcast functionality
3. Observe different connection types

---

## Troubleshooting Examples

### Peer Not Found
```bash
> send --file test.txt --to 12D3KooWInvalid...
Peer not connected.
```
**Solution**: Use `peers` to get correct peer ID

### File Not Found
```bash
> send --file nonexistent.txt --to 12D3KooW...
✗ File transfer error: File not found: nonexistent.txt
```
**Solution**: Check file path and permissions

### No Peers Available
```bash
> send --file test.txt --broadcast
No peers to broadcast to.
```
**Solution**: Wait for peers to join or check room name
