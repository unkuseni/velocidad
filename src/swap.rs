//! Live swaps via the 0x Swap API v2 (Permit2 endpoint) — optional, enabled
//! with `ZEROEX_API_KEY`.
//!
//! Flow:
//!   1. `GET /swap/permit2/quote?chainId=…&sellToken=…&buyToken=…&sellAmount=…&taker=…`
//!      with headers `0x-api-key` + `0x-version: v2`.
//!   2. For ERC20 sells: approve the allowance target from `issues.allowance.spender`.
//!   3. Build an EIP-1559 transaction from `transaction.to/data/gas/gasPrice/value`,
//!      sign it with the user's private key (k256 + keccak), broadcast via RPC,
//!      wait for the receipt.

use std::time::Duration;

use k256::ecdsa::{Signature, SigningKey};
use serde::Deserialize;

use crate::chains::Chain;
use crate::rpc::RpcClient;

/// Parsed 0x v2 quote with everything needed to build + sign the tx.
#[derive(Debug, Clone)]
pub struct SwapQuote {
    pub buy_amount: u128,
    pub sell_amount: u128,
    #[allow(dead_code)]
    pub min_buy_amount: Option<u128>,
    /// Entry-point contract for the swap payload.
    pub to: String,
    /// Swap calldata (hex, 0x-prefixed).
    pub data: String,
    /// Gas limit (decimal string in the API).
    pub gas: u64,
    /// Gas price suggested by the API (wei).
    pub gas_price: u128,
    /// Value to send with the tx (wei).
    pub value: u128,
    /// Contract that must hold an ERC20 allowance for sells.
    pub allowance_target: Option<String>,
    /// Whether the API reports the taker has enough balance.
    #[allow(dead_code)]
    pub balance_ok: bool,
    /// Effective token price in native units (sell/buy for buys).
    pub price_native: f64,
}

#[derive(Deserialize, Default)]
struct QuoteResponse {
    #[serde(default)]
    buy_amount: Option<String>,
    #[serde(default)]
    sell_amount: Option<String>,
    #[serde(default)]
    min_buy_amount: Option<String>,
    #[serde(default)]
    liquidity_available: Option<bool>,
    #[serde(default)]
    issues: Option<QuoteIssues>,
    #[serde(default)]
    transaction: Option<QuoteTransaction>,
    #[serde(default)]
    error: Option<QuoteError>,
}

#[derive(Deserialize, Default)]
struct QuoteIssues {
    #[serde(default)]
    allowance: Option<QuoteAllowance>,
    #[serde(default)]
    balance: Option<QuoteBalance>,
}

#[derive(Deserialize, Default)]
struct QuoteAllowance {
    #[serde(default)]
    spender: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    amount: Option<String>,
}

#[derive(Deserialize, Default)]
struct QuoteBalance {
    #[serde(default)]
    amount: Option<String>,
}

#[derive(Deserialize)]
struct QuoteTransaction {
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    gas: Option<String>,
    #[serde(default)]
    gas_price: Option<String>,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Deserialize)]
struct QuoteError {
    #[serde(default)]
    #[allow(dead_code)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    reasons: Option<serde_json::Value>,
}

/// Swap client: 0x quotes + RPC broadcast.
pub struct SwapClient {
    http: reqwest::Client,
    api_key: Option<String>,
    rpc: RpcClient,
}

