@echo off
chcp 65001 > nul

set MINDFORK_ENGINE_URL=http://192.168.1.20:8000/v1
set MINDFORK_EMBED_URL=http://192.168.1.20:8001/v1

cargo test -- --include-ignored