# Velocidad Trading Bot - Technical Architecture & Roadmap

> ⚠️ STATUS: This document is a **roadmap** (aspirational targets like
> PostgreSQL/Redis/K8s). The **actual implemented architecture** — libSQL/Turso,
> teloxide long-polling, axum, DexScreener/0x/Jupiter, EIP-7702 sponsorship —
> is documented in `README.md` and mirrors this roadmap only partially.
>

## Overview

Velocidad is a high-performance, multi-chain Telegram trading bot that combines features from Trojan, Axiom, Photon, and DexScreener with a comprehensive web interface. Built with Rust for backend reliability and TanStack (React) for frontend performance, this system provides real-time trading, market analysis, and portfolio management.

## Core Principles

1. **Performance**: Sub-millisecond trading execution with Rust's zero-cost abstractions
2. **Security**: Memory-safe Rust backend, comprehensive input validation, and encrypted storage
3. **Reliability**: Fault-tolerant architecture with automatic failover and recovery
4. **Scalability**: Horizontally scalable components for high-frequency trading loads
5. **User Experience**: Real-time updates with WebSockets, intuitive web interface

## System Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         User Interfaces                                 │
├─────────────────────────────┬───────────────────────────────────────────┤
│     Telegram Bot            │           Web Dashboard                   │
│   (Teloxide + Webhooks)     │    (TanStack React + TypeScript)          │
└──────────────┬──────────────┴──────────────┬───────────────────────────┘
               │                             │
               └──────────────┬──────────────┘
                              │
               ┌──────────────▼──────────────┐
               │       API Gateway           │
               │    (Axum + JWT Auth +       │
               │     Rate Limiting)          │
               └──────────────┬──────────────┘
                              │
               ┌──────────────▼──────────────┐
               │     Service Orchestrator    │
               │  (Event-driven, Async)      │
               └──────────────┬──────────────┘
                              │
    ┌──────────┬──────────────┼──────────────┬──────────┐
    │          │              │              │          │
┌───▼──┐  ┌───▼──┐     ┌─────▼────┐    ┌───▼──┐  ┌───▼──┐
│Trading│  │Market│     │ Portfolio│    │Alert │  │User  │
│Engine │  │ Data │     │ Manager  │    │System│  │Mgmt  │
└───┬──┘  └───┬──┘     └─────┬────┘    └───┬──┘  └───┬──┘
    │         │              │             │         │
    └─────────┼──────────────┼─────────────┼─────────┘
              │              │             │
    ┌─────────▼──────────────▼─────────────▼─────────┐
    │           Data Access Layer                     │
    │    (SQLx + Redis + Message Queue)              │
    └─────────────────────────────────────────────────┘
```

## Tech Stack

### Backend (Rust)
- **Web Framework**: Axum (async, tower ecosystem)
- **Telegram Bot**: Teloxide (official wrapper)
- **Database**: PostgreSQL with SQLx (compile-time checked queries)
- **Caching**: Redis with redis-rs
- **Message Queue**: RabbitMQ with lapin or Kafka with rdkafka
- **Blockchain**: 
  - Ethereum: ethers-rs, alloy-rs
  - Solana: solana-client, anchor-lang
  - Multi-chain: Cosmos SDK (cosmrs), Near (near-sdk-rs)
- **WebSocket**: tokio-tungstenite + tokio-serde
- **Real-time Data**: InfluxDB for time-series, TimescaleDB
- **Monitoring**: Prometheus + Grafana with metrics crate
- **Logging**: tracing + OpenTelemetry
- **Configuration**: figment with dotenv

### Frontend (TanStack Ecosystem)
- **Framework**: React 18+ with TypeScript
- **Build Tool**: Vite with SWC
- **State Management**: 
  - Server State: TanStack Query v5
  - Client State: Zustand or Jotai
  - Form State: TanStack Form
  - Router: TanStack Router
- **UI Components**:
  - Styling: Tailwind CSS + shadcn/ui
  - Charts: TradingView Lightweight Charts + Recharts
  - Tables: TanStack Table v8
  - Icons: Lucide React
- **Real-time**: Socket.io-client or native WebSocket
- **Build Quality**: ESLint, Prettier, Vitest, Playwright

### Infrastructure
- **Containerization**: Docker + Docker Compose
- **Orchestration**: Kubernetes for production
- **CI/CD**: GitHub Actions with Rust caching
- **Security**: Vault for secrets, Cloudflare for DDoS protection
- **Monitoring**: ELK Stack, Sentry, Datadog

## Component Breakdown

### 1. Telegram Bot Service (`teloxide-service`)
```rust
// High-level structure
pub struct TelegramBot {
    bot: Bot,
    trading_engine: Arc<TradingEngine>,
    alert_system: Arc<AlertSystem>,
    user_manager: Arc<UserManager>,
}