impl SwapClient {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("failed to build 0x HTTP client"),
            api_key,
            rpc: RpcClient::new(),
        }
    }

    /// Live swaps require an API key configured at startup.
    pub fn enabled(&self) -> bool {
        self.api_key.as_ref().map(|k| !k.trim().is_empty()).unwrap_or(false)
    }

    /// Fetch a 0x v2 quote.
    ///
    /// - `sell_token`: 0x address, or the native sentinel
    /// - `buy_token`: 0x address, or the native sentinel
    /// - `sell_amount_wei`: amount to sell, in the token's smallest unit
    /// - `slippage_bps`: allowed slippage in basis points
    pub async fn quote(
        &self,
        chain: &Chain,
        sell_token: &str,
        buy_token: &str,
        sell_amount_wei: u128,
        taker: &str,
        slippage_bps: u64,
    ) -> anyhow::Result<SwapQuote> {
        let key = self
            .api_key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("0x API key not configured (ZEROEX_API_KEY)"))?;

        let url = format!(
            "https://api.0x.org/swap/permit2/quote?chainId={}&sellToken={}&buyToken={}&sellAmount={}&taker={}&slippageBps={}",
            chain.chain_id, sell_token, buy_token, sell_amount_wei, taker, slippage_bps
        );
        let resp = self
            .http
            .get(&url)
            .header("0x-api-key", &key)
            .header("0x-version", "v2")
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<QuoteResponse>(&text) {
                if let Some(e) = err.error {
                    let msg = e
                        .message
                        .or(e.description)
                        .unwrap_or_else(|| "unknown 0x error".to_string());
                    anyhow::bail!("0x API {status}: {msg}");
                }
            }
            anyhow::bail!("0x API returned {status}: {}", text.chars().take(300).collect::<String>());
        }

        let parsed: QuoteResponse = serde_json::from_str(&text)?;
        let tx = parsed
            .transaction
            .ok_or_else(|| anyhow::anyhow!("0x quote missing transaction"))?;
        let to = tx.to.ok_or_else(|| anyhow::anyhow!("0x quote missing transaction.to"))?;
        let data = tx.data.ok_or_else(|| anyhow::anyhow!("0x quote missing transaction.data"))?;

        let buy_amount = parse_quantity(parsed.buy_amount.as_deref().unwrap_or("0"))?;
        let sell_amount = parse_quantity(parsed.sell_amount.as_deref().unwrap_or("0"))?;
        let min_buy_amount = parsed
            .min_buy_amount
            .as_deref()
            .map(parse_quantity)
            .transpose()?;
        let gas = parse_quantity(tx.gas.as_deref().unwrap_or("200000"))? as u64;
        let gas_price = parse_quantity(tx.gas_price.as_deref().unwrap_or("0"))?;
        let value = parse_quantity(tx.value.as_deref().unwrap_or("0"))?;

        let allowance_target = parsed.issues.as_ref().and_then(|i| i.allowance.as_ref()).and_then(|a| a.spender.clone());
        let balance_ok = parsed.issues.as_ref().and_then(|i| i.balance.as_ref()).map(|b| b.amount.is_none()).unwrap_or(true);
        if parsed.liquidity_available == Some(false) {
            anyhow::bail!("0x API: no liquidity available for this pair");
        }
        if !balance_ok {
            anyhow::bail!("0x API: taker balance too low for this swap");
        }

        // Effective price in native units: for buys sellAmount is native.
        let price_native = if buy_amount > 0 { sell_amount as f64 / buy_amount as f64 } else { 0.0 };

        Ok(SwapQuote {
            buy_amount,
            sell_amount,
            min_buy_amount,
            to,
            data,
            gas,
            gas_price,
            value,
            allowance_target,
            balance_ok: true,
            price_native,
        })
    }
}

/// Parse a quantity that may be decimal or hex, as a string.
pub fn parse_quantity(s: &str) -> anyhow::Result<u128> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x") {
        return u128::from_str_radix(hex, 16).map_err(|e| anyhow::anyhow!("bad hex quantity {t}: {e}"));
    }
    t.parse::<u128>().map_err(|e| anyhow::anyhow!("bad quantity {t}: {e}"))
}

/// 0x sentinel address for the native coin.
pub const NATIVE: &str = "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE";

/// EIP-1559 transaction parameters.
pub struct Tx1559 {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee: u128,
    pub max_fee: u128,
    pub gas: u64,
    pub to: String,
    pub value: u128,
    pub data: Vec<u8>,
}

