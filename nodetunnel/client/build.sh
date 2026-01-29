#!/bin/bash
cargo build --release
cp target/release/libnodetunnel.so ../../addons/nodetunnel/bin/