impl TelegramBot {
    async fn handle_command(&self, msg: Message) -> Result<()> {
        match msg.text() {
            Some("/portfolio") => self.handle_portfolio(msg).await,
            Some("/snip") => self.handle_snipe(msg).await,
            Some("/limit") => self.handle_limit_order(msg).await,
            Some("/chart") => self.handle_chart(msg).await,
            Some("/arbitrage") => self.handle_arbitrage(msg).await,
            _ => Ok(()),
        }
    }
    
    async fn handle_snipe(&self, msg: Message) -> Result<()> {
        // Parse token address, amount, slippage
        // Validate token (honeypot check, contract verification)
        // Execute trade via trading engine
        // Send confirmation with inline buttons
    }
}
```

### 2. Trading Engine (`trading-engine`)
```rust
pub struct TradingEngine {
    dex_aggregator: Arc<DexAggregator>,
    risk_manager: Arc<RiskManager>,
    order_manager: Arc<OrderManager>,
    strategy_runner: StrategyRunner,
}

impl TradingEngine {
    pub async fn execute_snipe(&self, config: SnipeConfig) -> Result<TradeResult> {
        // 1. Token validation (rug check, liquidity verification)
        // 2. MEV protection (Flashbots, Eden Network)
        // 3. Optimal route calculation across DEXs
        // 4. Gas optimization with EIP-1559
        // 5. Transaction simulation before execution
        // 6. Atomic execution with fallback
    }
    
    pub async fn find_arbitrage(&self) -> Vec<ArbitrageOpportunity> {
        // Monitor price differences across:
        // - Uniswap V2/V3, Sushiswap, Pancakeswap
        // - Curve, Balancer, Bancor
        // - Cross-chain arbitrage (Layer 2, different chains)
    }
    
    pub async fn run_strategy(&self, strategy: Box<dyn TradingStrategy>) {
        // Strategy patterns:
        // - Market Making
        // - Statistical Arbitrage
        // - Mean Reversion
        // - Momentum Trading
        // - HFT Scalping
    }
}
```

### 3. Market Data Aggregator (`market-data`)
```rust
pub struct MarketDataAggregator {
    ws_connections: HashMap<Dex, WebSocketStream>,
    price_feeds: HashMap<TokenId, PriceFeed>,
    liquidity_pools: Arc<RwLock<HashMap<PoolId, LiquidityPool>>>,
}

impl MarketDataAggregator {
    pub async fn subscribe_to_dex(&self, dex: Dex) -> Result<()> {
        // WebSocket connections to:
        // - The Graph for on-chain data
        // - DEX APIs (Uniswap, 1inch, 0x)
        // - Price oracles (Chainlink, Pyth)
        // - Blockchain RPC nodes
    }
    
    pub fn get_token_metrics(&self, token: TokenId) -> TokenMetrics {
        // Real-time metrics:
        // - Price with confidence intervals
        // - Liquidity depth
        // - Volume and velocity
        // - Holder distribution
        // - Social sentiment
    }
}
```

### 4. Web API Gateway (`api-gateway`)
```rust
// Axum application structure
pub async fn create_app() -> Router {
    Router::new()
        .route("/api/v1/trades", post(execute_trade))
        .route("/api/v1/portfolio", get(get_portfolio))
        .route("/api/v1/market/:token", get(get_market_data))
        .route("/api/v1/ws", get(handle_websocket))
        .layer(Extension(SharedState::new()))
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(RateLimitLayer::new(
            RateLimiter::drain().with_interval(Duration::from_secs(1))
        ))
}

