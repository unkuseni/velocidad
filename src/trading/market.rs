//! Deterministic mock market used by paper trading.
//!
//! Every token address maps to a stable-ish price that drifts slightly every
//! few seconds, so `/price`, buys and sells behave like a real (if synthetic)
//! market. Replace `price()` with a real price-feed/RPC call to go live.

use chrono::Utc;

pub struct MarketSimulator;

impl MarketSimulator {
    pub fn new() -> Self {
        Self
    }

    /// Stable seed derived from the token address.
    fn seed(token: &str) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in token.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Simulated price: `base_price * (1 + walk)`, refreshed every 5 seconds.
    pub fn price(&self, token: &str) -> f64 {
        let seed = Self::seed(token);
        let base = 1e-6 + (seed % 100_000) as f64 / 1e8; // 1e-6 .. ~1e-3

        // time-bucketed random walk so price moves but isn't erratic
        let bucket = Utc::now().timestamp();
        let walk_seed = (bucket as u64).wrapping_mul(0x9e3779b97f4a7c15) ^ seed;
        let drift = ((walk_seed % 1000) as f64 / 1000.0 - 0.5) * 0.04; // ±2%

        (base * (1.0 + drift)).max(1e-12)
    }

    /// Human-friendly price string, adapting precision to magnitude.
    pub fn format_price(price: f64) -> String {
        if price >= 1.0 {
            format!("${:.4}", price)
        } else if price >= 0.001 {
            format!("${:.6}", price)
        } else {
            format!("${:.10}", price)
        }
    }
}

impl Default for MarketSimulator {
    fn default() -> Self {
        Self::new()
    }
}