/// Result of a live swap, ready to render.
#[derive(Debug, Clone)]
pub struct LiveTradeResult {
    pub tx_hash: String,
    pub success: bool,
    /// Tokens received (buys) or sold (sells), smallest unit.
    pub token_amount: u128,
    /// Native amount spent (buys) or received (sells), wei.
    pub native_amount: u128,
    /// Effective token price in native units.
    pub price_native: f64,
    /// Approval tx hash, when one had to be sent first.
    pub approval_tx: Option<String>,
    pub explorer_link: String,
}

/// Sign an EIP-1559 transaction with a 32-byte secret key.
/// Returns `0x02 || rlp([chain_id, nonce, prio, max_fee, gas, to, value, data, [], y_parity, r, s])`.
pub fn sign_1559(tx: &Tx1559, secret: &[u8; 32]) -> anyhow::Result<String> {
    use k256::ecdsa::signature::hazmat::PrehashSigner;
    use sha3::{Digest, Keccak256};

    let to = decode_address(&tx.to)?;

    let mut unsigned = rlp::RlpStream::new_list(9);
    append_u(&mut unsigned, tx.chain_id);
    append_u(&mut unsigned, tx.nonce);
    append_u(&mut unsigned, tx.max_priority_fee);
    append_u(&mut unsigned, tx.max_fee);
    append_u(&mut unsigned, tx.gas);
    append_u(&mut unsigned, to.clone());
    append_u(&mut unsigned, tx.value);
    append_u(&mut unsigned, tx.data.clone());
    unsigned.begin_list(0); // access list

    let mut payload = Vec::with_capacity(1 + unsigned.as_raw().len());
    payload.push(0x02);
    payload.extend_from_slice(unsigned.as_raw());

    let hash: [u8; 32] = Keccak256::digest(&payload).into();

    let sk = SigningKey::from_bytes(secret.into()).map_err(|_| anyhow::anyhow!("invalid signing key"))?;
    let sig: Signature = sk
        .sign_prehash(&hash)
        .map_err(|e| anyhow::anyhow!("signing failed: {e}"))?;
    let sig = sig.normalize_s().unwrap_or(sig);
    let y_parity = recovery_y_parity(&sk, &sig, &hash)?;

    let r = sig.r().to_bytes();
    let s = sig.s().to_bytes();

    // 12 fields: chain_id, nonce, prio, max_fee, gas, to, value, data, access_list, y_parity, r, s
    let mut signed = rlp::RlpStream::new_list(12);
    append_u(&mut signed, tx.chain_id);
    append_u(&mut signed, tx.nonce);
    append_u(&mut signed, tx.max_priority_fee);
    append_u(&mut signed, tx.max_fee);
    append_u(&mut signed, tx.gas);
    append_u(&mut signed, to);
    append_u(&mut signed, tx.value);
    append_u(&mut signed, tx.data.clone());
    signed.begin_list(0);
    append_u(&mut signed, y_parity);
    append_u(&mut signed, r.to_vec());
    append_u(&mut signed, s.to_vec());

    let mut raw = Vec::with_capacity(1 + signed.as_raw().len());
    raw.push(0x02);
    raw.extend_from_slice(signed.as_raw());
    Ok(format!("0x{}", hex::encode(raw)))
}

/// Compute the ECDSA y-parity recovery bit for a signature by recovering
/// the ephemeral point R = r^-1 (sG - eP) and reading the parity of its y.
/// (k256 has no recoverable-signing feature; this is the standard derivation.)
fn recovery_y_parity(
    sk: &SigningKey,
    sig: &Signature,
    hash: &[u8; 32],
) -> anyhow::Result<u64> {
    use k256::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
    use k256::elliptic_curve::PrimeField;
    use k256::{AffinePoint, ProjectivePoint, Scalar};

    let e = Option::<Scalar>::from(Scalar::from_repr((*hash).into())).unwrap_or(Scalar::ZERO);
    let r = Scalar::from(*sig.r());
    let s = Scalar::from(*sig.s());

    let enc = sk.verifying_key().to_encoded_point(false);
    let pk = AffinePoint::from_encoded_point(&enc).unwrap();
    let r_inv = Option::<Scalar>::from(r.invert())
        .ok_or_else(|| anyhow::anyhow!("signature r not invertible"))?;

    // R = r^-1 (sG - eP); read the parity of R.y from the SEC1 encoding.
    let r_point = (ProjectivePoint::GENERATOR * s - ProjectivePoint::from(pk) * e) * r_inv;
    let enc = r_point.to_affine().to_encoded_point(false);
    let bytes = enc.as_bytes();
    if bytes.len() < 65 {
        anyhow::bail!("recovered point is the identity");
    }
    Ok((bytes[64] & 1) as u64)
}

