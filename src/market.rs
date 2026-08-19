//! Market data: live token quotes from the **DexScreener** API with an
//! in-memory cache, and a deterministic **simulator** fallback so the bot keeps
//! working end-to-end offline (paper trading).
//!
//! Every EVM chain supported by the registry maps to a DexScreener segment,
//! so one code path serves Ethereum, BSC, Base, Arbitrum and friends.
//!
//! API: `GET https://api.dexscreener.com/latest/dex/tokens/{address}`

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::chains::Chain;

/// Price + market info for one token on one chain.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TokenQuote {
    pub address: String,
    pub chain_id: String,
    pub name: String,
    pub symbol: String,
    /// USD price of the token.
    pub price_usd: f64,
    /// Token price in native units (e.g. ETH on Ethereum, BNB on BSC).
    pub price_native: f64,
    /// Total liquidity across the best pair, USD.
    pub liquidity_usd: f64,
    /// 24h volume, USD.
    pub volume_24h_usd: f64,
    /// 24h price change, percent (-100 … +inf).
    pub price_change_24h: f64,
    /// Fully diluted valuation, USD.
    pub fdv: Option<f64>,
    /// Best-pair address, when listed.
    pub pair_address: Option<String>,
    /// DEX name of the best pair.
    pub dex: Option<String>,
    /// Best-pair creation time (unix seconds) — used for "age" heuristics.
    pub pair_created_at: Option<i64>,
    /// 24h buy/sell transaction counts.
    pub txns_buy_24h: u64,
    pub txns_sell_24h: u64,
    /// `"dexscreener"` when live data was used, `"simulator"` for fallback.
    pub source: &'static str,
}

/// A trending token profile (DexScreener token-profiles feed).
#[derive(Debug, Clone)]
pub struct TrendingToken {
    pub chain: String,
    pub token_address: String,
    pub name: Option<String>,
    pub symbol: Option<String>,
    #[allow(dead_code)]
    pub url: String,
}

