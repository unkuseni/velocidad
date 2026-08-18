//! Shared trade execution — the single place where buys and sells happen,
//! used by BOTH the Telegram bot and the HTTP API so the two surfaces can
//! never drift apart.
//!
//! Branches:
//! - paper fills (engine) when PAPER_TRADING=true or no swap backend is
//!   configured;
//! - live 0x swaps on EVM chains (self-paid or gas-sponsored via EIP-7702
//!   delegation, gated by the user's sponsor:<chain> opt-in);
//! - live Jupiter swaps on Solana (self-paid or sponsored fee payer).

use anyhow::{bail, Context, Result};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::app::AppState;
use crate::chains::{Chain, ChainKind};
use crate::db::models::Wallet;
use crate::db::repo;
use crate::solana::SolanaTradeResult;
use crate::swap::LiveTradeResult;
use crate::trading::TradeReceipt;

/// Result of an execution — paper receipt or a live swap result.
#[derive(Debug, Serialize)]
#[serde(tag = "mode")]
pub enum TradeOutcome {
    #[serde(rename = "paper")]
    Paper(Box<TradeReceipt>),
    #[serde(rename = "live")]
    EvmLive(EvmLive),
    #[serde(rename = "solana")]
    SolanaLive(SolanaLive),
}

#[derive(Debug, Serialize)]
pub struct EvmLive {
    pub result: LiveTradeResult,
    pub qty: f64,
    pub price: f64,
    pub sponsored: bool,
}

#[derive(Debug, Serialize)]
pub struct SolanaLive {
    pub result: SolanaTradeResult,
    pub qty: f64,
    pub price: f64,
    pub sponsored: bool,
}

/// Execute a buy (or snipe) on `chain`. `side` is "buy" | "snipe".
pub async fn buy(
    state: &AppState,
    chain: &Chain,
    user_id: i64,
    wallet: &Wallet,
    token: &str,
    amount: f64,
    slippage: f64,
    side: &str,
) -> Result<TradeOutcome> {
    // Paper fills on every chain kind when configured or when no backend exists.
    if state.config.paper_trading || (chain.kind == ChainKind::Evm && !state.swap.enabled()) {
        let receipt = state
            .engine
            .buy(
                &state.db,
                chain,
                user_id,
                Some(wallet.id),
                token,
                amount,
                slippage,
                side,
            )
            .await?;
        return Ok(TradeOutcome::Paper(Box::new(receipt)));
    }

    match chain.kind {
        ChainKind::Solana => {
            let secret = wallet_secret(state, wallet)?;
            let sponsor = if user_sponsorship(state, user_id, chain).await? {
                state.solana.sponsor.as_ref()
            } else {
                None
            };
            let result = state
                .solana
                .buy(
                    chain,
                    token,
                    amount,
                    slippage,
                    &wallet.address,
                    &secret,
                    sponsor,
                )
                .await?;
            if !result.success {
                bail!("swap failed — tx {}", result.tx_signature);
            }
            let decimals = state.solana.token_decimals(chain, token).await.unwrap_or(9);
            let qty = result.token_amount as f64 / 10f64.powi(decimals as i32);
            let price = result.price_native;
            record_live_order(
                state,
                user_id,
                Some(wallet.id),
                chain,
                token,
                side,
                amount,
                qty,
                price,
                slippage,
                &result.tx_signature,
            )
            .await?;
            Ok(TradeOutcome::SolanaLive(SolanaLive {
                result,
                qty,
                price,
                sponsored: sponsor.is_some(),
            }))
        }
        ChainKind::Evm => {
            let secret = wallet_secret(state, wallet)?;
            let amount_wei = (amount * 1e18) as u128;
            if amount_wei == 0 {
                bail!("amount too small for a live swap");
            }
            let sponsored = user_sponsorship(state, user_id, chain).await?;
            let result = if sponsored {
                sponsored_evm_buy(state, chain, wallet, token, amount_wei, slippage).await?
            } else {
                state
                    .swap
                    .buy_native(chain, token, amount_wei, slippage, &wallet.address, &secret)
                    .await?
            };
            if !result.success {
                bail!("swap reverted — tx {}", result.tx_hash);
            }
            let decimals = state.rpc.erc20_decimals(chain, token).await;
            let qty = result.token_amount as f64 / 10f64.powi(decimals as i32);
            let price = result.price_native;
            record_live_order(
                state,
                user_id,
                Some(wallet.id),
                chain,
                token,
                side,
                amount,
                qty,
                price,
                slippage,
                &result.tx_hash,
            )
            .await?;
            Ok(TradeOutcome::EvmLive(EvmLive {
                result,
                qty,
                price,
                sponsored,
            }))
        }
    }
}

