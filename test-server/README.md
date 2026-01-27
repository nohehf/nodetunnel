# NodeTunnel Test Server

A minimal WebRTC relay test server for testing WebRTC connections with NodeTunnel.

## Features

- Accepts WebRTC offers via HTTP POST
- Creates WebRTC answers and returns them
- Establishes WebRTC data channel connections
- Monitors connection state

## Usage

### Build

```bash
cd test-server
cargo build --release
```

### Run

```bash
cargo run
```

The server will listen on `http://0.0.0.0:5344`.

**Note**: When connecting from Godot, use `http://127.0.0.1:5344/signaling` (not `localhost`) as Godot's HTTP client cannot resolve `localhost`.

### Endpoint

- **POST** `/signaling`
  - Accepts: `{ "type": "offer", "sdp": "..." }`
  - Returns: `{ "type": "answer", "sdp": "..." }`

## Testing with Godot

In your Godot project, connect to the test server:

```gdscript
var peer: WebNodeTunnelPeer

func _ready():
    peer = WebNodeTunnelPeer.new()
    # Connect to the test server
    # NOTE: Use 127.0.0.1 instead of localhost - Godot's HTTP client has DNS issues with localhost
    peer.connect_to_relay("http://127.0.0.1:5344/signaling", "test_app_id")
    multiplayer.multiplayer_peer = peer
```

**Important**: Use `127.0.0.1` instead of `localhost` - Godot's HTTP client cannot resolve `localhost` and will fail with `CANT_RESOLVE` status.

## Notes

- This is a minimal test server that only handles WebRTC connection establishment
- It does not implement the full relay protocol yet
- The server keeps connections alive until they close naturally
