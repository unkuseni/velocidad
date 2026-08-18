# ⚡ Velocidad

A **Trojan/Photon/Axiom-style Telegram trading bot** built in Rust — token sniping,
instant buy/sell, auto-filled limit orders, security scans, price alerts, wallets,
portfolio tracking — with **multi-chain support**: 12 EVM chains (Ethereum, BSC,
Polygon, Arbitrum, Optimism, Base, Avalanche, Fantom, Linea, Blast, Scroll, Gnosis)
**plus Solana** (ed25519 wallets, SPL tokens, Jupiter swaps) and **libSQL / Turso**
as the database.

```
Telegram Bot (teloxide) ─┐
                         ├─▶ Trading Engine ─▶ libSQL (local file or Turso cloud)
HTTP API (Axum) ────────┘        │
                                  └─▶ Positions · Orders · Alerts · Wallets · Tokens

background workers: limit-order matching + price-alert polling
```

## What works today

| Feature | How |
| --- | --- |
| ⛓️ Multi-chain | 13 chains via one registry (12 EVM + Solana); `chain:0x…` / `sol:…` prefix on any token command or a per-user default chain |
| 💰 Wallet management | Generate/import EVM + Solana wallets (auto-detected); private keys encrypted at rest with AES-256-GCM |
| 📈 Live market data | DexScreener prices, liquidity, volume, FDV, pair age, buy/sell pressure — with an offline simulator fallback |
| 🟢🔴 Instant buy/sell | Paper fills at **live** prices by default; real on-chain swaps when `PAPER_TRADING=false` — EVM via 0x Swap API v2 (EIP-1559), Solana via Jupiter v6 (ed25519, keyless) |
| 🛰️ Token sniping | Same fast path, with an automated honeypot/risk gate |
| 🔬 Security scans | Liquidity, pair age, price action, buy/sell pressure + optional honeypot.is live simulation (EVM) → risk score |
| ⏳ Limit orders | Pending orders **auto-filled by a background worker** when the trigger price is hit, with Telegram notification |
| 🔔 Price alerts | `above`/`below` triggers checked by a background poller; instant Telegram notification |
| 📂 Portfolio & PnL | Weighted-average positions, realized + unrealized PnL, per-chain with USD totals |
| 💰 On-chain balances | Native + token balances via public RPCs (EVM ERC-20, Solana SPL) (`/balance`) |
| ⛽ Gas sponsorship | Opt-in: EIP-7702 delegation (EVM) + SPL delegate/fee-payer (Solana) — the operator's sponsor key pays your gas (`/sponsor`) |
| 🔍 Token discovery | `/find <ticker>` (full-text search), `/trending`, `/boosts` (DexScreener feeds) |
| 🌐 Web API | Axum REST API mirroring the bot (see below) |
| 🗄️ Storage | **libSQL** — local SQLite file or remote **Turso** (same code path) |

> ⚠️ **Paper trading by default.** Trades fill instantly against **real** DexScreener
> prices, so the entire product works end-to-end with no API keys. Set
> `PAPER_TRADING=false` + `ZEROEX_API_KEY` to execute real on-chain swaps signed
> with the user's stored wallet key.

## Requirements

