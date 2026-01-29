@echo off
cargo build --release
copy target\release\nodetunnel.dll ..\..\addons\nodetunnel\bin\