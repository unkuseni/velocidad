//! Minimal EVM JSON-RPC client.
//!
//! Enough for a trading bot: native/ERC20 balances, nonce, gas estimates,
//! raw transaction broadcast and receipt polling. Tries each of the chain's
//! public RPC endpoints in order until one answers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::chains::Chain;

#[allow(dead_code)]
const NATIVE_ADDRESS: &str = "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE";

/// Retry rounds per RPC call; each round walks all endpoints, then backs off.
const RPC_ATTEMPTS: u32 = 3;
const RPC_BACKOFF_BASE: Duration = Duration::from_millis(200);

pub struct RpcClient {
    http: reqwest::Client,
    next_id: AtomicU64,
}

impl RpcClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("failed to build RPC HTTP client"),
            next_id: AtomicU64::new(1),
        }
    }

    async fn call(&self, chain: &Chain, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut attempt: u32 = 0;
        loop {
            let mut last_err: Option<anyhow::Error> = None;
            for url in chain.rpc_urls {
                match self.http.post(*url).json(&body).send().await {
                    Ok(resp) => match resp.json::<Value>().await {
                        Ok(v) => {
                            if let Some(err) = v.get("error") {
                                last_err = Some(anyhow::anyhow!("rpc error: {err}"));
                                continue;
                            }
                            if let Some(result) = v.get("result") {
                                return Ok(result.clone());
                            }
                            last_err = Some(anyhow::anyhow!("rpc returned no result: {v}"));
                        }
                        Err(e) => last_err = Some(anyhow::anyhow!("rpc json: {e}")),
                    },
                    Err(e) => last_err = Some(anyhow::anyhow!("rpc transport {url}: {e}")),
                }
            }
            attempt += 1;
            if attempt >= RPC_ATTEMPTS {
                return Err(last_err
                    .unwrap_or_else(|| anyhow::anyhow!("no RPC endpoints for {}", chain.id)));
            }
            let backoff = RPC_BACKOFF_BASE * 2u32.pow(attempt - 1);
            tracing::debug!(
                chain = chain.id,
                method,
                attempt,
                backoff_ms = backoff.as_millis(),
                "rpc call failed — retrying with backoff"
            );
            tokio::time::sleep(backoff).await;
        }
    }

    /// Native balance of an address, in wei.
    pub async fn native_balance(&self, chain: &Chain, address: &str) -> anyhow::Result<u128> {
        let v = self
            .call(chain, "eth_getBalance", json!([address, "latest"]))
            .await?;
        hex_u128(v)
    }

    /// ERC20 balanceOf(address) result, in raw token units.
    pub async fn erc20_balance(
        &self,
        chain: &Chain,
        token: &str,
        address: &str,
    ) -> anyhow::Result<u128> {
        let data = format!(
            "0x70a08231000000000000000000000000{}",
            address.trim_start_matches("0x")
        );
        let v = self
            .call(
                chain,
                "eth_call",
                json!([{ "to": token, "data": data }, "latest"]),
            )
            .await?;
        hex_u128(v)
    }

    /// ERC20 allowance(owner, spender), raw units.
    pub async fn erc20_allowance(
        &self,
        chain: &Chain,
        token: &str,
        owner: &str,
        spender: &str,
    ) -> anyhow::Result<u128> {
        let data = format!("0xdd62ed3e{}{}", pad32(owner), pad32(spender));
        let v = self
            .call(
                chain,
                "eth_call",
                json!([{ "to": token, "data": data }, "latest"]),
            )
            .await?;
        hex_u128(v)
    }

    /// Pending nonce for an address.
    pub async fn nonce(&self, chain: &Chain, address: &str) -> anyhow::Result<u64> {
        let v = self
            .call(
                chain,
                "eth_getTransactionCount",
                json!([address, "pending"]),
            )
            .await?;
        Ok(hex_u128(v)? as u64)
    }

    /// Current gas price (wei per gas).
    pub async fn gas_price(&self, chain: &Chain) -> anyhow::Result<u128> {
        let v = self.call(chain, "eth_gasPrice", json!([])).await?;
        hex_u128(v)
    }

    /// Suggested priority fee (wei per gas); falls back to 1 gwei.
    pub async fn priority_fee(&self, chain: &Chain) -> u128 {
        match self
            .call(chain, "eth_maxPriorityFeePerGas", json!([]))
            .await
        {
            Ok(v) => hex_u128(v).unwrap_or(1_000_000_000),
            Err(_) => 1_000_000_000,
        }
    }

    /// Estimate gas for a tx.
    pub async fn estimate_gas(
        &self,
        chain: &Chain,
        from: &str,
        to: &str,
        value: &str,
        data: &str,
    ) -> anyhow::Result<u128> {
        let v = self
            .call(
                chain,
                "eth_estimateGas",
                json!([{ "from": from, "to": to, "value": value, "data": data }]),
            )
            .await?;
        hex_u128(v)
    }

    /// Code at an address (hex, 0x-prefixed).
    pub async fn code(&self, chain: &Chain, address: &str) -> anyhow::Result<String> {
        let v = self
            .call(chain, "eth_getCode", json!([address, "latest"]))
            .await?;
        v.as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("eth_getCode returned non-string"))
    }

    /// EIP-7702 delegation target of an EOA, if delegated (0xef0100 || addr).
    pub async fn delegation_of(
        &self,
        chain: &Chain,
        address: &str,
    ) -> anyhow::Result<Option<String>> {
        let code = self.code(chain, address).await?;
        if let Some(rest) = code.strip_prefix("0xef0100") {
            if rest.len() >= 40 {
                return Ok(Some(format!("0x{}", &rest[..40])));
            }
        }
        Ok(None)
    }
    /// Broadcast a signed raw transaction; returns the tx hash.
    pub async fn send_raw_transaction(&self, chain: &Chain, raw: &str) -> anyhow::Result<String> {
        let v = self
            .call(chain, "eth_sendRawTransaction", json!([raw]))
            .await?;
        v.as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("sendRawTransaction returned non-string"))
    }

    /// Poll for a receipt until timeout; returns receipt JSON when mined.
    pub async fn wait_for_receipt(
        &self,
        chain: &Chain,
        tx_hash: &str,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Ok(v) = self
                .call(chain, "eth_getTransactionReceipt", json!([tx_hash]))
                .await
            {
                if v.is_object() {
                    return Ok(v);
                }
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!("timed out waiting for receipt {tx_hash}");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Verify the RPC endpoint chain id matches the expected chain.
    pub async fn chain_id(&self, chain: &Chain) -> anyhow::Result<u64> {
        let v = self.call(chain, "eth_chainId", json!([])).await?;
        Ok(hex_u128(v)? as u64)
    }

    /// ERC20 decimals() value; falls back to 18 when the call fails.
    /// The fallback is loud: a wrong decimals guess makes quantities off by
    /// orders of magnitude, so it must never happen silently.
    pub async fn erc20_decimals(&self, chain: &Chain, token: &str) -> u8 {
        let data = "0x313ce567";
        match self
            .call(
                chain,
                "eth_call",
                json!([{ "to": token, "data": data }, "latest"]),
            )
            .await
        {
            Ok(v) => match hex_u128(v) {
                Ok(d) => d as u8,
                Err(_) => {
                    tracing::warn!(
                        chain = chain.id,
                        token,
                        "decimals() returned garbage — assuming 18"
                    );
                    18
                }
            },
            Err(e) => {
                tracing::warn!(chain = chain.id, token, error = %e, "decimals() failed — assuming 18");
                18
            }
        }
    }
    /// Resolve a token reference to an address: native/eth/empty means the
    /// chain's native coin; anything else must be a 0x address.
    #[allow(dead_code)]
    pub fn resolve_token(&self, _chain: &Chain, token: &str) -> Option<(String, bool)> {
        let t = token.trim().to_lowercase();
        if t.is_empty()
            || t == "native"
            || t == "eth"
            || t == "bnb"
            || t == "matic"
            || t == "pol"
            || t == "avax"
            || t == "ftm"
        {
            return Some((NATIVE_ADDRESS.to_string(), true));
        }
        if t.starts_with("0x") && t.len() == 42 && t[2..].chars().all(|c| c.is_ascii_hexdigit()) {
            return Some((t, false));
        }
        None
    }
}

impl Default for RpcClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a hex quantity (0x...) into u128.
pub fn hex_u128(v: Value) -> anyhow::Result<u128> {
    let s = v
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("expected hex string, got {v}"))?;
    let cleaned = s.trim_start_matches("0x");
    if cleaned.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(cleaned, 16).map_err(|e| anyhow::anyhow!("bad hex {s}: {e}"))
}

