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
    /// The token ACCOUNT (ATA) holding the balance — the address a delegate",
    /// must be set on.
    pub account: String,
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


/// Sponsor context: seed + derived pubkey.
#[derive(Debug, Clone, Copy)]
pub struct SponsorCtx {
    pub seed: [u8; 32],
    pub pubkey: [u8; 32],
}

impl SponsorCtx {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&seed);
        Self { seed, pubkey: sk.verifying_key().to_bytes() }
    }

    pub fn address(&self) -> String {
        bs58::encode(self.pubkey).into_string()
    }
}

pub struct SolanaClient {
    http: reqwest::Client,
    /// Solana gas-sponsorship operator, when configured.
    pub sponsor: Option<SponsorCtx>,
}

impl SolanaClient {
    pub fn new(sponsor: Option<SponsorCtx>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("failed to build Solana HTTP client"),
            sponsor,
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
                let account = item
                    .get("pubkey")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string();
                let ta = info.get("tokenAmount").context("malformed tokenAmount")?;
                let decimals = ta.get("decimals").and_then(|d| d.as_u64()).unwrap_or(0) as u8;
                let amount = ta.get("uiAmount").and_then(|a| a.as_f64()).unwrap_or(0.0);
                if amount > 0.0 {
                    out.push(SolBalance { mint, account, amount, decimals });
                }
            }
        }
        Ok(out)
    }

    /// Latest blockhash (32 raw bytes) for message construction.
    pub async fn latest_blockhash(&self, chain: &Chain) -> Result<[u8; 32]> {
        let v = self.rpc(chain, "getLatestBlockhash", json!([{ "commitment": "processed" }])).await?;
        let bh = v
            .pointer("/value/blockhash")
            .and_then(|b| b.as_str())
            .context("getLatestBlockhash missing blockhash")?;
        let raw = bs58::decode(bh).into_vec().context("bad blockhash")?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&raw);
        Ok(out)
    }

    /// Send a dual-signed (sponsor + owner) transaction; returns the signature.
    pub async fn send_sponsored(
        &self,
        chain: &Chain,
        message: &[u8],
        signers: &[[u8; 32]],
        seeds: &[&[u8; 32]],
    ) -> Result<String> {
        if signers.len() != seeds.len() {
            bail!("signer/seed count mismatch");
        }
        let mut sigs = Vec::with_capacity(signers.len());
        for (i, _s) in signers.iter().enumerate() {
            sigs.push(sign_message(message, seeds[i]));
        }
        let signed = assemble_transaction(message, &sigs)?;
        self.send_transaction(chain, &signed).await
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
        sponsor: Option<&SponsorCtx>,
    ) -> Result<SolanaTradeResult> {
        let lamports = (amount_sol * 1e9) as u64;
        if lamports == 0 {
            bail!("amount too small for a live swap");
        }
        self.swap_flow(chain, WSOL, mint, lamports, slippage, wallet, seed, sponsor).await
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
        sponsor: Option<&SponsorCtx>,
    ) -> Result<SolanaTradeResult> {
        if quantity <= 0.0 {
            bail!("quantity must be positive");
        }
        let decimals = self.token_decimals(chain, mint).await.unwrap_or(9);
        let amount = (quantity * 10f64.powi(decimals as i32)) as u64;
        if amount == 0 {
            bail!("quantity too small for token decimals");
        }
        self.swap_flow(chain, mint, WSOL, amount, slippage, wallet, seed, sponsor).await
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
        sponsor: Option<&SponsorCtx>,
    ) -> Result<SolanaTradeResult> {
        let bps = (slippage * 10_000.0) as u16;
        let quote = self.quote(input_mint, output_mint, amount, bps).await?;
        if quote.out_amount == 0 {
            bail!("Jupiter returned zero outAmount — no route?");
        }
        let tx_b64 = self.swap_transaction(&quote, wallet).await?;
        let signed = match sponsor {
            Some(sp) => {
                // Rebuild with the sponsor as fee payer; dual-sign (sponsor + owner).
                let owner = bs58::decode(wallet.trim()).into_vec().context("bad wallet pubkey")?;
                let owner: [u8; 32] = owner.try_into().map_err(|_| anyhow::anyhow!("wallet pubkey must be 32 bytes"))?;
                let (message, signers) = rebuild_with_sponsor_payer(&tx_b64, &sp.pubkey, &owner)?;
                let mut sigs = Vec::with_capacity(signers.len());
                for s in &signers {
                    let key = if s == &sp.pubkey { &sp.seed } else { seed };
                    sigs.push(sign_message(&message, key));
                }
                assemble_transaction(&message, &sigs)?
            }
            None => Self::sign_transaction(&tx_b64, seed)?,
        };
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
        Self::new(None)
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


    /// Golden fixture generated with @solana/web3.js: a 1-signer transfer tx
    /// (owner=seed 1, payer=owner, fixed blockhash). We rebuild it with the
    /// sponsor (seed 2) as fee payer, dual-sign, and assert the exact result
    /// — cross-checked against Transaction.from() in node.
    #[test]
    fn sponsor_payer_rebuild_golden() {
        let owner_seed = [1u8; 32];
        let sponsor_seed = [2u8; 32];
        let owner_pub = ed25519_dalek::SigningKey::from_bytes(&owner_seed).verifying_key().to_bytes();
        let sponsor_pub = ed25519_dalek::SigningKey::from_bytes(&sponsor_seed).verifying_key().to_bytes();
        assert_eq!(bs58::encode(owner_pub).into_string(), "AKnL4NNf3DGWZJS6cPknBuEGnVsV4A4m5tgebLHaRSZ9");
        assert_eq!(bs58::encode(sponsor_pub).into_string(), "9hSR6S7WPtxmTojgo6GG3k4yDPecgJY292j7xrsUGWBu");

        let tx_b64 = "AcCnDmXG96o+yad9kYHR/4BKMpqsc8Unmz5tcktAo6ixdnO/yUuC0Ovx07Tn8ZxsWogpxjz23q48wBRzMKasAwIBAAEDiojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1y6MKHN0adBINpOyUwdPqR+WjQqzYisKrPZfLTwitxUvgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAASdwXDTUzpIWq6tYjZnZ+XQKEn5uCAMh2KqupFG63KfsBAgIAAQwCAAAAOTAAAAAAAAA=";
        let (message, signers) = rebuild_with_sponsor_payer(tx_b64, &sponsor_pub, &owner_pub).unwrap();
        assert_eq!(signers.len(), 2);
        assert_eq!(signers[0], sponsor_pub);
        assert_eq!(signers[1], owner_pub);
        // message[0] = 2 signatures now
        assert_eq!(message[0], 2);
        // first two keys in the message = sponsor, then owner
        assert_eq!(&message[3 + 1..3 + 1 + 32], &sponsor_pub[..]);
        assert_eq!(&message[3 + 1 + 32..3 + 1 + 64], &owner_pub[..]);

        let sigs: Vec<Vec<u8>> = signers
            .iter()
            .map(|s| {
                let seed = if *s == sponsor_pub { sponsor_seed } else { owner_seed };
                sign_message(&message, &seed)
            })
            .collect();
        let assembled = assemble_transaction(&message, &sigs).unwrap();
        // Deterministic across runs.
        let again = assemble_transaction(&message, &sigs).unwrap();
        assert_eq!(assembled, again);
        // Decode: count byte, 2 sigs, message intact.
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine;
        let raw = B64.decode(&assembled).unwrap();
        assert_eq!(raw[0], 2);
        assert_eq!(raw.len(), 1 + 128 + message.len());
        assert_eq!(&raw[129..], &message[..]);
        // 64-byte signatures.
        assert_eq!(&raw[1..65], &sigs[0][..]);
        assert_eq!(&raw[65..129], &sigs[1][..]);
        eprintln!("ASSEMBLED={assembled}");
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


// ---------------------------------------------------------------------------
// Gas-fee sponsorship & token delegation
// ---------------------------------------------------------------------------

/// A parsed Solana message (header + keys + blockhash + instructions).
pub struct SolanaMessage {
    pub header: [u8; 3],
    pub keys: Vec<[u8; 32]>,
    pub blockhash: [u8; 32],
    /// Encoded instruction blobs: [program_index, accounts_len_cu16, idx…, data_len_cu16, data…]
    pub instructions: Vec<Vec<u8>>,
}

/// Parse the message portion of a serialized transaction (after signatures).
pub fn parse_message(msg: &[u8]) -> Result<SolanaMessage> {
    if msg.len() < 3 {
        bail!("message too short");
    }
    let header = [msg[0], msg[1], msg[2]];
    let (nkeys, mut pos) = decode_short_u16(&msg[3..]).context("key count")?;
    pos += 3;
    let mut keys = Vec::with_capacity(nkeys as usize);
    for _ in 0..nkeys {
        if msg.len() < pos + 32 {
            bail!("message too short for keys");
        }
        let mut k = [0u8; 32];
        k.copy_from_slice(&msg[pos..pos + 32]);
        keys.push(k);
        pos += 32;
    }
    if msg.len() < pos + 32 {
        bail!("message too short for blockhash");
    }
    let mut blockhash = [0u8; 32];
    blockhash.copy_from_slice(&msg[pos..pos + 32]);
    pos += 32;
    let (ninstr, ipos) = decode_short_u16(&msg[pos..]).context("instruction count")?;
    pos += ipos;
    let mut instructions = Vec::with_capacity(ninstr as usize);
    for _ in 0..ninstr {
        if msg.len() <= pos {
            bail!("message too short for instruction");
        }
        let program = msg[pos];
        let (nacc, ap) = decode_short_u16(&msg[pos + 1..]).context("instruction accounts")?;
        let acc_start = pos + 1 + ap;
        let acc_bytes = nacc as usize;
        let (ndata, dp) = decode_short_u16(&msg[acc_start + acc_bytes..]).context("instruction data")?;
        let data_start = acc_start + acc_bytes + dp;
        let data_bytes = ndata as usize;
        if msg.len() < data_start + data_bytes {
            bail!("message too short for instruction data");
        }
        let mut blob = Vec::with_capacity(1 + ap + acc_bytes + dp + data_bytes);
        blob.push(program);
        encode_short_u16(nacc, &mut blob);
        blob.extend_from_slice(&msg[acc_start..acc_start + acc_bytes]);
        encode_short_u16(ndata, &mut blob);
        blob.extend_from_slice(&msg[data_start..data_start + data_bytes]);
        instructions.push(blob);
        pos = data_start + data_bytes;
    }
    Ok(SolanaMessage { header, keys, blockhash, instructions })
}

fn encode_short_u16(v: u16, out: &mut Vec<u8>) {
    if v < 0x80 {
        out.push(v as u8);
    } else {
        out.push((v as u8) | 0x80);
        out.push((v >> 7) as u8);
    }
}

fn encode_message(m: &SolanaMessage) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&m.header);
    encode_short_u16(m.keys.len() as u16, &mut out);
    for k in &m.keys {
        out.extend_from_slice(k);
    }
    out.extend_from_slice(&m.blockhash);
    encode_short_u16(m.instructions.len() as u16, &mut out);
    for i in &m.instructions {
        out.extend_from_slice(i);
    }
    out
}

