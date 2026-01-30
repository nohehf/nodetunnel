#!/bin/bash
# Build only the client package to avoid mixing with server targets
# Use local target directory instead of workspace root
cargo build -p nodetunnel --target-dir target

# Copy artifacts for each architecture
# Targets are built in local target directory
# This handles both architecture-specific builds (target/<arch>/debug/) 
# and default builds (target/debug/)
# Only copy libnodetunnel.* (client library), not relay-server (server binary)
for lib_file in target/*/debug/libnodetunnel.* target/debug/libnodetunnel.*; do
    if [ -f "$lib_file" ]; then
        cp "$lib_file" ../../addons/nodetunnel/bin/
    fi
done