/// Append any value that implements rlp's Encodable.
fn append_u<T: rlp::Encodable>(stream: &mut rlp::RlpStream, value: T) {
    stream.append(&value);
}

impl SwapClient {
    /// Sign + broadcast a tx and wait for its receipt.
    /// Returns (tx hash, success).
    pub async fn broadcast(
        &self,
        chain: &Chain,
        tx: &Tx1559,
        secret: &[u8; 32],
    ) -> anyhow::Result<(String, bool)> {
        let actual = self.rpc.chain_id(chain).await?;
        if actual != chain.chain_id {
            anyhow::bail!("RPC chain id mismatch: expected {}, got {}", chain.chain_id, actual);
        }
        let raw = sign_1559(tx, secret)?;
        let hash = self.rpc.send_raw_transaction(chain, &raw).await?;
        let receipt = self
            .rpc
            .wait_for_receipt(chain, &hash, Duration::from_secs(90))
            .await?;
        let status = receipt
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("0x0");
        let ok = status == "0x1" || status == "1";
        tracing::info!(tx_hash = %hash, success = ok, "live tx mined");
        Ok((hash, ok))
    }

    /// (priority_fee, max_fee) from the RPC, with a floor above the quote price.
    async fn fees(&self, chain: &Chain, quote_gas_price: u128) -> (u128, u128) {
        let priority = self.rpc.priority_fee(chain).await;
        let gas_price = self.rpc.gas_price(chain).await.unwrap_or(quote_gas_price);
        let floor = priority + 1_000_000_000; // at least 1 gwei above priority
        let max_fee = (gas_price * 2).max(quote_gas_price * 2).max(floor);
        (priority.min(max_fee), max_fee)
    }

    /// Send ERC20 approve(spender, amount) and wait for it to mine.
    pub async fn approve(
        &self,
        chain: &Chain,
        token: &str,
        spender: &str,
        amount_wei: u128,
        owner: &str,
        secret: &[u8; 32],
    ) -> anyhow::Result<String> {
        let mut data = hex::decode("095ea7b3")?; // approve(address,uint256)
        let mut arg = [0u8; 32];
        arg[12..].copy_from_slice(&decode_address(spender)?);
        data.extend_from_slice(&arg);
        let mut amount = [0u8; 32];
        amount[16..].copy_from_slice(&amount_wei.to_be_bytes());
        data.extend_from_slice(&amount);

        let data_hex = format!("0x{}", hex::encode(&data));
        let gas = self
            .rpc
            .estimate_gas(chain, owner, token, "0x0", &data_hex)
            .await
            .unwrap_or(100_000)
            .max(60_000);
        let (priority, max_fee) = self.fees(chain, 0).await;
        let nonce = self.rpc.nonce(chain, owner).await?;
        let tx = Tx1559 {
            chain_id: chain.chain_id,
            nonce,
            max_priority_fee: priority,
            max_fee,
            gas: gas as u64,
            to: token.to_string(),
            value: 0,
            data,
        };
        let (hash, ok) = self.broadcast(chain, &tx, secret).await?;
        if !ok {
            anyhow::bail!("approval reverted: {hash}");
        }
        Ok(hash)
    }

