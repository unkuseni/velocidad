//! Token security scanner (honeypot / rug-pull heuristics).
//!
//! In production this would pull real on-chain data (holder distribution via
//! RPC, contract source verification, liquidity locks, transfer taxes). Here we
//! use deterministic address-derived heuristics so the full pipeline
//! (scan → risk score → block/allow → trade) is wired up and testable without
//! chain access. Replace `scan()` internals with RPC/DexScreener calls to go live.

use crate::db::models::Token;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    pub passed: bool,
    pub note: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TokenReport {
    pub address: String,
    pub name: String,
    pub symbol: String,
    pub risk_score: u8, // 0 = safe … 100 = scam
    pub is_honeypot: bool,
    pub liquidity_eth: f64,
    pub checks: Vec<CheckResult>,
}

pub struct TokenScanner;

impl TokenScanner {
    pub fn new() -> Self {
        Self
    }

    /// Deterministic pseudo-random u64 from an address, stable across runs.
    fn seed(token: &str) -> u64 {
        let mut h: u64 = 0x9e3779b97f4a7c15;
        for b in token.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Run the scan and return a report. Also caches the result in the tokens
    /// table (with the current simulated price).
    pub async fn scan(&self, db: &crate::db::Db, token: &str, price: f64) -> TokenReport {
        let seed = Self::seed(token);
        let n = token.trim_start_matches("0x");
        let name = format!("Token {}", &n[..n.len().min(6)]);
        let symbol = format!("T{}", &n[n.len().saturating_sub(4)..]);

        let liquidity_eth = 0.5 + (seed % 10_000) as f64 / 100.0; // 0.5 .. 100.5
        let liquidity_ok = liquidity_eth >= 1.0;

        // Heuristic checks — replace with real verifications in production.
        let checks = vec![
            CheckResult {
                name: "Liquidity",
                passed: liquidity_ok,
                note: if liquidity_ok {
                    format!("{:.1} ETH locked", liquidity_eth)
                } else {
                    format!("{:.2} ETH — too low", liquidity_eth)
                },
            },
            CheckResult {
                name: "Ownership renounced",
                passed: seed % 3 != 0,
                note: if seed % 3 == 0 { "admin key still active" } else { "renounced" }
                    .to_string(),
            },
            CheckResult {
                name: "Transfer tax",
                passed: seed % 5 != 0,
                note: if seed % 5 == 0 { "suspicious buy/sell tax" } else { "≤ 5%" }.to_string(),
            },
            CheckResult {
                name: "Verified source",
                passed: seed % 4 != 0,
                note: if seed % 4 == 0 { "unverified contract" } else { "verified" }.to_string(),
            },
            CheckResult {
                name: "Honeypot pattern",
                passed: seed % 7 != 0,
                note: if seed % 7 == 0 { "can't sell — honeypot" } else { "none detected" }
                    .to_string(),
            },
        ];

        let failed = checks.iter().filter(|c| !c.passed).count();
        let risk_score = (5 + failed as u64 * 20 + seed % 5).min(100) as u8;
        let is_honeypot = seed % 7 == 0;

        let report = TokenReport {
            address: token.to_string(),
            name,
            symbol,
            risk_score,
            is_honeypot,
            liquidity_eth,
            checks,
        };

        // Cache into the tokens table.
        let _ = crate::db::repo::upsert_token(
            db.conn(),
            &Token {
                address: token.to_string(),
                network: "ethereum".into(),
                name: Some(report.name.clone()),
                symbol: Some(report.symbol.clone()),
                decimals: 18,
                risk_score: report.risk_score as i64,
                is_honeypot: report.is_honeypot,
                liquidity: Some(report.liquidity_eth),
                last_price: Some(price),
                updated_at: Some(chrono::Utc::now().to_rfc3339()),
            },
        )
        .await;

        report
    }
}

impl Default for TokenScanner {
    fn default() -> Self {
        Self::new()
    }
}