/// Execute a sell of `qty` tokens on `chain`.
pub async fn sell(
    state: &AppState,
    chain: &Chain,
    user_id: i64,
    wallet: &Wallet,
    token: &str,
    qty: f64,
    slippage: f64,
) -> Result<TradeOutcome> {
    if qty <= 0.0 {
        bail!("quantity must be positive");
    }

    if state.config.paper_trading || (chain.kind == ChainKind::Evm && !state.swap.enabled()) {
        let receipt = state
            .engine
            .sell(
                &state.db,
                chain,
                user_id,
                Some(wallet.id),
                token,
                qty,
                slippage,
            )
            .await?;
        return Ok(TradeOutcome::Paper(Box::new(receipt)));
    }

    match chain.kind {
        ChainKind::Solana => {
            let secret = wallet_secret(state, wallet)?;
            let sponsor = if user_sponsorship(state, user_id, chain).await? {
                state.solana.sponsor.as_ref()
            } else {
                None
            };
            let result = state
                .solana
                .sell(
                    chain,
                    token,
                    qty,
                    slippage,
                    &wallet.address,
                    &secret,
                    sponsor,
                )
                .await?;
            if !result.success {
                bail!("swap failed — tx {}", result.tx_signature);
            }
            let proceeds = result.native_amount as f64 / 1e9;
            let price = result.price_native;
            record_live_order(
                state,
                user_id,
                Some(wallet.id),
                chain,
                token,
                "sell",
                qty,
                proceeds,
                price,
                slippage,
                &result.tx_signature,
            )
            .await?;
            Ok(TradeOutcome::SolanaLive(SolanaLive {
                result,
                qty,
                price,
                sponsored: sponsor.is_some(),
            }))
        }
        ChainKind::Evm => {
            let secret = wallet_secret(state, wallet)?;
            let sponsored = user_sponsorship(state, user_id, chain).await?;
            let result = if sponsored {
                sponsored_evm_sell(state, chain, wallet, token, qty, slippage).await?
            } else {
                state
                    .swap
                    .sell_tokens(chain, token, qty, slippage, &wallet.address, &secret)
                    .await?
            };
            if !result.success {
                bail!("swap reverted — tx {}", result.tx_hash);
            }
            let proceeds = result.native_amount as f64 / 1e18;
            let price = result.price_native;
            record_live_order(
                state,
                user_id,
                Some(wallet.id),
                chain,
                token,
                "sell",
                qty,
                proceeds,
                price,
                slippage,
                &result.tx_hash,
            )
            .await?;
            Ok(TradeOutcome::EvmLive(EvmLive {
                result,
                qty,
                price,
                sponsored,
            }))
        }
    }
}
// ---------------------------------------------------------------------------
// Sponsored EVM flows (delegated execution — sponsor pays gas)
// ---------------------------------------------------------------------------

/// Preconditions for a sponsored EVM trade.
async fn sponsored_preconditions(
    state: &AppState,
    chain: &Chain,
    wallet: &Wallet,
) -> Result<(String, [u8; 32])> {
    let account = sponsor_account_for(state, chain).await?;
    if account.is_empty() {
        bail!(
            "sponsorship ON but no SponsorAccount on {} — run /sponsor setup",
            chain.id
        );
    }
    let target = state.rpc.delegation_of(chain, &wallet.address).await?;
    if target.as_deref() != Some(account.as_str()) {
        bail!("EOA is not delegated to the sponsor account — run /sponsor on");
    }
    let (sponsor_addr, sponsor_secret) = state
        .swap
        .sponsor
        .as_ref()
        .context("sponsor not configured")?;
    Ok((sponsor_addr.clone(), *sponsor_secret))
}