/// Left-pad a 0x address to 32 bytes for calldata packing.
fn pad32(addr: &str) -> String {
    let a = addr.trim_start_matches("0x");
    format!("{:0>64}", a)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(hex_u128(json!("0x0")).unwrap(), 0);
        assert_eq!(
            hex_u128(json!("0xde0b6b3a7640000")).unwrap(),
            1_000_000_000_000_000_000
        );
        assert!(hex_u128(json!(5)).is_err());
    }

    #[test]
    fn calldata_padding() {
        assert_eq!(
            pad32("0x1234567890abcdef1234567890abcdef12345678"),
            "0000000000000000000000001234567890abcdef1234567890abcdef12345678"
        );
    }

    #[test]
    fn token_resolution() {
        let rpc = RpcClient::new();
        let eth = crate::chains::by_id("ethereum").unwrap();
        let (addr, native) = rpc.resolve_token(eth, "native").unwrap();
        assert!(native);
        assert!(addr.starts_with("0xEeee"));
        let (addr, native) = rpc
            .resolve_token(eth, "0x1234567890abcdef1234567890abcdef12345678")
            .unwrap();
        assert!(!native);
        assert_eq!(addr, "0x1234567890abcdef1234567890abcdef12345678");
        assert!(rpc.resolve_token(eth, "notanaddress").is_none());
    }
}