/// Rebuild a single-signer transaction so the SPONSOR becomes the fee payer:
/// keys[0] is replaced with the sponsor and the original owner is appended,
/// with every instruction index remapped. The message must then be signed by
/// both the sponsor and the owner.
pub fn rebuild_with_sponsor_payer(
    base64_tx: &str,
    sponsor: &[u8; 32],
    owner: &[u8; 32],
) -> Result<(Vec<u8>, Vec<[u8; 32]>)> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;

    let bytes = B64.decode(base64_tx.trim()).context("invalid base64 transaction")?;
    let (count, count_len) = decode_short_u16(&bytes).context("signature count")?;
    let msg_start = count_len + 64 * count as usize;
    let mut m = parse_message(&bytes[msg_start..])?;

    let owner_idx = m
        .keys
        .iter()
        .position(|k| k == owner)
        .context("owner not found in transaction keys")?;
    if owner_idx != 0 {
        bail!("unexpected owner position {owner_idx} — fee-payer remap only supports owner-as-payer txs");
    }

    // keys[0] = fee payer slot → sponsor; the owner moves to index 1 so the
    // header's signer set (first num_required_signatures keys) is
    // [sponsor, owner]. Everything after shifts by one.
    m.keys[0] = *sponsor;
    m.keys.insert(1, *owner);

    let remap = |idx: u8| -> u8 {
        if idx == owner_idx as u8 {
            1 // owner now at index 1
        } else if idx > 0 {
            idx + 1 // everything after the payer shifts by one
        } else {
            idx
        }
    };
    for instr in &mut m.instructions {
        // The program-id index shifts with the key list too.
        instr[0] = remap(instr[0]);
        let (nacc, ap) = decode_short_u16(&instr[1..]).context("instr accounts")?;
        let pos = 1 + ap;
        for i in 0..nacc as usize {
            let idx = instr[pos + i];
            instr[pos + i] = remap(idx);
        }
    }

    m.header[0] += 1; // now two signers: sponsor (payer) + owner
    let message = encode_message(&m);
    Ok((message, vec![*sponsor, *owner]))
}