/// Sponsored native→token buy via the delegated EOA.
async fn sponsored_evm_buy(
    state: &AppState,
    chain: &Chain,
    wallet: &Wallet,
    token: &str,
    amount_wei: u128,
    slippage: f64,
) -> Result<LiveTradeResult> {
    let (sponsor_addr, sponsor_secret) = sponsored_preconditions(state, chain, wallet).await?;
    let quote = state
        .swap
        .quote(
            chain,
            crate::swap::NATIVE,
            token,
            amount_wei,
            &wallet.address,
            (slippage * 10_000.0) as u64,
        )
        .await?;
    let swap_data = hex::decode(quote.data.trim_start_matches("0x"))?;
    let (hash, ok) = state
        .swap
        .sponsored_execute(
            chain,
            &wallet.address,
            &sponsor_addr,
            &sponsor_secret,
            &quote.to,
            quote.value,
            &swap_data,
            (quote.gas as f64 * 2.0) as u64,
        )
        .await?;
    if !ok {
        bail!("sponsored swap reverted — tx {hash}");
    }
    Ok(LiveTradeResult {
        tx_hash: hash.clone(),
        success: true,
        token_amount: quote.buy_amount,
        native_amount: quote.sell_amount,
        price_native: quote.price_native,
        approval_tx: None,
        explorer_link: chain.explorer_link(&hash),
    })
}

/// Sponsored token→native sell via the delegated EOA (with approval).
async fn sponsored_evm_sell(
    state: &AppState,
    chain: &Chain,
    wallet: &Wallet,
    token: &str,
    qty: f64,
    slippage: f64,
) -> Result<LiveTradeResult> {
    let (sponsor_addr, sponsor_secret) = sponsored_preconditions(state, chain, wallet).await?;
    let decimals = state.rpc.erc20_decimals(chain, token).await;
    let amount_wei = (qty * 10f64.powi(decimals as i32)) as u128;
    let quote = state
        .swap
        .quote(
            chain,
            token,
            crate::swap::NATIVE,
            amount_wei,
            &wallet.address,
            (slippage * 10_000.0) as u64,
        )
        .await?;

    let mut approval_tx: Option<String> = None;
    if let Some(spender) = &quote.allowance_target {
        let current = state
            .rpc
            .erc20_allowance(chain, token, &wallet.address, spender)
            .await
            .unwrap_or(0);
        if current < amount_wei {
            let approve_data = crate::swap::SwapClient::approve_calldata(spender, amount_wei)?;
            let (ahash, aok) = state
                .swap
                .sponsored_execute(
                    chain,
                    &wallet.address,
                    &sponsor_addr,
                    &sponsor_secret,
                    token,
                    0,
                    &approve_data,
                    150_000,
                )
                .await?;
            if !aok {
                bail!("sponsored approval reverted — tx {ahash}");
            }
            approval_tx = Some(ahash);
        }
    }

    let swap_data = hex::decode(quote.data.trim_start_matches("0x"))?;
    let (hash, ok) = state
        .swap
        .sponsored_execute(
            chain,
            &wallet.address,
            &sponsor_addr,
            &sponsor_secret,
            &quote.to,
            quote.value,
            &swap_data,
            (quote.gas as f64 * 2.0) as u64,
        )
        .await?;
    if !ok {
        bail!("sponsored swap reverted — tx {hash}");
    }
    Ok(LiveTradeResult {
        tx_hash: hash.clone(),
        success: true,
        token_amount: quote.sell_amount,
        native_amount: quote.buy_amount,
        price_native: 1.0 / quote.price_native.max(1e-18),
        approval_tx,
        explorer_link: chain.explorer_link(&hash),
    })
}

