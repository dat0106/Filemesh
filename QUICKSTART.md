# FileMesh-RS Quick Start Guide

Get up and running with FileMesh-RS in 5 minutes!

## Step 1: Build the Project

```bash
cargo build --release
```

This may take a few minutes the first time as it downloads and compiles dependencies.

## Step 2: Start First Peer (Alice)

Open a new terminal window:

```bash
cargo run -- start --name Alice --room demo
```

You'll see:
- Your peer ID
- Listening addresses
- Available commands

## Step 3: Start Second Peer (Bob)

Open another terminal window:

```bash
cargo run -- start --name Bob --room demo
```

Both peers should automatically discover each other and connect!

## Step 4: Check Connected Peers

In either terminal, type:

```bash
> peers
```

You should see the other peer listed with their connection type (Direct, Relayed, or Hole-Punched).

## Step 5: Send a Test File

In Alice's terminal:

```bash
> send --file example.txt --broadcast
```

In Bob's terminal, you'll see the file being received and saved to `received_files/example.txt`!

## Common Commands

**List peers:**
```bash
> peers
```

**Send to specific peer:**
```bash
> send --file myfile.pdf --to <PEER_ID>
```
(Get PEER_ID from the `peers` command)

**Broadcast to all:**
```bash
> send --file photo.jpg --broadcast
```

**Get help:**
```bash
> help
```

**Exit:**
```bash
> quit
```

## Connection Types Explained

- **Direct** 🟢 - Best performance, peers connect directly
- **Relayed** 🟡 - Connection through relay server (NAT traversal)
- **Hole-Punched** 🔵 - Upgraded from relayed to direct using DCUtR

## Troubleshooting

**Peers not connecting?**
- Make sure both are in the same room name
- Try on the same WiFi network first
- Check firewall settings

**File not sending?**
- Verify file path is correct
- Check the peer is still connected with `peers`
- Make sure you have permission to read the file

## Next Steps

- Try with 3+ peers in the same room
- Test on different networks to see relay and hole-punching in action
- Send larger files to see streaming in action
- Check the full README.md for advanced configuration

Happy file sharing! 🚀