#[derive(Deserialize, Default)]
struct DexPairsResponse {
    #[serde(default)]
    pairs: Vec<DexPair>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DexPair {
    #[serde(default)]
    chain_id: Option<String>,
    #[serde(default)]
    dex_id: Option<String>,
    #[serde(default)]
    pair_address: Option<String>,
    #[serde(default)]
    base_token: Option<DexToken>,
    #[serde(default)]
    quote_token: Option<DexToken>,
    #[serde(default)]
    price_usd: Option<String>,
    #[serde(default)]
    price_native: Option<String>,
    #[serde(default)]
    liquidity: Option<DexLiquidity>,
    #[serde(default)]
    volume: Option<DexVolume>,
    #[serde(default)]
    price_change: Option<DexPriceChange>,
    #[serde(default)]
    fdv: Option<f64>,
    #[serde(default)]
    pair_created_at: Option<i64>,
    #[serde(default)]
    txns: Option<DexTxns>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DexToken {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
}

#[derive(Deserialize)]
struct DexLiquidity {
    #[serde(default)]
    usd: Option<f64>,
}

#[derive(Deserialize)]
struct DexVolume {
    #[serde(default)]
    h24: Option<f64>,
}

#[derive(Deserialize)]
struct DexPriceChange {
    #[serde(default)]
    h24: Option<f64>,
}

#[derive(Deserialize)]
struct DexTxns {
    #[serde(default)]
    h24: Option<DexTxnSide>,
}

#[derive(Deserialize)]
struct DexTxnSide {
    #[serde(default)]
    buys: Option<u64>,
    #[serde(default)]
    sells: Option<u64>,
}

/// Best pair for a token among DexScreener's pairs: highest liquidity among
/// pairs where the requested token is the BASE token, preferring pairs quoted
/// in the chain native or a stablecoin — bogus pairings like a broken
/// JUP/MET pool can otherwise dominate by liquidity.
fn best_pair<'a>(chain: &Chain, pairs: &'a [DexPair], token: &str) -> Option<&'a DexPair> {
    let token_lower = token.to_lowercase();
    let base_matches = |p: &&DexPair| {
        p.chain_id.as_deref() == Some(chain.dex_segment)
            && p.base_token
                .as_ref()
                .and_then(|t| t.address.as_deref())
                .map(|a| a.to_lowercase() == token_lower)
                .unwrap_or(false)
    };
    let safe_quote = |p: &&DexPair| {
        let sym = p
            .quote_token
            .as_ref()
            .and_then(|t| t.symbol.as_deref())
            .map(|s| s.to_uppercase());
        let addr = p
            .quote_token
            .as_ref()
            .and_then(|t| t.address.as_deref())
            .map(|a| a.to_lowercase());
        let native = chain.native.to_uppercase();
        let wrapped = format!("W{}", chain.native).to_uppercase();
        let stable = match sym.as_deref() {
            Some(s) => {
                let s = s.to_uppercase();
                s == native
                    || s == wrapped
                    || matches!(
                        s.as_str(),
                        "USDC"
                            | "USDT"
                            | "DAI"
                            | "BUSD"
                            | "FDUSD"
                            | "USDS"
                            | "PYUSD"
                            | "USDY"
                            | "TUSD"
                    )
            }
            None => false,
        };
        stable || addr.as_deref() == Some(&chain.wrapped_native.to_lowercase())
    };
    pairs
        .iter()
        .filter(base_matches)
        .filter(safe_quote)
        .max_by(|a, b| {
            a.liquidity
                .as_ref()
                .and_then(|l| l.usd)
                .unwrap_or(0.0)
                .partial_cmp(&b.liquidity.as_ref().and_then(|l| l.usd).unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .or_else(|| {
            // Fall back to any base-matching pair when no safe-quote pair exists.
            pairs.iter().filter(base_matches).max_by(|a, b| {
                a.liquidity
                    .as_ref()
                    .and_then(|l| l.usd)
                    .unwrap_or(0.0)
                    .partial_cmp(&b.liquidity.as_ref().and_then(|l| l.usd).unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        })
}

/// Live market data with cache + simulator fallback.
pub struct MarketData {
    http: reqwest::Client,
    /// (chain_id, address) → cached quote
    quote_cache: Mutex<HashMap<(String, String), (Instant, TokenQuote)>>,
    /// chain id → cached native price USD
    native_cache: Mutex<HashMap<&'static str, (Instant, f64)>>,
    trending_cache: Mutex<Option<(Instant, Vec<TrendingToken>)>>,
    /// When false, always use the simulator (deterministic offline mode).
    pub live: bool,
}

const QUOTE_TTL: Duration = Duration::from_secs(30);
const NATIVE_TTL: Duration = Duration::from_secs(120);
const TRENDING_TTL: Duration = Duration::from_secs(300);

impl MarketData {
    pub fn new(live: bool) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent("velocidad-trading-bot/0.2")
            .build()
            .expect("failed to build HTTP client");
        Self {
            http,
            quote_cache: Mutex::new(HashMap::new()),
            native_cache: Mutex::new(HashMap::new()),
            trending_cache: Mutex::new(None),
            live,
        }
    }

    /// Best quote for a token on a chain. Falls back to the deterministic
    /// simulator when DexScreener is unreachable, unlisted, or disabled.
    pub async fn quote(&self, chain: &Chain, token: &str) -> TokenQuote {
        let key = (chain.id.to_string(), token.to_lowercase());
        if let Some((at, q)) = self.quote_cache.lock().unwrap().get(&key) {
            if at.elapsed() < QUOTE_TTL {
                return q.clone();
            }
        }

        let fetched = if self.live {
            self.fetch_quote(chain, token).await
        } else {
            None
        };

        match fetched {
            Some(q) => {
                self.quote_cache
                    .lock()
                    .unwrap()
                    .insert(key, (Instant::now(), q.clone()));
                q
            }
            None => self.simulate(chain, token),
        }
    }

    /// Bulk best-quotes for several tokens on ONE chain: one HTTP request per
    /// chunk of 30 tokens (DexScreener's batch limit). Serves from cache when
    /// fresh; unlisted or unreachable tokens fall back to the simulator.
    pub async fn quotes_batch(
        &self,
        chain: &Chain,
        tokens: &[String],
    ) -> HashMap<String, TokenQuote> {
        let mut out: HashMap<String, TokenQuote> = HashMap::new();
        let mut missing: Vec<String> = Vec::new();
        for t in tokens {
            let key = (chain.id.to_string(), t.to_lowercase());
            if let Some((at, q)) = self.quote_cache.lock().unwrap().get(&key) {
                if at.elapsed() < QUOTE_TTL {
                    out.insert(t.to_lowercase(), q.clone());
                    continue;
                }
            }
            missing.push(t.clone());
        }
        if self.live {
            for chunk in missing.chunks(30) {
                for (t, q) in self.fetch_quotes(chain, chunk).await {
                    let key = (chain.id.to_string(), t.to_lowercase());
                    self.quote_cache
                        .lock()
                        .unwrap()
                        .insert(key, (Instant::now(), q.clone()));
                    out.insert(t, q);
                }
            }
        }
        for t in missing {
            out.entry(t.to_lowercase())
                .or_insert_with(|| self.simulate(chain, &t));
        }
        out
    }

    /// Hit the DexScreener API for the best pair on this chain.
    async fn fetch_quote(&self, chain: &Chain, token: &str) -> Option<TokenQuote> {
        let url = format!("https://api.dexscreener.com/latest/dex/tokens/{token}");
        let resp: DexPairsResponse = self.http.get(&url).send().await.ok()?.json().await.ok()?;
        let best = best_pair(chain, &resp.pairs, token)?;
        self.build_quote(chain, token, best).await
    }

    /// Bulk variant: /tokens/v1 returns a flat pair array for up to 30 tokens.
    async fn fetch_quotes(&self, chain: &Chain, tokens: &[String]) -> HashMap<String, TokenQuote> {
        let mut out = HashMap::new();
        if tokens.is_empty() {
            return out;
        }
        let url = format!(
            "https://api.dexscreener.com/tokens/v1/{}/{}",
            chain.dex_segment,
            tokens.join(",")
        );
        let pairs: Vec<DexPair> = match self.http.get(&url).send().await {
            Ok(resp) => resp.json().await.unwrap_or_default(),
            Err(_) => return out,
        };
        let native_usd = self.native_price_usd(chain).await;
        for token in tokens {
            if let Some(pair) = best_pair(chain, &pairs, token) {
                if let Some(q) = self.build_quote_with_native(chain, token, pair, native_usd) {
                    out.insert(token.to_lowercase(), q);
                }
            }
        }
        out
    }

    /// Build a TokenQuote from a chosen pair (single-token path).
    async fn build_quote(&self, chain: &Chain, token: &str, pair: &DexPair) -> Option<TokenQuote> {
        let native_usd = self.native_price_usd(chain).await;
        self.build_quote_with_native(chain, token, pair, native_usd)
    }

    /// Shared quote construction (given the native/USD rate).
    fn build_quote_with_native(
        &self,
        chain: &Chain,
        token: &str,
        best: &DexPair,
        native_usd: f64,
    ) -> Option<TokenQuote> {
        // Note: priceNative is quoted in the PAIR's quote token (WBNB, USDC, ...),
        // not necessarily the chain native coin — so always derive the native
        // price from USD (DexScreener's priceUsd) via the native/USD rate.
        let price_usd = best
            .price_usd
            .as_ref()
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(0.0);
        let price_native = if price_usd > 0.0 && native_usd > 0.0 {
            price_usd / native_usd
        } else {
            best.price_native
                .as_ref()
                .and_then(|p| p.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        if price_usd <= 0.0 && price_native <= 0.0 {
            return None;
        }

        let (buys, sells) = best
            .txns
            .as_ref()
            .and_then(|t| t.h24.as_ref())
            .map(|h| (h.buys.unwrap_or(0), h.sells.unwrap_or(0)))
            .unwrap_or((0, 0));

        Some(TokenQuote {
            address: token.to_string(),
            chain_id: chain.id.to_string(),
            name: best
                .base_token
                .as_ref()
                .and_then(|t| t.name.clone())
                .unwrap_or_else(|| short_addr(token)),
            symbol: best
                .base_token
                .as_ref()
                .and_then(|t| t.symbol.clone())
                .unwrap_or_else(|| short_addr(token)),
            price_usd,
            price_native,
            liquidity_usd: best.liquidity.as_ref().and_then(|l| l.usd).unwrap_or(0.0),
            volume_24h_usd: best.volume.as_ref().and_then(|v| v.h24).unwrap_or(0.0),
            price_change_24h: best
                .price_change
                .as_ref()
                .and_then(|c| c.h24)
                .unwrap_or(0.0),
            fdv: best.fdv,
            pair_address: best.pair_address.clone(),
            dex: best.dex_id.clone(),
            // DexScreener returns unix milliseconds; store seconds.
            pair_created_at: best.pair_created_at.map(|t| t / 1000),
            txns_buy_24h: buys,
            txns_sell_24h: sells,
            source: "dexscreener",
        })
    }

    /// Native coin price in USD (cached). Uses DexScreener on the wrapped
    /// native token; falls back to the chain's rough constant.
    pub async fn native_price_usd(&self, chain: &Chain) -> f64 {
        if let Some((at, p)) = self.native_cache.lock().unwrap().get(&chain.id) {
            if at.elapsed() < NATIVE_TTL {
                return *p;
            }
        }
        let price = if self.live {
            self.fetch_native_price(chain)
                .await
                .unwrap_or(chain.fallback_native_usd)
        } else {
            chain.fallback_native_usd
        };
        self.native_cache
            .lock()
            .unwrap()
            .insert(chain.id, (Instant::now(), price));
        price
    }

    async fn fetch_native_price(&self, chain: &Chain) -> Option<f64> {
        let url = format!(
            "https://api.dexscreener.com/latest/dex/tokens/{}",
            chain.wrapped_native
        );
        let resp: DexPairsResponse = self.http.get(&url).send().await.ok()?.json().await.ok()?;
        let wn_lower = chain.wrapped_native.to_lowercase();
        resp.pairs
            .iter()
            .filter(|p| {
                p.chain_id.as_deref() == Some(chain.dex_segment)
                    && p.base_token
                        .as_ref()
                        .and_then(|t| t.address.as_deref())
                        .map(|a| a.to_lowercase() == wn_lower)
                        .unwrap_or(false)
            })
            .filter_map(|p| p.price_usd.as_ref()?.parse::<f64>().ok())
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Deterministic fallback quote: stable-ish price per (chain, token).
    pub fn simulate(&self, chain: &Chain, token: &str) -> TokenQuote {
        let price_native = simulate_price(chain, token);
        let native_usd = chain.fallback_native_usd;
        TokenQuote {
            address: token.to_string(),
            chain_id: chain.id.to_string(),
            name: short_addr(token),
            symbol: short_addr(token),
            price_usd: price_native * native_usd,
            price_native,
            liquidity_usd: 0.0,
            volume_24h_usd: 0.0,
            price_change_24h: 0.0,
            fdv: None,
            pair_address: None,
            dex: None,
            pair_created_at: None,
            txns_buy_24h: 0,
            txns_sell_24h: 0,
            source: "simulator",
        }
    }

    /// Trending token profiles (DexScreener token-profiles feed), cached.
    pub async fn trending(&self) -> Vec<TrendingToken> {
        if let Some((at, list)) = self.trending_cache.lock().unwrap().as_ref() {
            if at.elapsed() < TRENDING_TTL {
                return list.clone();
            }
        }
        let list = if self.live {
            self.fetch_trending().await
        } else {
            Vec::new()
        };
        self.trending_cache
            .lock()
            .unwrap()
            .replace((Instant::now(), list.clone()));
        list
    }

    async fn fetch_trending(&self) -> Vec<TrendingToken> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Profile {
            #[serde(default)]
            chain_id: Option<String>,
            #[serde(default)]
            token_address: Option<String>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            symbol: Option<String>,
            #[serde(default)]
            url: Option<String>,
        }
        let url = "https://api.dexscreener.com/token-profiles/latest/v1";
        let profiles: Vec<Profile> = match self.http.get(url).send().await {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(_) => return Vec::new(),
        };
        profiles
            .into_iter()
            .filter_map(|p| {
                let token_address = p.token_address?;
                let chain = p.chain_id.unwrap_or_default();
                if chain.is_empty() {
                    return None;
                }
                Some(TrendingToken {
                    chain,
                    token_address,
                    name: p.name,
                    symbol: p.symbol,
                    url: p.url.unwrap_or_default(),
                })
            })
            .collect()
    }

    /// Recently-launched pairs on a chain (launchpad feed). Uses the
    /// token-profiles feed (the same source as /trending, cached 5 min) and
    /// enriches each profile with its live quote.
    pub async fn new_pairs_on(&self, chain: &Chain) -> Vec<NewPair> {
        let profiles = self.trending().await;
        let tokens: Vec<String> = profiles
            .iter()
            .filter(|p| p.chain == chain.id)
            .map(|p| p.token_address.clone())
            .collect();
        if tokens.is_empty() {
            return Vec::new();
        }
        let quotes = self.quotes_batch(chain, &tokens).await;
        let now = chrono::Utc::now().timestamp();
        let mut out = Vec::new();
        for p in profiles.iter().filter(|p| p.chain == chain.id) {
            let Some(q) = quotes.get(&p.token_address.to_lowercase()) else {
                continue;
            };
            let age_secs = q.pair_created_at.map(|t| now - t);
            if let Some(age) = age_secs {
                if !(0..=24 * 3600).contains(&age) {
                    continue;
                }
            }
            out.push(NewPair {
                chain: chain.id.to_string(),
                token_address: p.token_address.clone(),
                symbol: q.symbol.clone(),
                pair_address: q.pair_address.clone().unwrap_or_default(),
                liquidity_usd: q.liquidity_usd,
                volume_24h_usd: q.volume_24h_usd,
                pair_age_secs: age_secs,
            });
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.liquidity_usd as u64));
        out
    }

    /// LP-friendly token suggestions: newest DexScreener profiles scored by
    /// liquidity depth, fee yield (volume/LP) and volatility (IL risk).
    pub async fn lp_suggestions(&self) -> Vec<LpSuggestion> {
        let profiles = self.trending().await;
        if profiles.is_empty() {
            return Vec::new();
        }
        // Group addresses per chain for batched quotes.
        let mut by_chain: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for p in &profiles {
            by_chain
                .entry(p.chain.clone())
                .or_default()
                .push(p.token_address.clone());
        }
        let mut out: Vec<LpSuggestion> = Vec::new();
        for (network, tokens) in by_chain {
            let Some(chain) = crate::chains::by_id(&network) else {
                continue;
            };
            let quotes = self.quotes_batch(chain, &tokens).await;
            for p in profiles.iter().filter(|p| p.chain == network) {
                let Some(q) = quotes.get(&p.token_address.to_lowercase()) else {
                    continue;
                };
                if q.price_usd <= 0.0 || q.source == "simulator" {
                    continue;
                }
                let (score, reasons) = lp_score(
                    q.liquidity_usd,
                    q.volume_24h_usd,
                    q.price_change_24h,
                    q.pair_created_at
                        .map(|t| (chrono::Utc::now().timestamp() - t).max(0)),
                );
                if score < 25 {
                    continue; // too thin / too volatile — not worth suggesting
                }
                out.push(LpSuggestion {
                    chain: network.clone(),
                    address: p.token_address.clone(),
                    name: q.name.clone(),
                    symbol: q.symbol.clone(),
                    liquidity_usd: q.liquidity_usd,
                    volume_24h_usd: q.volume_24h_usd,
                    price_change_24h: q.price_change_24h,
                    pair_age_days: q
                        .pair_created_at
                        .map(|t| (chrono::Utc::now().timestamp() - t) as f64 / 86400.0),
                    score,
                    reasons,
                });
            }
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.score));
        out.truncate(10);
        out
    }
}

/// Score an LP candidate 0-100: liquidity depth (40), fee yield via
/// volume/LP ratio (25), pool age bonus (10) minus volatility (IL proxy, up
/// to -25). Deterministic and unit-testable.
pub fn lp_score(
    liquidity_usd: f64,
    volume_24h_usd: f64,
    price_change_24h: f64,
    pair_age_secs: Option<i64>,
) -> (u8, Vec<String>) {
    let mut score = 0.0;
    let mut reasons = Vec::new();

    if liquidity_usd >= 5_000.0 {
        score += (liquidity_usd / 50_000.0 * 40.0).min(40.0);
        reasons.push(format!("{:.0}k liq", liquidity_usd / 1000.0));
    } else {
        reasons.push(format!("thin: {:.0} liq", liquidity_usd));
    }

    if liquidity_usd >= 5_000.0 {
        let ratio = volume_24h_usd / liquidity_usd * 100.0; // % of pool/day
        score += (ratio / 25.0 * 25.0).min(25.0);
        reasons.push(format!("{:.0}% daily vol/LP", ratio));
    }

    let age_days = pair_age_secs.map(|s| s as f64 / 86400.0);
    match age_days {
        Some(d) if d <= 7.0 => {
            score += 10.0;
            reasons.push(format!("new pool ({:.0}d)", d.max(0.0)));
        }
        Some(d) if d <= 30.0 => {
            score += 5.0;
            reasons.push(format!("{:.0}d old", d));
        }
        _ => {}
    }

    let vol = price_change_24h.abs();
    score -= (vol / 50.0 * 25.0).min(25.0);
    if vol > 30.0 {
        reasons.push(format!("⚠️ {:.0}% 24h move (IL risk)", price_change_24h));
    }

    (score.clamp(0.0, 100.0) as u8, reasons)
}

/// Deterministic pseudo-random price for (chain, token), drifting ±2% every 5s.
pub fn simulate_price(chain: &Chain, token: &str) -> f64 {
    let mut h: u64 = 0xcbf29ce484222325 ^ chain.chain_id;
    for b in token.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let base = 1e-6 + (h % 100_000) as f64 / 1e8;
    let bucket = chrono::Utc::now().timestamp();
    let walk_seed = (bucket as u64).wrapping_mul(0x9e3779b97f4a7c15) ^ h;
    let drift = ((walk_seed % 1000) as f64 / 1000.0 - 0.5) * 0.04;
    (base * (1.0 + drift)).max(1e-12)
}

/// Human-friendly token price, adapting precision to magnitude.
pub fn format_price(price: f64) -> String {
    if price >= 1.0 {
        format!("${:.4}", price)
    } else if price >= 0.001 {
        format!("${:.6}", price)
    } else {
        format!("${:.10}", price)
    }
}

/// Human-friendly NATIVE price (no $ — used where the unit is ETH/BNB/SOL,
/// so users never mistake native triggers for USD).
pub fn format_native(price: f64) -> String {
    if price >= 1.0 {
        format!("{:.4}", price)
    } else if price >= 0.001 {
        format!("{:.6}", price)
    } else {
        format!("{:.10}", price)
    }
}

/// Human-friendly USD amount.
pub fn format_usd(usd: f64) -> String {
    if usd >= 1_000_000.0 {
        format!("${:.1}M", usd / 1_000_000.0)
    } else if usd >= 10_000.0 {
        format!("${:.0}k", usd / 1_000.0)
    } else if usd >= 100.0 {
        format!("${:.0}", usd)
    } else {
        format!("${:.2}", usd)
    }
}

/// Human-friendly quantity with adaptive precision.
pub fn format_qty(qty: f64) -> String {
    if qty >= 1000.0 {
        format!("{:.2}", qty)
    } else if qty >= 1.0 {
        format!("{:.4}", qty)
    } else if qty >= 0.001 {
        format!("{:.6}", qty)
    } else {
        format!("{:.8}", qty)
    }
}

/// 0x…abcd short form.
pub fn short_addr(addr: &str) -> String {
    let a = addr.trim();
    if a.len() > 10 {
        format!("{}…{}", &a[..6], &a[a.len() - 4..])
    } else {
        a.to_string()
    }
}

/// A token search hit (DexScreener /search).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub chain: String,
    pub address: String,
    pub name: String,
    pub symbol: String,
    pub price_usd: f64,
    pub liquidity_usd: f64,
    pub volume_24h_usd: f64,
    pub price_change_24h: f64,
}

/// A boosted token (DexScreener /token-boosts).
#[derive(Debug, Clone, serde::Serialize)]
pub struct BoostedToken {
    pub chain: String,
    pub address: String,
    pub amount: f64,
    pub total_amount: f64,
}

/// A token worth providing liquidity for, scored for LP-friendliness
/// (liquidity depth + fee yield vs impermanent-loss risk).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LpSuggestion {
    pub chain: String,
    pub address: String,
    pub name: String,
    pub symbol: String,
    pub liquidity_usd: f64,
    pub volume_24h_usd: f64,
    pub price_change_24h: f64,
    pub pair_age_days: Option<f64>,
    /// 0-100, higher = better LP candidate.
    pub score: u8,
    pub reasons: Vec<String>,
}

/// A freshly created liquidity pool (DexScreener /latest/dex/pairs).
#[derive(Debug, Clone, serde::Serialize)]
pub struct NewPair {
    pub chain: String,
    pub token_address: String,
    pub symbol: String,
    pub pair_address: String,
    pub liquidity_usd: f64,
    pub volume_24h_usd: f64,
    pub pair_age_secs: Option<i64>,
}

impl MarketData {
    /// Full-text token search across chains (DexScreener /search).
    /// Results are deduplicated per (chain, token) keeping the most liquid pair.
    pub async fn search(&self, q: &str) -> Vec<SearchResult> {
        if !self.live || q.trim().is_empty() {
            return Vec::new();
        }
        // Minimal URL encoding: spaces → %20, drop other unsafe chars.
        let qq: String = q
            .trim()
            .chars()
            .map(|c| {
                if c == ' ' {
                    "%20".to_string()
                } else if c.is_ascii_alphanumeric() {
                    c.to_string()
                } else {
                    String::new()
                }
            })
            .collect();
        if qq.is_empty() {
            return Vec::new();
        }
        let url = format!("https://api.dexscreener.com/latest/dex/search?q={qq}");
        let resp: DexPairsResponse = match self.http.get(&url).send().await {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(_) => return Vec::new(),
        };

        let mut seen: std::collections::HashMap<(String, String), usize> =
            std::collections::HashMap::new();
        let mut out: Vec<SearchResult> = Vec::new();
        for p in resp.pairs {
            let Some(base) = &p.base_token else { continue };
            let Some(address) = base.address.clone() else {
                continue;
            };
            let chain = p.chain_id.clone().unwrap_or_default();
            if chain.is_empty() {
                continue;
            }
            let price = p
                .price_usd
                .as_ref()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let liq = p.liquidity.as_ref().and_then(|l| l.usd).unwrap_or(0.0);
            let key = (chain.clone(), address.to_lowercase());
            let hit = SearchResult {
                chain,
                address,
                name: base.name.clone().unwrap_or_default(),
                symbol: base.symbol.clone().unwrap_or_default(),
                price_usd: price,
                liquidity_usd: liq,
                volume_24h_usd: p.volume.as_ref().and_then(|v| v.h24).unwrap_or(0.0),
                price_change_24h: p.price_change.as_ref().and_then(|c| c.h24).unwrap_or(0.0),
            };
            match seen.get(&key) {
                Some(idx) if out[*idx].liquidity_usd < liq => out[*idx] = hit,
                Some(_) => {}
                None => {
                    seen.insert(key, out.len());
                    out.push(hit);
                }
            }
        }
        out.sort_by(|a, b| {
            b.liquidity_usd
                .partial_cmp(&a.liquidity_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out.truncate(15);
        out
    }

    /// Top boosted tokens (DexScreener /token-boosts/top/v1).
    pub async fn boosts(&self) -> Vec<BoostedToken> {
        if !self.live {
            return Vec::new();
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Boost {
            #[serde(default)]
            chain_id: Option<String>,
            #[serde(default)]
            token_address: Option<String>,
            #[serde(default)]
            amount: Option<f64>,
            #[serde(default)]
            total_amount: Option<f64>,
        }
        let url = "https://api.dexscreener.com/token-boosts/top/v1";
        let boosts: Vec<Boost> = match self.http.get(url).send().await {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(_) => return Vec::new(),
        };
        boosts
            .into_iter()
            .filter_map(|b| {
                Some(BoostedToken {
                    chain: b.chain_id?,
                    address: b.token_address?,
                    amount: b.amount.unwrap_or(0.0),
                    total_amount: b.total_amount.unwrap_or(0.0),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lp_score_ranks_sane_pools_higher() {
        // Deep liquidity, strong fee yield, calm price: best candidate.
        let (good, _) = lp_score(200_000.0, 60_000.0, 8.0, Some(3 * 86400));
        assert!(good >= 60, "good pool scored {good}");
        // Thin or wildly volatile pools score low.
        let (thin, _) = lp_score(2_000.0, 500.0, 5.0, Some(86400));
        assert!(thin < 25, "thin pool scored {thin}");
        let (wild, _) = lp_score(200_000.0, 60_000.0, 120.0, Some(3 * 86400));
        assert!(wild < good, "volatile pool should rank below calm");
        // Scores are bounded.
        let (max, _) = lp_score(5_000_000.0, 5_000_000.0, 0.0, Some(86400));
        assert!(max <= 100);
        assert!(lp_score(0.0, 0.0, 0.0, None).0 <= 100);
    }

    #[test]
    fn parses_dexscreener_payload() {
        let json = r#"{"pairs":[
          {"chainId":"bsc","dexId":"pancakeswap","pairAddress":"0x1111111111111111111111111111111111111111",
           "baseToken":{"address":"0xabc","name":"Test Token","symbol":"TST"},
           "quoteToken":{"address":"0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c","symbol":"WBNB"},
           "priceNative":"0.00012345","priceUsd":"0.07407",
           "txns":{"h24":{"buys":1200,"sells":300}},
           "volume":{"h24":523000.5},"priceChange":{"h24":45.6},
           "liquidity":{"usd":250000.0},"fdv":3000000.0,"pairCreatedAt":1712345678},
          {"chainId":"ethereum","dexId":"uniswap","pairAddress":"0x2222222222222222222222222222222222222222",
           "baseToken":{"address":"0xabc","name":"Test Token","symbol":"TST"},
           "priceNative":"0.000001","priceUsd":"0.0035","liquidity":{"usd":500.0}}
        ]}"#;
        let resp: DexPairsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.pairs.len(), 2);
        let best = resp
            .pairs
            .iter()
            .find(|p| p.chain_id.as_deref() == Some("bsc"))
            .unwrap();
        assert_eq!(
            best.pair_address.as_deref(),
            Some("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            best.txns.as_ref().unwrap().h24.as_ref().unwrap().buys,
            Some(1200)
        );
        assert_eq!(best.liquidity.as_ref().unwrap().usd, Some(250000.0));
        assert_eq!(best.pair_created_at, Some(1712345678));
        assert_eq!(best.price_change.as_ref().unwrap().h24, Some(45.6));
    }

    #[test]
    fn format_helpers() {
        assert_eq!(format_price(1234.5), "$1234.5000");
        assert_eq!(format_price(0.000123), "$0.0001230000");
        assert_eq!(format_usd(2_500_000.0), "$2.5M");
        assert_eq!(format_usd(500.0), "$500");
        assert_eq!(format_qty(0.0000000123), "0.00000001");
        assert_eq!(
            short_addr("0xabcdef1234567890abcdef1234567890abcdef12"),
            "0xabcd…ef12"
        );
    }

    #[test]
    fn simulator_is_stable_and_bounded() {
        let eth = crate::chains::by_id("ethereum").unwrap();
        let p1 = simulate_price(eth, "0xabc");
        let p2 = simulate_price(eth, "0xabc");
        assert!((p1 - p2).abs() < 1.0); // bounded drift
        assert!(p1 > 0.0);
        let other = simulate_price(eth, "0xdef");
        assert_ne!(format!("{:.10}", p1), format!("{:.10}", other));
    }
}
