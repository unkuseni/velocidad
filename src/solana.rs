//! Solana support: JSON-RPC (SOL + SPL balances, broadcast), keyless swaps via
//! the **Jupiter v6 API** (quote → swap transaction → local ed25519 signing →
//! broadcast), and partial-transaction signing for Jupiter's single-signer
//! swap transactions.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::chains::Chain;

/// WSOL mint (wrapped SOL).
pub const WSOL: &str = "So11111111111111111111111111111111111111112";

/// Priority fee in lamports added to Jupiter swap transactions.
const PRIORITIZATION_FEE_LAMPORTS: u64 = 5000;

/// A single SPL token balance.
#[derive(Debug, Clone)]
pub struct SolBalance {
    pub mint: String,
    pub amount: f64,
    #[allow(dead_code)]
    pub decimals: u8,
}

/// Parsed Jupiter v6 quote.
#[derive(Debug, Clone)]
pub struct JupiterQuote {
    pub in_amount: u64,
    pub out_amount: u64,
    #[allow(dead_code)]
    pub price_impact_pct: f64,
    /// The full quoteResponse object — passed back to /swap verbatim.
    pub raw: Value,
}

/// Result of a live Solana swap.
#[derive(Debug, Clone)]
pub struct SolanaTradeResult {
    pub tx_signature: String,
    pub success: bool,
    pub token_amount: u64,
    pub native_amount: u64,
    pub price_native: f64,
    pub explorer_link: String,
}

pub struct SolanaClient {
    http: reqwest::Client,
}

