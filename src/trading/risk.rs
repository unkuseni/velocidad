//! Pre-trade risk validation. Mirrors what a production Trojan-style bot needs:
//! position sizing, slippage caps, token sanity checks.

use anyhow::{bail, Result};

use crate::chains::{Chain, ChainKind};

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
    pub fn validate_trade(
        &self,
        chain: &Chain,
        token_address: &str,
        amount: f64,
        slippage: f64,
        is_sell: bool,
    ) -> Result<()> {
        if !amount.is_finite() || amount <= 0.0 {
            bail!("amount must be a positive number");
        }
        // Max-order size guards NATIVE spend on buys. Sells are bounded by
        // what the user actually holds, so the cap must not apply (a token
        // position can easily be worth more than 5 native).
        if !is_sell && amount > self.max_order_eth {
            bail!(
                "order exceeds max size of {} {}",
                self.max_order_eth,
                chain.native
            );
        }
        if !slippage.is_finite() || slippage < 0.0 || slippage > self.max_slippage {
            bail!(
                "slippage must be between 0 and {}%",
                self.max_slippage * 100.0
            );
        }
        match chain.kind {
            ChainKind::Evm => {
                let t = token_address.trim().trim_start_matches("0x");
                if t.len() != 40 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
                    bail!("invalid token address: expected a 0x… hex address (40 hex chars)");
                }
            }
            ChainKind::Solana => {
                let t = token_address.trim();
                let ok = match bs58::decode(t).into_vec() {
                    Ok(b) => b.len() == 32,
                    Err(_) => false,
                };
                if !ok {
                    bail!("invalid token address: expected a base58 Solana mint (32 bytes)");
                }
            }
        }
        Ok(())
    }
}