// WebSocket handler for real-time updates
async fn handle_websocket(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}
```

### 5. Frontend Architecture (`frontend/`)

```
frontend/
├── src/
│   ├── api/                    # TanStack Query hooks
│   │   ├── trades.ts
│   │   ├── market.ts
│   │   └── portfolio.ts
│   ├── components/
│   │   ├── trading/
│   │   │   ├── TradeForm.tsx
│   │   │   ├── OrderBook.tsx
│   │   │   └── Chart.tsx
│   │   ├── dashboard/
│   │   │   ├── PortfolioCard.tsx
│   │   │   ├── MarketOverview.tsx
│   │   │   └── AlertManager.tsx
│   │   └── ui/                # shadcn/ui components
│   ├── routes/
│   │   ├── dashboard/
│   │   ├── trading/
│   │   └── settings/
│   ├── stores/                # Zustand stores
│   │   ├── user.store.ts
│   │   ├── trading.store.ts
│   │   └── ui.store.ts
│   ├── lib/
│   │   ├── websocket.ts
│   │   ├── blockchain.ts
│   │   └── utils.ts
│   └── types/                 # TypeScript definitions
```

#### TanStack Query Configuration
```typescript
// src/api/client.ts
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 1000 * 30, // 30 seconds
      gcTime: 1000 * 60 * 5, // 5 minutes
      retry: 2,
      refetchOnWindowFocus: false,
    },
  },
});

// Trading API hooks
export const useExecuteTrade = () => {
  return useMutation({
    mutationFn: async (trade: TradeRequest) => {
      const response = await fetch('/api/v1/trades', {
        method: 'POST',
        body: JSON.stringify(trade),
      });
      return response.json();
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['portfolio'] });
      queryClient.invalidateQueries({ queryKey: ['trades'] });
    },
  });
};

// Real-time subscription hook
export const useMarketData = (token: string) => {
  return useQuery({
    queryKey: ['market', token],
    queryFn: () => fetchMarketData(token),
    refetchInterval: 5000, // 5 seconds
  });
};
```

## Data Flow

### 1. Trade Execution Flow
```
User Command (Telegram/Web) 
    → API Gateway (Auth + Validation)
    → Trading Engine (Strategy Selection)
    → Risk Manager (Position Sizing, Limits)
    → DEX Aggregator (Route Optimization)
    → Blockchain (Transaction Simulation)
    → Execution (MEV-Protected Tx)
    → Database (Trade Recording)
    → Notification (Telegram/WebSocket)
```

### 2. Real-time Market Data Flow
```
Blockchain Events (RPC/WebSocket)
    → Market Data Aggregator (Parsing)
    → Price Feed (Normalization)
    → Redis Cache (Low-latency storage)
    → WebSocket Server (Broadcast)
    → Frontend (TanStack Query Updates)
    → UI Components (Re-render)
```

### 3. Alert System Flow
```
Market Conditions (Price, Volume, etc.)
    → Alert Engine (Condition Evaluation)
    → Priority Queue (Alert Scheduling)
    → Notification Service (Multi-channel)
    → User Delivery (Telegram/Email/Web)
    → Acknowledgment Tracking
```

## Database Schema

### Core Tables
```sql
-- Users and authentication
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    telegram_id BIGINT UNIQUE,
    wallet_address TEXT UNIQUE,
    encrypted_private_key BYTEA, -- Encrypted with user password
    settings JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Trading strategies
CREATE TABLE strategies (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id),
    name TEXT NOT NULL,
    type STRATEGY_TYPE NOT NULL, -- 'sniping', 'arbitrage', 'hft', 'market_making'
    config JSONB NOT NULL,
    is_active BOOLEAN DEFAULT true,
    performance_metrics JSONB DEFAULT '{}'
);

-- Trade executions
CREATE TABLE trades (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id),
    strategy_id UUID REFERENCES strategies(id),
    chain_id INTEGER NOT NULL,
    token_in TEXT NOT NULL,
    token_out TEXT NOT NULL,
    amount_in DECIMAL(36, 18) NOT NULL,
    amount_out DECIMAL(36, 18) NOT NULL,
    gas_used BIGINT,
    gas_price DECIMAL(36, 18),
    tx_hash TEXT UNIQUE,
    status TRADE_STATUS NOT NULL, -- 'pending', 'executed', 'failed', 'reverted'
    execution_time TIMESTAMPTZ DEFAULT NOW(),
    profit_loss DECIMAL(36, 18)
);

