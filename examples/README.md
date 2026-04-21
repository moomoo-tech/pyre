# Pyronova Examples

Three production-grade demo applications.

## Setup

```bash
# Build Pyronova first
cd ..
maturin develop --release

# Install demo dependencies
pip install pydantic numpy msgpack httpx
```

## Run

```bash
# AI Agent Server — MCP tools, SSE streaming, session memory
python ai_agent_server.py

# Trading Data API — numpy analytics, WebSocket, Pydantic, MsgPack RPC
python trading_api.py

# Full-stack REST API — CRUD, cookie auth, file upload
python fullstack_api.py
```

All demos start on `http://127.0.0.1:8000`. See each file's docstring for curl examples.

## What each demo shows

| Demo | C Extensions | Sub-interp routes | GIL routes | Features |
|------|-------------|-------------------|-----------|----------|
| **AI Agent** | pydantic | `/` index | `/chat`, `/stream`, `/memory` | MCP, SSE, SharedState, async |
| **Trading** | numpy, pydantic | `/` index | `/market`, `/order`, `/analytics` | WebSocket, RPC, SharedState |
| **Full-stack** | pydantic | `/items` (list) | `/auth/*`, `/items` (CRUD) | Cookie, upload, redirect |

Routes using C extensions (pydantic, numpy) are automatically dispatched to the main interpreter via `gil=True`. Fast routes run on sub-interpreters at 220k req/s.