- Rust 1.75+ (stable)
- A Telegram bot token from [@BotFather](https://t.me/BotFather)
- (optional) a free [0x API key](https://0x.org/) for live swaps

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
/start                       👋 onboard
/wallet new                  💰 create an EVM wallet (private key shown once)
/wallet new solana           🪐 create a Solana wallet (Phantom-style key)
/wallet import <key>         📥 auto-detects EVM hex / Solana base58
/chain bsc                   ⛓️ set your default chain
/buy bsc:0xdeadbeef… 0.5     🟢 buy 0.5 BNB worth of a BSC token
/buy sol:EPjF… 1.5           🪐 buy Solana tokens (paper or live via Jupiter)
/sell 0xdeadbeef… all        🔴 sell your whole position
/snipe 0xdeadbeef… 0.5       🛰️ snipe with honeypot gate
/limit 0xdeadbeef… 0.0001 0.5 ⏳ auto-filled limit order
/price 0xdeadbeef…           📈 current price (USD + native)
/token 0xdeadbeef…           🔎 full market info (liquidity, volume, FDV, age)
/scan 0xdeadbeef…            🔬 security report
/alert 0xdeadbeef… above 0.001 🔔 price alert (auto-notified)
/find pepe                   🔍 search tokens by ticker across chains
/trending                    🔥 trending tokens
/boosts                      🚀 top boosted tokens
/balance                     💰 on-chain balances (EVM + Solana)
/portfolio                   📂 positions + PnL with per-chain USD totals
/settings slippage 0.10      ⚙️ slippage
/help                        ❓ all commands
```

**Multi-chain:** prefix any token with `chain:` — `/buy bsc:0x…`, `/price arb:0x…`,
`/scan sol:EPjF…`. Without a prefix the user's default chain (`/chain`) is used.
Solana addresses are base58 mints; `sol:` also works as the prefix.

## Gas-fee sponsorship (/sponsor)

The bot can **pay your gas fees** on both chain families. It's fully opt-in:

1. The operator sets `SPONSOR_KEY` (EVM) and/or `SPONSOR_SOLANA_KEY` and funds
   the derived addresses with gas (EVM per chain, SOL on Solana).
2. The operator deploys the sponsor account once per EVM chain: `/sponsor setup`
   (deploys `contracts/SponsorAccount.sol` from the sponsor key).
3. The user opts in per chain: `/sponsor on`:
   - **EVM** — sends an **EIP-7702** set-code transaction delegating the user's
     EOA to the SponsorAccount (type-0x04, signed locally, user pays gas once).
     From then on the sponsor executes user swaps/approvals through the
     delegate while paying the gas; funds always move from the user's balance.
   - **Solana** — sets the sponsor as **SPL delegate** on the user's token
     accounts (sponsored Approve txs) and rebuilds trade transactions with the
     **sponsor as fee payer** (dual-signed: sponsor + user).
4. `/sponsor status` shows delegation state (live `eth_getCode` check);
   `/sponsor off` clears it (zero-address authorization / revoke).

The EIP-7702 transaction builder and RFC6979 signing are golden-vector tested
against **viem**; the Solana fee-payer rebuild + dual-signing pipeline is
verified against **@solana/web3.js**.

## Live trading (on-chain swaps)

1. Get a free API key from [0x.org](https://0x.org/) and set `ZEROEX_API_KEY`.
2. Set `PAPER_TRADING=false`.
3. Import (don't generate) a wallet that holds funds: `/wallet import <privkey>`.
4. Trade: `/buy 0x… 0.05` — Velocidad quotes 0x, approves allowances when
   needed, signs an EIP-1559 transaction locally with your stored key and
   broadcasts it via public RPCs.

Supported on every registered chain. Sells set an exact-amount approval on the
0x Permit2 contract first (`/balance` shows allowance progress is not needed).

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
| `GET  /api/v1/chains` | supported EVM chains |
| `GET  /api/v1/users/:telegram_id` | user, wallets, stats |
| `GET  /api/v1/portfolio/:telegram_id` | open positions + live PnL |
| `GET  /api/v1/balances/:telegram_id?chain=bsc` | on-chain balances |
| `GET  /api/v1/trades/:telegram_id` | order history |
| `POST /api/v1/trades` | execute trade (`chain` optional) |
| `GET  /api/v1/tokens/:address?chain=bsc` | price + security report |
| `GET  /api/v1/alerts/:telegram_id` | alerts |
| `POST /api/v1/alerts` | create alert (`chain` optional) |

```bash
curl -s localhost:8080/health
curl -s 'localhost:8080/api/v1/tokens/0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599?chain=ethereum'
curl -s -X POST localhost:8080/api/v1/trades \
  -H 'content-type: application/json' \
  -d '{"telegram_id": 123456, "token_address": "0xdeadbeef", "side": "buy", "amount": 0.5, "chain": "bsc"}'
```

## Project layout

```
src/
├── main.rs          # entry: config → db → api + bot + workers
├── config.rs        # figment/dotenv config (DATABASE_URL, TELOXIDE_TOKEN, …)
├── app.rs           # shared AppState (engine, market, scanner, rpc, swap)
├── chains.rs        # EVM chain registry (12 chains, RPCs, explorers, tokens)
├── market.rs        # DexScreener market data + cache + simulator fallback
├── rpc.rs           # minimal EVM JSON-RPC client (balances, gas, broadcast)
├── swap.rs          # 0x API v2 quotes + EIP-1559 signing + broadcast
├── crypto.rs        # EVM keygen/import + AES-256-GCM keyring
├── security.rs      # honeypot / rug-pull heuristics → risk score
├── workers.rs       # background: limit matching + alert polling
├── db/
│   ├── mod.rs       # libSQL connection (local or Turso) + migrations
│   ├── models.rs    # User, Wallet, Token, Order, Position, Alert
│   └── repo.rs      # parameterized queries
├── trading/
│   ├── mod.rs       # engine: chain-aware buy/sell/snipe/limit + receipt
│   └── risk.rs      # position size / slippage / address validation
├── bot/
│   └── mod.rs       # teloxide command surface (Trojan-style)
└── api/
    └── mod.rs       # Axum REST API
```

## Configuration reference

| Variable | Default | Description |
| --- | --- | --- |
| `DATABASE_URL` | `file:./velocidad.db` | libSQL file or Turso URL |
| `TURSO_AUTH_TOKEN` | — | required for Turso URLs |
| `TELOXIDE_TOKEN` | — | Telegram bot token (required) |
| `API_PORT` | `8080` | HTTP API port |
| `PAPER_TRADING` | `true` | simulate fills (at live prices) instead of broadcasting |
| `LIVE_MARKET` | `true` | DexScreener live prices (offline fallback to simulator) |
| `DEFAULT_CHAIN` | `ethereum` | default chain for chain-less commands (`solana` works too) |
| `ZEROEX_API_KEY` | — | 0x Swap API v2 key → enables real on-chain swaps |
| `HONEYPOT_API_KEY` | — | honeypot.is key → live honeypot simulations in /scan |
| `SPONSOR_KEY` | — | EIP-7702 gas-sponsorship operator key (0x-hex) |
| `SPONSOR_SOLANA_KEY` | — | Solana gas-sponsorship operator key (seed) |
| `MASTER_KEY` | auto-generated | 64-hex AES key for wallet encryption (0x allowed) |

All vars can also be prefixed `VELOCIDAD_` (e.g. `VELOCIDAD_API_PORT`).

## Tests

```bash
cargo test
```

Covers chain resolution, DexScreener parsing, RLP/EIP-1559/EIP-7702 transaction
encoding (golden vectors vs ethers.js + viem), deterministic RFC6979 ECDSA
(cross-wallet compatible), Solana address derivation + fee-payer rebuild
(golden vectors vs @solana/web3.js), calldata packing and formatting helpers.