-- Market data (time-series)
CREATE TABLE market_data (
    token_address TEXT NOT NULL,
    dex TEXT NOT NULL,
    price DECIMAL(36, 18) NOT NULL,
    liquidity DECIMAL(36, 18) NOT NULL,
    volume_24h DECIMAL(36, 18),
    timestamp TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (token_address, dex, timestamp)
) USING TimescaleDB;

-- Create hypertable for time-series data
SELECT create_hypertable('market_data', 'timestamp');
```

## Security Architecture

### 1. Private Key Management
```rust
// Secure key storage using AWS KMS or HashiCorp Vault
pub struct SecureKeyManager {
    kms_client: aws_sdk_kms::Client,
    key_id: String,
}

impl SecureKeyManager {
    pub async fn sign_transaction(
        &self, 
        tx: TypedTransaction, 
        user_id: Uuid
    ) -> Result<Signature> {
        // Derive key from master seed + user salt
        // Decrypt via KMS (never stored in plaintext)
        // Sign transaction in isolated environment
        // Clear memory immediately after use
    }
}
```

### 2. Rate Limiting & Anti-Abuse
```rust
// Distributed rate limiting with Redis
pub struct RateLimiter {
    redis: redis::Client,
}

impl RateLimiter {
    pub async fn check_limit(&self, user_id: Uuid, action: &str) -> Result<bool> {
        let key = format!("rate_limit:{}:{}", user_id, action);
        let count: u64 = self.redis.incr(&key, 1).await?;
        
        if count == 1 {
            // Set expiration on first increment
            self.redis.expire(&key, 60).await?; // 60 seconds
        }
        
        Ok(count <= self.get_limit(action))
    }
}
```

### 3. Smart Contract Verification
```rust
pub struct ContractVerifier {
    slither: SlitherAnalyzer,
    mythril: MythrilAnalyzer,
    manual_checks: Vec<Check>,
}

impl ContractVerifier {
    pub async fn verify_token(&self, address: Address) -> VerificationResult {
        // 1. Bytecode analysis for honeypot patterns
        // 2. Function signature analysis
        // 3. Ownership and admin privileges
        // 4. Transfer tax and fee structures
        // 5. Liquidity lock verification
    }
}
```

## Deployment Architecture

### Docker Configuration
```dockerfile
# Backend Dockerfile
FROM rust:1.75-slim as builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin velocidad-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    openssl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/velocidad-api /usr/local/bin/
CMD ["velocidad-api"]
```

### Kubernetes Deployment
```yaml
# k8s/deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: trading-engine
spec:
  replicas: 3
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 1
      maxUnavailable: 0
  template:
    spec:
      containers:
      - name: trading-engine
        image: velocidad/trading-engine:latest
        env:
        - name: RUST_LOG
          value: "info"
        - name: DATABASE_URL
          valueFrom:
            secretKeyRef:
              name: db-credentials
              key: url
        resources:
          requests:
            memory: "512Mi"
            cpu: "500m"
          limits:
            memory: "1Gi"
            cpu: "1000m"
        livenessProbe:
          httpGet:
            path: /health
            port: 8080
          initialDelaySeconds: 30
          periodSeconds: 10
```

## Development Roadmap

### Phase 1: Foundation (Weeks 1-4)
- [ ] Set up Rust workspace with crates.io dependencies
- [ ] Implement basic Axum API with JWT authentication
- [ ] Create PostgreSQL schema with SQLx migrations
- [ ] Set up Teloxide bot with command structure
- [ ] Initialize TanStack React project with TypeScript
- [ ] Configure CI/CD with GitHub Actions

### Phase 2: Core Trading (Weeks 5-8)
- [ ] Implement ethers-rs integration for Ethereum
- [ ] Build DEX aggregator with 1inch/0x APIs
- [ ] Create basic trading engine with limit orders
- [ ] Implement risk management system
- [ ] Build Telegram trading commands
- [ ] Create web dashboard with portfolio view

### Phase 3: Advanced Features (Weeks 9-12)
- [ ] Add multi-chain support (Solana, Arbitrum, Polygon)
- [ ] Implement arbitrage detection engine
- [ ] Build HFT market making strategies
- [ ] Add token sniping with MEV protection
- [ ] Implement real-time charts with TradingView
- [ ] Create alert system with multiple channels

### Phase 4: Optimization & Scaling (Weeks 13-16)
- [ ] Add WebSocket streaming for real-time data
- [ ] Implement caching layer with Redis
- [ ] Add message queue for trade execution
- [ ] Optimize database queries and indexing
- [ ] Implement comprehensive monitoring
- [ ] Add automated testing and benchmarking

### Phase 5: Production Readiness (Weeks 17-20)
- [ ] Security audit and penetration testing
- [ ] Load testing and performance optimization
- [ ] Disaster recovery and backup procedures
- [ ] Documentation and user guides
- [ ] Beta testing with select users
- [ ] Production deployment

## Performance Targets

1. **Latency**: < 100ms for trade execution, < 10ms for market data updates
2. **Throughput**: Support 1000+ concurrent users, 100+ trades per second
3. **Uptime**: 99.99% availability with automatic failover
4. **Data Freshness**: < 1 second delay for price updates
5. **Database**: < 50ms query latency for 95% of requests

## Monitoring & Observability

### Metrics Collection
```rust
// Using Prometheus metrics
lazy_static! {
    static ref TRADES_EXECUTED: IntCounter = register_int_counter!(
        "trades_executed_total",
        "Total number of trades executed"
    ).unwrap();
    
    static ref TRADE_LATENCY: Histogram = register_histogram!(
        "trade_execution_latency_seconds",
        "Trade execution latency in seconds"
    ).unwrap();
}