    /// Ensure the owner has approved at least `needed_wei` to `spender`.
    /// Returns the approval tx hash when one was sent.
    pub async fn ensure_allowance(
        &self,
        chain: &Chain,
        token: &str,
        spender: &str,
        needed_wei: u128,
        owner: &str,
        secret: &[u8; 32],
    ) -> anyhow::Result<Option<String>> {
        let current = self
            .rpc
            .erc20_allowance(chain, token, owner, spender)
            .await
            .unwrap_or(0);
        if current >= needed_wei {
            return Ok(None);
        }
        let hash = self.approve(chain, token, spender, needed_wei, owner, secret).await?;
        Ok(Some(hash))
    }

    /// Execute a 0x quote. When `approve_token` is set, ensures allowance
    /// for that token first (ERC20 sells).
    pub async fn execute_swap(
        &self,
        chain: &Chain,
        quote: &SwapQuote,
        approve_token: Option<&str>,
        owner: &str,
        secret: &[u8; 32],
    ) -> anyhow::Result<(String, bool, Option<String>)> {
        let approval = if let (Some(token), Some(target)) = (approve_token, &quote.allowance_target) {
            self.ensure_allowance(chain, token, target, quote.sell_amount, owner, secret)
                .await?
        } else {
            None
        };

        let (priority, max_fee) = self.fees(chain, quote.gas_price).await;
        let nonce = self.rpc.nonce(chain, owner).await?;
        let data = hex::decode(quote.data.trim_start_matches("0x"))?;
        let tx = Tx1559 {
            chain_id: chain.chain_id,
            nonce,
            max_priority_fee: priority,
            max_fee,
            gas: ((quote.gas as f64) * 1.2) as u64,
            to: quote.to.clone(),
            value: quote.value,
            data,
        };
        let (hash, ok) = self.broadcast(chain, &tx, secret).await?;
        Ok((hash, ok, approval))
    }

    /// Live buy: spend `amount_native_wei` of the chain's native coin for a token.
    pub async fn buy_native(
        &self,
        chain: &Chain,
        token_address: &str,
        amount_native_wei: u128,
        slippage: f64,
        taker: &str,
        secret: &[u8; 32],
    ) -> anyhow::Result<LiveTradeResult> {
        let bps = (slippage * 10_000.0) as u64;
        let quote = self
            .quote(chain, NATIVE, token_address, amount_native_wei, taker, bps)
            .await?;
        let (hash, ok, _) = self.execute_swap(chain, &quote, None, taker, secret).await?;
        Ok(LiveTradeResult {
            tx_hash: hash.clone(),
            success: ok,
            token_amount: quote.buy_amount,
            native_amount: quote.sell_amount,
            price_native: quote.price_native,
            approval_tx: None,
            explorer_link: chain.explorer_link(&hash),
        })
    }

