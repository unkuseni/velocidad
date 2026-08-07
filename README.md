# ⚡ Velocidad

A **Trojan-style Telegram trading bot** built in Rust — token sniping, instant
buy/sell, limit orders, security scans, price alerts, wallets and portfolio
tracking — with **libSQL / Turso** as the database instead of Postgres.

```
Telegram Bot (teloxide) ─┐
                         ├─▶ Trading Engine ─▶ libSQL (local file or Turso cloud)
HTTP API (Axum) ────────┘        │
                                 └─▶ Positions · Orders · Alerts · Wallets · Tokens
```

## What works today

| Feature | How |
| --- | --- |
| 💰 Wallet management | Generate/import EVM wallets; private keys encrypted at rest with AES-256-GCM under a master key (`k256` + `sha3` + `aes-gcm`) |
| 🟢🔴 Instant buy/sell | Filled instantly against a deterministic mock market (**paper trading**) |
| 🛰️ Token sniping | Same fast path, with an automated honeypot/risk gate |
| 🔬 Security scans | Honeypot / ownership / tax / liquidity heuristics → risk score, cached in DB |
| ⏳ Limit orders | Recorded as pending orders (real matching engine is a follow-up) |
| 🔔 Price alerts | `above`/`below` triggers checked on every fill |
| 📂 Portfolio & PnL | Weighted-average positions, realized + unrealized PnL |
| 🧾 Order history | Full trade log per user |
| 🌐 Web API | Axum REST API mirroring the bot (see below) |
| 🗄️ Storage | **libSQL** — local SQLite file or remote **Turso** (same code path) |

> ⚠️ **Paper trading by default.** Trades are simulated against a synthetic
> market so the entire product works end-to-end with no RPC nodes or keys.
> Swapping `MarketSimulator` for a real DEX/RPC integration is the path to live
> trading — every other layer (risk → fill → accounting → alerts → audit log)
> is already wired.

## Requirements

- Rust 1.75+ (stable)
- A Telegram bot token from [@BotFather](https://t.me/BotFather)

## Quick start (local libSQL)

```bash
# 1. configure
cp .env.example .env
#    → edit TELOXIDE_TOKEN (required)
#    → DATABASE_URL=file:./velocidad.db is the default

# 2. run
cargo run
```

On first start Velocidad creates `velocidad.db` (schema + migrations) and a
`velocidad.key` master key used to encrypt wallet private keys. **Keep both
safe.** Then open your bot in Telegram:

```
/start                 👋 onboard
/wallet new            💰 create a wallet (private key shown once)
/buy 0xdeadbeef… 0.5   🟢 instant buy 0.5 ETH worth
/snipe 0xdeadbeef… 0.5 🛰️ snipe with honeypot gate
/scan 0xdeadbeef…      🔬 security report
/alert 0xdeadbeef… above 0.001  🔔 price alert
/portfolio             📂 positions + PnL
/help                  ❓ all commands
```

## Using Turso (hosted libSQL)

```bash
# install the turso CLI: https://docs.turso.tech/cli
turso auth login
turso db create velocidad

# get the connection URL + token
turso db show velocidad        # → libsql://velocidad-<org>.turso.io
turso db tokens create velocidad

# point the bot at it
DATABASE_URL=libsql://velocidad-<org>.turso.io
TURSO_AUTH_TOKEN=<token from the previous step>
```

The same schema/migrations run against Turso automatically — nothing else
changes.

## HTTP API

Start the bot, then:

| Endpoint | Purpose |
| --- | --- |
| `GET  /health` | liveness |
| `GET  /api/v1/users/:telegram_id` | user, wallets, stats |
| `GET  /api/v1/portfolio/:telegram_id` | open positions + live PnL |
| `GET  /api/v1/trades/:telegram_id` | order history |
| `POST /api/v1/trades` | execute trade |
| `GET  /api/v1/tokens/:address` | price + security report |
| `GET  /api/v1/alerts/:telegram_id` | alerts |
| `POST /api/v1/alerts` | create alert |

```bash
curl -s localhost:8080/health
curl -s -X POST localhost:8080/api/v1/trades \
  -H 'content-type: application/json' \
  -d '{"telegram_id": 123456, "token_address": "0xdeadbeef", "side": "buy", "amount": 0.5}'
```

## Project layout

```
src/
├── main.rs          # entry: config → db → bot + api
├── config.rs        # figment/dotenv config (DATABASE_URL, TELOXIDE_TOKEN, …)
├── app.rs           # shared AppState
├── crypto.rs        # EVM keygen/import + AES-256-GCM keyring
├── security.rs      # honeypot / rug-pull heuristics → risk score
├── db/
│   ├── mod.rs       # libSQL connection (local or Turso) + migrations
│   ├── models.rs    # User, Wallet, Token, Order, Position, Alert
│   └── repo.rs      # parameterized queries
├── trading/
│   ├── mod.rs       # engine: buy/sell/snipe/limit + receipt
│   ├── market.rs    # deterministic mock market (paper trading)
│   └── risk.rs      # position size / slippage / address validation
├── bot/
│   └── mod.rs       # teloxide command surface (Trojan-style)
└── api/
    └── mod.rs       # Axum REST API
```

## Roadmap to live trading

1. Replace `MarketSimulator::price()` with a real price feed (RPC / DexScreener).
2. Replace `TokenScanner::scan()` internals with on-chain checks (holders, LP
   locks, transfer tax via RPC).
3. Sign + broadcast orders with the stored wallet keys (ethers-rs / alloy).
4. Background workers: limit-order matching, alert polling.
5. JWT auth + rate limiting on the API; WebSocket price streams for the
   TanStack dashboard.

## Configuration reference

| Variable | Default | Description |
| --- | --- | --- |
| `DATABASE_URL` | `file:./velocidad.db` | libSQL file or Turso URL |
| `TURSO_AUTH_TOKEN` | — | required for Turso URLs |
| `TELOXIDE_TOKEN` | — | Telegram bot token (required) |
| `API_PORT` | `8080` | HTTP API port |
| `PAPER_TRADING` | `true` | simulate fills instead of broadcasting |
| `MASTER_KEY` | auto-generated | 64-hex AES key for wallet encryption |

All vars can also be prefixed `VELOCIDAD_` (e.g. `VELOCIDAD_API_PORT`).
