#!/bin/bash
cd src
rm -rf ../addons/nodetunnel/bin/*
cargo build --target aarch64-apple-darwin
cp target/aarch64-apple-darwin/debug/libnodetunnel.dylib ../addons/nodetunnel/bin/ 
cp -r ../addons/nodetunnel ~/code/godot/node-tunnel-web/Addons