    /// Live sell: swap a token quantity for native coin.
    pub async fn sell_tokens(
        &self,
        chain: &Chain,
        token_address: &str,
        quantity: f64,
        slippage: f64,
        taker: &str,
        secret: &[u8; 32],
    ) -> anyhow::Result<LiveTradeResult> {
        if quantity <= 0.0 {
            anyhow::bail!("quantity must be positive");
        }
        let decimals = self.rpc.erc20_decimals(chain, token_address).await;
        let amount_wei = (quantity * 10f64.powi(decimals as i32)) as u128;
        if amount_wei == 0 {
            anyhow::bail!("quantity too small for token decimals");
        }
        let bps = (slippage * 10_000.0) as u64;
        let quote = self
            .quote(chain, token_address, NATIVE, amount_wei, taker, bps)
            .await?;
        let (hash, ok, approval) = self
            .execute_swap(chain, &quote, Some(token_address), taker, secret)
            .await?;
        Ok(LiveTradeResult {
            tx_hash: hash.clone(),
            success: ok,
            token_amount: quote.sell_amount,
            native_amount: quote.buy_amount,
            price_native: 1.0 / quote.price_native.max(1e-18),
            approval_tx: approval,
            explorer_link: chain.explorer_link(&hash),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantity_parsing() {
        assert_eq!(parse_quantity("100000").unwrap(), 100000);
        assert_eq!(parse_quantity("0x10").unwrap(), 16);
        assert_eq!(parse_quantity("0").unwrap(), 0);
        assert!(parse_quantity("abc").is_err());
    }

    #[test]
    fn address_decoding() {
        let b = decode_address("0x3535353535353535353535353535353535353535").unwrap();
        assert_eq!(b.len(), 20);
        assert!(decode_address("0x1234").is_err());
        assert!(decode_address("zzz").is_err());
    }

    /// The unsigned payload of the EIP-1559 specification example must match
    /// exactly: 0x02 || rlp([1, 0, 0x3b9aca00, 0x2540be400, 0x5208, 0x3535…, 0xde0b6b3a7640000, [], []])
    #[test]
    fn eip1559_unsigned_payload() {
        let tx = Tx1559 {
            chain_id: 1,
            nonce: 0,
            max_priority_fee: 0x3b9aca00,
            max_fee: 0x2540be400,
            gas: 0x5208,
            to: "0x3535353535353535353535353535353535353535".to_string(),
            value: 0xde0b6b3a7640000,
            data: vec![],
        };
        let to = decode_address(&tx.to).unwrap();
        let mut unsigned = rlp::RlpStream::new_list(9);
        append_u(&mut unsigned, tx.chain_id);
        append_u(&mut unsigned, tx.nonce);
        append_u(&mut unsigned, tx.max_priority_fee);
        append_u(&mut unsigned, tx.max_fee);
        append_u(&mut unsigned, tx.gas);
        append_u(&mut unsigned, to);
        append_u(&mut unsigned, tx.value);
        append_u(&mut unsigned, tx.data);
        unsigned.begin_list(0);
        let mut payload = vec![0x02u8];
        payload.extend_from_slice(unsigned.as_raw());
        // 48-byte payload → short list header 0xf0 (verified against ethers.js: the
        // signed variant uses 0xf873 for the same 9 fields + signature).
        assert_eq!(
            hex::encode(&payload),
            "02f00180843b9aca008502540be400825208943535353535353535353535353535353535353535880de0b6b3a764000080c0"
        );
    }

    /// Signing the spec example with a fixed key must produce a known
    /// (r, s, y_parity) — cross-checked against ethers.js.
    #[test]
    fn eip1559_signature_golden() {
        let tx = Tx1559 {
            chain_id: 1,
            nonce: 0,
            max_priority_fee: 0x3b9aca00,
            max_fee: 0x2540be400,
            gas: 0x5208,
            to: "0x3535353535353535353535353535353535353535".to_string(),
            value: 0xde0b6b3a7640000,
            data: vec![],
        };
        // Known private key (ethers.js test fixture)
        let secret: [u8; 32] = hex::decode("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap()
            .try_into()
            .unwrap();
        let raw = sign_1559(&tx, &secret).unwrap();
        // Golden vector generated with ethers.js v6 (independent implementation):
        // Wallet(0x00...01).signTransaction(specExample)
        assert_eq!(
            raw,
            "0x02f8730180843b9aca008502540be400825208943535353535353535353535353535353535353535880de0b6b3a764000080c080a09f6cbc7079332058214939f12c1fcbd740e01be24c18f7ff51e9d9921ca613e9a0228186c3562c5bcdc869bf4e7cafc0dd5fd9b22141c876ec397b037b812869fa"
        );
    }
}
/// Decode a 0x address into its 20 raw bytes.
pub fn decode_address(addr: &str) -> anyhow::Result<Vec<u8>> {
    let cleaned = addr.trim().trim_start_matches("0x");
    let bytes = hex::decode(cleaned).map_err(|e| anyhow::anyhow!("bad address {addr}: {e}"))?;
    if bytes.len() != 20 {
        anyhow::bail!("address {addr} must be 20 bytes");
    }
    Ok(bytes)
}
