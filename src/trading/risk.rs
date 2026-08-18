//! Pre-trade risk validation. Mirrors what a production Trojan-style bot needs:
//! position sizing, slippage caps, token sanity checks.

use anyhow::{bail, Result};

#[derive(Debug, Clone)]
pub struct RiskManager {
    /// Maximum order size in ETH (paper account).
    pub max_order_eth: f64,
    /// Maximum allowed slippage fraction.
    pub max_slippage: f64,
}

impl Default for RiskManager {
    fn default() -> Self {
        Self {
            max_order_eth: 5.0,
            max_slippage: 0.50,
        }
    }
}

impl RiskManager {
    pub fn validate_trade(&self, token_address: &str, amount: f64, slippage: f64, native: &str) -> Result<()> {
        if amount <= 0.0 {
            bail!("amount must be positive");
        }
        if amount > self.max_order_eth {
            bail!("order exceeds max size of {} {}", self.max_order_eth, native);
        }
        if slippage < 0.0 || slippage > self.max_slippage {
            bail!("slippage must be between 0 and {}%", self.max_slippage * 100.0);
        }
        let t = token_address.trim().trim_start_matches("0x");
        if t.len() < 32 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("invalid token address: expected a hex address like 0x…");
        }
        Ok(())
    }
}