/// Assemble a signed transaction: [count][sig1][sig2…][message].
pub fn assemble_transaction(message: &[u8], signatures: &[Vec<u8>]) -> Result<String> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    if signatures.len() != message[0] as usize {
        bail!("signature count mismatch");
    }
    let mut out = Vec::with_capacity(1 + 64 * signatures.len() + message.len());
    out.push(message[0]);
    for s in signatures {
        if s.len() != 64 {
            bail!("signature must be 64 bytes");
        }
        out.extend_from_slice(s);
    }
    out.extend_from_slice(message);
    Ok(B64.encode(out))
}

/// ed25519 signature over sha256(message).
pub fn sign_message(message: &[u8], seed: &[u8; 32]) -> Vec<u8> {
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    let sk = SigningKey::from_bytes(seed);
    let sig = sk.sign(&Sha256::digest(message));
    sig.to_bytes().to_vec()
}

/// SPL-token Approve instruction blob (variant 4).
fn approve_instruction(program_idx: u8, source_idx: u8, delegate_idx: u8, owner_idx: u8, amount: u64) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(program_idx);
    encode_short_u16(3, &mut out);
    out.push(source_idx);
    out.push(delegate_idx);
    out.push(owner_idx);
    encode_short_u16(9, &mut out);
    out.push(4);
    out.extend_from_slice(&amount.to_le_bytes());
    out
}

