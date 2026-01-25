mod node_tunnel_peer;
pub mod protocol;
mod relay_client;
mod transport;
mod web_node_tunnel_peer;

use godot::prelude::*;

struct NodeTunnel;

#[gdextension]
unsafe impl ExtensionLibrary for NodeTunnel {}