// ---------------------------------------------------------------------------
// Shared helpers (moved here from the bot so API + bot share one path)
// ---------------------------------------------------------------------------

/// Default wallet for a chain: Solana wallets are separate from EVM wallets.
pub async fn wallet_for_chain(state: &AppState, user_id: i64, chain: &Chain) -> Result<Wallet> {
    let network = if chain.kind == ChainKind::Solana {
        "solana"
    } else {
        "ethereum"
    };
    match repo::get_default_wallet_for(state.db.conn(), user_id, network).await? {
        Some(w) => Ok(w),
        None => Err(anyhow::anyhow!(if chain.kind == ChainKind::Solana {
            "create a Solana wallet first with /wallet new solana"
        } else {
            "create a wallet first with /wallet new"
        })),
    }
}

/// Decrypt the wallet's private key for live signing (zeroized on drop).
pub fn wallet_secret(state: &AppState, wallet: &Wallet) -> Result<Zeroizing<[u8; 32]>> {
    let enc = wallet.encrypted_key.as_ref().context(
        "wallet is watch-only — import its private key with /wallet import <key> to trade live",
    )?;
    let bytes = state.keyring.decrypt(enc)?;
    let mut secret = Zeroizing::new([0u8; 32]);
    secret.copy_from_slice(&bytes);
    Ok(secret)
}

/// Whether the user opted into gas sponsorship on this chain (and it's configured).
pub async fn user_sponsorship(state: &AppState, user_id: i64, chain: &Chain) -> Result<bool> {
    let setting =
        repo::get_setting(state.db.conn(), user_id, &format!("sponsor:{}", chain.id)).await?;
    if setting.as_deref() != Some("on") {
        return Ok(false);
    }
    Ok(match chain.kind {
        ChainKind::Evm => state.swap.sponsor.is_some(),
        ChainKind::Solana => state.solana.sponsor.is_some(),
    })
}

/// Look up the deployed SponsorAccount address for a chain (global setting).
pub async fn sponsor_account_for(state: &AppState, chain: &Chain) -> Result<String> {
    Ok(
        repo::get_setting(state.db.conn(), 0, &format!("sponsor_account:{}", chain.id))
            .await?
            .unwrap_or_default(),
    )
}

/// Record a live (on-chain) order + update paper accounting atomically.
/// Sells clamp the paper reduction to the tracked position so untracked
/// on-chain holdings don't corrupt the ledger.
pub async fn record_live_order(
    state: &AppState,
    user_id: i64,
    wallet_id: Option<i64>,
    chain: &Chain,
    token: &str,
    side: &str,
    amount_in: f64,
    amount_out: f64,
    price: f64,
    slippage: f64,
    tx_hash: &str,
) -> Result<()> {
    let conn = state.db.conn();
    let tx = conn.transaction().await?;
    if side == "sell" {
        if let Ok(Some(pos)) = repo::get_position(&tx, user_id, chain.id, token).await {
            let tracked = pos.quantity.min(amount_in);
            if (tracked - amount_in).abs() > 1e-12 {
                tracing::warn!(
                    token = %token,
                    chain = %chain.id,
                    tracked = pos.quantity,
                    sold = amount_in,
                    "live sell exceeds tracked position — clamping paper reduction"
                );
            }
            let _ = repo::reduce_position(&tx, user_id, chain.id, token, tracked, price).await;
        }
    } else {
        repo::add_to_position(&tx, user_id, wallet_id, chain.id, token, amount_out, price).await?;
    }
    repo::insert_order(
        &tx,
        &repo::NewOrder {
            user_id,
            wallet_id,
            network: chain.id,
            token_address: token,
            side,
            amount_in: Some(amount_in),
            amount_out: Some(amount_out),
            price: Some(price),
            slippage,
            status: "executed",
            tx_hash: Some(tx_hash),
            error: None,
        },
    )
    .await?;
    let _ = repo::trigger_satisfied_alerts(&tx, user_id, token, price).await?;
    tx.commit().await?;
    Ok(())
}