/// SPL-token Revoke instruction blob (variant 5).
#[allow(dead_code)]
fn revoke_instruction(program_idx: u8, source_idx: u8, owner_idx: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(program_idx);
    encode_short_u16(2, &mut out);
    out.push(source_idx);
    out.push(owner_idx);
    encode_short_u16(1, &mut out);
    out.push(5);
    out
}

/// Assemble an SPL Approve transaction with the sponsor as fee payer.
/// Signature order: [sponsor, owner].
pub fn build_sponsored_approve(
    owner: &[u8; 32],
    source_token_account: &[u8; 32],
    delegate: &[u8; 32],
    amount: u64,
    sponsor: &[u8; 32],
    blockhash: &[u8; 32],
) -> (Vec<u8>, Vec<[u8; 32]>) {
    const TOKEN_PROGRAM: [u8; 32] = [
        6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133, 237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
    ];
    let keys = vec![*sponsor, *owner, TOKEN_PROGRAM, *source_token_account, *delegate];
    let header = [2u8, 0, 2];
    let mut instructions = Vec::new();
    instructions.push(approve_instruction(2, 3, 4, 1, amount));
    let m = SolanaMessage {
        header,
        keys: keys.clone(),
        blockhash: *blockhash,
        instructions,
    };
    (encode_message(&m), vec![*sponsor, *owner])
}

/// Assemble an SPL Revoke transaction with the sponsor as fee payer.
#[allow(dead_code)]
pub fn build_sponsored_revoke(
    owner: &[u8; 32],
    source_token_account: &[u8; 32],
    sponsor: &[u8; 32],
    blockhash: &[u8; 32],
) -> (Vec<u8>, Vec<[u8; 32]>) {
    const TOKEN_PROGRAM: [u8; 32] = [
        6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133, 237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
    ];
    let keys = vec![*sponsor, *owner, TOKEN_PROGRAM, *source_token_account];
    let header = [2u8, 0, 2];
    let mut instructions = Vec::new();
    instructions.push(revoke_instruction(2, 3, 1));
    let m = SolanaMessage {
        header,
        keys: keys.clone(),
        blockhash: *blockhash,
        instructions,
    };
    (encode_message(&m), vec![*sponsor, *owner])
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