// Tracing with OpenTelemetry
let tracer = opentelemetry_jaeger::new_pipeline()
    .with_service_name("trading-engine")
    .install_simple()?;
```

### Logging Structure
```rust
use tracing::{info, error, warn, debug};

#[tracing::instrument(skip(trade, engine))]
async fn execute_trade(trade: Trade, engine: Arc<TradingEngine>) -> Result<()> {
    info!(trade_id = %trade.id, "Starting trade execution");
    
    let timer = TRADE_LATENCY.start_timer();
    let result = engine.execute(trade).await;
    timer.observe_duration();
    
    match result {
        Ok(_) => {
            TRADES_EXECUTED.inc();
            info!(trade_id = %trade.id, "Trade executed successfully");
        }
        Err(e) => {
            error!(trade_id = %trade.id, error = %e, "Trade execution failed");
        }
    }
    
    result
}
```

## Risk Management

### Trading Limits
```rust
pub struct RiskManager {
    daily_loss_limit: Decimal,
    position_size_limit: Decimal,
    max_slippage: Decimal,
    blacklisted_tokens: HashSet<Address>,
}

impl RiskManager {
    pub async fn validate_trade(&self, trade: &TradeProposal) -> Result<()> {
        // Check daily P&L limits
        // Verify position size vs portfolio
        // Validate token safety (honeypot, rug pull risk)
        // Check market conditions (volatility, liquidity)
        // Ensure sufficient gas budget
    }
}
```

## Scaling Strategies

### Horizontal Scaling
1. **Stateless Services**: API gateway, Telegram bot
2. **Sharded Databases**: User data by region, market data by token
3. **Message Queue Partitioning**: Trades by user group, alerts by priority
4. **Cache Replication**: Redis cluster with read replicas

### Vertical Scaling
1. **Memory Optimization**: Use arenas for high-frequency data
2. **CPU Optimization**: SIMD for price calculations, async/await for I/O
3. **Disk Optimization**: SSD for databases, RAM disk for hot cache

## Future Extensions

1. **DeFi Integration**: Lending, staking, yield farming strategies
2. **NFT Trading**: Floor price tracking, collection analysis
3. **Cross-chain Bridges**: Atomic swaps, bridge arbitrage
4. **Institutional Features**: OTC trading, dark pools
5. **AI/ML Integration**: Predictive analytics, sentiment analysis
6. **Mobile App**: React Native for iOS/Android

## Conclusion

This architecture provides a robust foundation for building a high-performance trading bot using Rust's safety and performance guarantees with TanStack's modern React patterns. The system is designed to be secure, scalable, and maintainable while providing real-time trading capabilities across multiple blockchains.

Key advantages:
- **Rust**: Memory safety, zero-cost abstractions, fearless concurrency
- **TanStack**: Type-safe, performant, developer-friendly React ecosystem
- **PostgreSQL + TimescaleDB**: Reliable storage with time-series optimization
- **Redis**: Low-latency caching and real-time data distribution
- **Kubernetes**: Production-grade orchestration and scaling

The modular design allows for incremental development and easy integration of new features, while the comprehensive monitoring and security measures ensure production readiness.