impl SolanaClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("failed to build Solana HTTP client"),
        }
    }

    /// JSON-RPC call against the chain's public endpoints, tried in order.
    async fn rpc(&self, chain: &Chain, method: &str, params: Value) -> Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut last_err: Option<anyhow::Error> = None;
        for url in chain.rpc_urls {
            match self.http.post(*url).json(&body).send().await {
                Ok(resp) => match resp.json::<Value>().await {
                    Ok(v) => {
                        if let Some(err) = v.get("error") {
                            last_err = Some(anyhow::anyhow!("solana rpc error: {err}"));
                            continue;
                        }
                        if let Some(result) = v.get("result") {
                            return Ok(result.clone());
                        }
                    }
                    Err(e) => last_err = Some(anyhow::anyhow!("solana rpc json: {e}")),
                },
                Err(e) => last_err = Some(anyhow::anyhow!("solana rpc transport: {e}")),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no Solana RPC endpoints")))
    }

    /// Native SOL balance in lamports.
    pub async fn balance(&self, chain: &Chain, address: &str) -> Result<u64> {
        let v = self.rpc(chain, "getBalance", json!([address])).await?;
        v.get("value")
            .and_then(|x| x.as_u64())
            .context("getBalance returned no value")
    }

    /// All SPL token accounts of an address (nonzero included).
    pub async fn spl_balances(&self, chain: &Chain, address: &str) -> Result<Vec<SolBalance>> {
        let v = self
            .rpc(
                chain,
                "getTokenAccountsByOwner",
                json!([
                    address,
                    { "programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" },
                    { "encoding": "jsonParsed" },
                ]),
            )
            .await?;
        let mut out = Vec::new();
        if let Some(items) = v.get("value").and_then(|v| v.as_array()) {
            for item in items {
                let info = item
                    .pointer("/account/data/parsed/info")
                    .context("malformed token account")?;
                let mint = info
                    .get("mint")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string();
                let ta = info.get("tokenAmount").context("malformed tokenAmount")?;
                let decimals = ta.get("decimals").and_then(|d| d.as_u64()).unwrap_or(0) as u8;
                let amount = ta.get("uiAmount").and_then(|a| a.as_f64()).unwrap_or(0.0);
                if amount > 0.0 {
                    out.push(SolBalance { mint, amount, decimals });
                }
            }
        }
        Ok(out)
    }

    /// SPL mint decimals via getTokenSupply.
    pub async fn token_decimals(&self, chain: &Chain, mint: &str) -> Result<u8> {
        let v = self.rpc(chain, "getTokenSupply", json!([mint])).await?;
        Ok(v.pointer("/value/decimals")
            .and_then(|d| d.as_u64())
            .unwrap_or(9) as u8)
    }

    /// Jupiter v6 quote. `input_mint`/`output_mint` are SPL mints (WSOL for
    /// native SOL); `amount` is in the input token's smallest unit.
    pub async fn quote(
        &self,
        input_mint: &str,
        output_mint: &str,
        amount: u64,
        slippage_bps: u16,
    ) -> Result<JupiterQuote> {
        let url = format!(
            "https://quote-api.jup.ag/v6/quote?inputMint={input_mint}&outputMint={output_mint}&amount={amount}&slippageBps={slippage_bps}&onlyDirectRoutes=false"
        );
        let resp = self.http.get(&url).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("Jupiter quote failed ({status}): {}", text.chars().take(300).collect::<String>());
        }
        let raw: Value = serde_json::from_str(&text)?;
        let in_amount: u64 = raw
            .get("inAmount")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .context("Jupiter quote missing inAmount")?;
        let out_amount: u64 = raw
            .get("outAmount")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .context("Jupiter quote missing outAmount")?;
        let price_impact_pct = raw
            .get("priceImpactPct")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        Ok(JupiterQuote {
            in_amount,
            out_amount,
            price_impact_pct,
            raw,
        })
    }

    /// Build the unsigned swap transaction (base64) for a quote.
    pub async fn swap_transaction(&self, quote: &JupiterQuote, user_pubkey: &str) -> Result<String> {
        let body = json!({
            "quoteResponse": quote.raw,
            "userPublicKey": user_pubkey,
            "wrapAndUnwrapSol": true,
            "dynamicComputeUnitLimit": true,
            "prioritizationFeeLamports": PRIORITIZATION_FEE_LAMPORTS,
        });
        let resp = self.http.post("https://quote-api.jup.ag/v6/swap").json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("Jupiter swap tx failed ({status}): {}", text.chars().take(300).collect::<String>());
        }
        let v: Value = serde_json::from_str(&text)?;
        v.get("swapTransaction")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .context("Jupiter swap response missing swapTransaction")
    }

    /// Sign a Jupiter single-signer transaction: replace the placeholder
    /// signature with an ed25519 signature over sha256(message).
    pub fn sign_transaction(base64_tx: &str, seed: &[u8; 32]) -> Result<String> {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine;
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};

        let bytes = B64.decode(base64_tx.trim()).context("invalid base64 transaction")?;
        let (count, count_len) = decode_short_u16(&bytes).context("invalid signature count")?;
        if count != 1 {
            bail!("expected a single-signer transaction, got {count} signers");
        }
        let msg_start = count_len + 64 * count as usize;
        if bytes.len() < msg_start + 1 {
            bail!("transaction too short");
        }
        let message = &bytes[msg_start..];
        let msg_hash = Sha256::digest(message);

        let sk = SigningKey::from_bytes(seed);
        let sig = sk.sign(&msg_hash);

        let mut out = Vec::with_capacity(1 + 64 + message.len());
        out.push(0x01);
        out.extend_from_slice(&sig.to_bytes());
        out.extend_from_slice(message);
        Ok(B64.encode(out))
    }

    /// Broadcast a signed transaction (base64), returns the tx signature.
    pub async fn send_transaction(&self, chain: &Chain, signed_b64: &str) -> Result<String> {
        let v = self
            .rpc(
                chain,
                "sendTransaction",
                json!([signed_b64, { "encoding": "base64", "preflightCommitment": "confirmed" }]),
            )
            .await?;
        v.as_str()
            .map(|s| s.to_string())
            .context("sendTransaction returned non-string")
    }

    /// Poll getSignatureStatuses until finalized/confirmed or timeout.
    pub async fn wait_confirmation(&self, chain: &Chain, sig: &str) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        loop {
            if let Ok(v) = self.rpc(chain, "getSignatureStatuses", json!([[sig]])).await {
                if let Some(status) = v.pointer("/value/0") {
                    if let Some(err) = status.get("err") {
                        if !err.is_null() {
                            tracing::warn!(sig, err = %err, "solana tx failed");
                            return false;
                        }
                    }
                    if let Some(conf) = status.get("confirmationStatus").and_then(|c| c.as_str()) {
                        if conf == "finalized" || conf == "confirmed" {
                            return true;
                        }
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Live buy: swap SOL for a token via Jupiter.
    pub async fn buy(
        &self,
        chain: &Chain,
        mint: &str,
        amount_sol: f64,
        slippage: f64,
        wallet: &str,
        seed: &[u8; 32],
    ) -> Result<SolanaTradeResult> {
        let lamports = (amount_sol * 1e9) as u64;
        if lamports == 0 {
            bail!("amount too small for a live swap");
        }
        self.swap_flow(chain, WSOL, mint, lamports, slippage, wallet, seed).await
    }

    /// Live sell: swap a token quantity for SOL.
    pub async fn sell(
        &self,
        chain: &Chain,
        mint: &str,
        quantity: f64,
        slippage: f64,
        wallet: &str,
        seed: &[u8; 32],
    ) -> Result<SolanaTradeResult> {
        if quantity <= 0.0 {
            bail!("quantity must be positive");
        }
        let decimals = self.token_decimals(chain, mint).await.unwrap_or(9);
        let amount = (quantity * 10f64.powi(decimals as i32)) as u64;
        if amount == 0 {
            bail!("quantity too small for token decimals");
        }
        self.swap_flow(chain, mint, WSOL, amount, slippage, wallet, seed).await
    }

    /// Shared swap pipeline: quote → swap tx → sign → broadcast → confirm.
    async fn swap_flow(
        &self,
        chain: &Chain,
        input_mint: &str,
        output_mint: &str,
        amount: u64,
        slippage: f64,
        wallet: &str,
        seed: &[u8; 32],
    ) -> Result<SolanaTradeResult> {
        let bps = (slippage * 10_000.0) as u16;
        let quote = self.quote(input_mint, output_mint, amount, bps).await?;
        if quote.out_amount == 0 {
            bail!("Jupiter returned zero outAmount — no route?");
        }
        let tx_b64 = self.swap_transaction(&quote, wallet).await?;
        let signed = Self::sign_transaction(&tx_b64, seed)?;
        let sig = self.send_transaction(chain, &signed).await?;
        let success = self.wait_confirmation(chain, &sig).await;
        // Effective price in SOL per token (for buys: inAmount/outAmount).
        let price_native = quote.in_amount as f64 / quote.out_amount as f64;
        Ok(SolanaTradeResult {
            tx_signature: sig.clone(),
            success,
            token_amount: quote.out_amount,
            native_amount: quote.in_amount,
            price_native,
            explorer_link: format!("https://solscan.io/tx/{sig}"),
        })
    }
}

impl Default for SolanaClient {
    fn default() -> Self {
        Self::new()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_u16_roundtrip() {
        assert_eq!(decode_short_u16(&[0x01, 0x00]).unwrap(), (1, 1));
        assert_eq!(decode_short_u16(&[0x7f]).unwrap(), (127, 1));
        assert_eq!(decode_short_u16(&[0x80, 0x01]).unwrap(), (128, 2));
        assert_eq!(decode_short_u16(&[0xac, 0x02]).unwrap(), (300, 2));
        assert!(decode_short_u16(&[]).is_err());
    }

    #[test]
    fn signs_a_known_transaction() {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine;
        // Hand-built single-signer tx: count=1, placeholder 64-byte signature,
        // then a trivial message. Signing must be deterministic.
        let message = [0x02u8, 0x03, 0x04, 0x05];
        let mut tx = Vec::new();
        tx.push(0x01);
        tx.extend_from_slice(&[0u8; 64]);
        tx.extend_from_slice(&message);
        let b64 = B64.encode(&tx);

        let seed = [7u8; 32];
        let signed = SolanaClient::sign_transaction(&b64, &seed).unwrap();
        let raw = B64.decode(&signed).unwrap();
        assert_eq!(raw[0], 0x01); // still one signer
        assert_eq!(raw.len(), 1 + 64 + message.len());
        assert_eq!(&raw[65..], &message); // message intact
        // Signature must verify against the derived pubkey.
        use ed25519_dalek::{Signature as EdSig, Verifier, VerifyingKey};
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(&message);
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let pk = VerifyingKey::from(&sk);
        let sig = EdSig::from_slice(&raw[1..65]).unwrap();
        assert!(pk.verify(&hash, &sig).is_ok());
        // Deterministic across calls.
        let again = SolanaClient::sign_transaction(&b64, &seed).unwrap();
        assert_eq!(signed, again);
    }

    #[test]
    fn parses_jupiter_quote_fixture() {
        let json = r#"{"inputMint": "So11111111111111111111111111111111111111112",
          "inAmount": "1000000",
          "outputMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
          "outAmount": "752398",
          "otherAmountThreshold": "746015",
          "swapMode": "ExactIn",
          "slippageBps": 300,
          "priceImpactPct": "0.12",
          "routePlan": [],
          "contextSlot": 1
        }"#;
        let raw: Value = serde_json::from_str(json).unwrap();
        // exercise the same extraction logic
        let in_amount: u64 = raw.get("inAmount").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()).unwrap();
        let out_amount: u64 = raw.get("outAmount").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()).unwrap();
        assert_eq!(in_amount, 1_000_000);
        assert_eq!(out_amount, 752_398);
    }
}

/// Decode Solana's compact-u16 (ShortU16) length prefix.
fn decode_short_u16(bytes: &[u8]) -> Result<(u16, usize)> {
    let b0 = *bytes.first().context("empty buffer")?;
    if b0 < 0x80 {
        Ok((b0 as u16, 1))
    } else {
        let b1 = *bytes.get(1).context("short compact-u16")?;
        Ok((((b0 as u16 & 0x7f) | ((b1 as u16) << 7)), 2))
    }
}
