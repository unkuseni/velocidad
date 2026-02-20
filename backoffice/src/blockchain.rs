//! Blockchain module for multi-chain trading support.
//!
//! This module provides abstractions for interacting with multiple blockchains
//! (Ethereum, Solana, etc.) through a unified interface.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::{Error, Result};
use crate::prelude::*;

/// Supported blockchain networks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    /// Ethereum Mainnet
    Ethereum,
    /// Ethereum Goerli Testnet
    EthereumGoerli,
    /// Ethereum Sepolia Testnet
    EthereumSepolia,
    /// Solana Mainnet
    Solana,
    /// Solana Devnet
    SolanaDevnet,
    /// Solana Testnet
    SolanaTestnet,
    /// Arbitrum One
    Arbitrum,
    /// Polygon Mainnet
    Polygon,
    /// Optimism
    Optimism,
    /// Base
    Base,
    /// Avalanche C-Chain
    Avalanche,
    /// Binance Smart Chain
    Bsc,
}

impl Chain {
    /// Get the chain ID for the network
    pub fn chain_id(&self) -> u64 {
        match self {
            Chain::Ethereum => 1,
            Chain::EthereumGoerli => 5,
            Chain::EthereumSepolia => 11155111,
            Chain::Arbitrum => 42161,
            Chain::Polygon => 137,
            Chain::Optimism => 10,
            Chain::Base => 8453,
            Chain::Avalanche => 43114,
            Chain::Bsc => 56,
            // Solana chains don't use Ethereum-style chain IDs
            Chain::Solana | Chain::SolanaDevnet | Chain::SolanaTestnet => 0,
        }
    }

    /// Get the human-readable name of the chain
    pub fn name(&self) -> &'static str {
        match self {
            Chain::Ethereum => "Ethereum Mainnet",
            Chain::EthereumGoerli => "Ethereum Goerli",
            Chain::EthereumSepolia => "Ethereum Sepolia",
            Chain::Solana => "Solana Mainnet",
            Chain::SolanaDevnet => "Solana Devnet",
            Chain::SolanaTestnet => "Solana Testnet",
            Chain::Arbitrum => "Arbitrum One",
            Chain::Polygon => "Polygon",
            Chain::Optimism => "Optimism",
            Chain::Base => "Base",
            Chain::Avalanche => "Avalanche",
            Chain::Bsc => "Binance Smart Chain",
        }
    }

    /// Check if this chain is a testnet
    pub fn is_testnet(&self) -> bool {
        matches!(
            self,
            Chain::EthereumGoerli
                | Chain::EthereumSepolia
                | Chain::SolanaDevnet
                | Chain::SolanaTestnet
        )
    }

    /// Get the native token symbol for the chain
    pub fn native_token_symbol(&self) -> &'static str {
        match self {
            Chain::Ethereum
            | Chain::EthereumGoerli
            | Chain::EthereumSepolia
            | Chain::Arbitrum
            | Chain::Optimism
            | Chain::Base => "ETH",
            Chain::Solana | Chain::SolanaDevnet | Chain::SolanaTestnet => "SOL",
            Chain::Polygon => "MATIC",
            Chain::Avalanche => "AVAX",
            Chain::Bsc => "BNB",
        }
    }
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for Chain {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "ethereum" | "eth" | "mainnet" => Ok(Chain::Ethereum),
            "goerli" | "ethereum-goerli" => Ok(Chain::EthereumGoerli),
            "sepolia" | "ethereum-sepolia" => Ok(Chain::EthereumSepolia),
            "solana" | "sol" => Ok(Chain::Solana),
            "solana-devnet" | "sol-devnet" => Ok(Chain::SolanaDevnet),
            "solana-testnet" | "sol-testnet" => Ok(Chain::SolanaTestnet),
            "arbitrum" | "arb" => Ok(Chain::Arbitrum),
            "polygon" | "matic" => Ok(Chain::Polygon),
            "optimism" | "op" => Ok(Chain::Optimism),
            "base" => Ok(Chain::Base),
            "avalanche" | "avax" => Ok(Chain::Avalanche),
            "bsc" | "binance" => Ok(Chain::Bsc),
            _ => Err(Error::validation(format!("Unknown chain: {}", s))),
        }
    }
}

/// Blockchain-specific error types
#[derive(Error, Debug)]
pub enum BlockchainError {
    /// Invalid address format
    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    /// Invalid transaction
    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),

    /// Transaction failed
    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    /// Transaction reverted
    #[error("Transaction reverted: {0}")]
    TransactionReverted(String),

    /// Insufficient funds for transaction
    #[error("Insufficient funds: {0}")]
    InsufficientFunds(String),

    /// Gas estimation failed
    #[error("Gas estimation failed: {0}")]
    GasEstimationFailed(String),

    /// Network error (timeout, connection, etc.)
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Chain-specific error
    #[error("{chain} error: {message}")]
    ChainError { chain: Chain, message: String },

    /// Unsupported operation for this chain
    #[error("Unsupported operation for {0}: {1}")]
    UnsupportedOperation(Chain, String),
}

/// Blockchain address (hex string for EVM, base58 for Solana, etc.)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address(String);

impl Address {
    /// Create a new address from a string
    pub fn new(address: impl Into<String>) -> Result<Self> {
        let addr = address.into();
        // Basic validation - can be extended per chain
        if addr.is_empty() {
            return Err(Error::validation("Address cannot be empty"));
        }
        Ok(Self(addr))
    }

    /// Get the address as a string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert to string
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Token amount with precision
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct TokenAmount {
    /// Raw amount (in smallest units, e.g., wei for ETH)
    pub raw: u128,
    /// Decimals for the token
    pub decimals: u8,
}

impl TokenAmount {
    /// Create a new token amount from raw units
    pub fn from_raw(raw: u128, decimals: u8) -> Self {
        Self { raw, decimals }
    }

    /// Create a new token amount from a decimal value
    pub fn from_decimal(value: f64, decimals: u8) -> Result<Self> {
        if value < 0.0 {
            return Err(Error::validation("Token amount cannot be negative"));
        }

        let multiplier = 10u128.pow(decimals as u32);
        let raw = (value * multiplier as f64).round() as u128;

        Ok(Self { raw, decimals })
    }

    /// Get the decimal value
    pub fn to_decimal(&self) -> f64 {
        let divisor = 10u128.pow(self.decimals as u32);
        self.raw as f64 / divisor as f64
    }
}

/// Transaction request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRequest {
    /// Chain to execute on
    pub chain: Chain,
    /// From address
    pub from: Address,
    /// To address (optional for contract creation)
    pub to: Option<Address>,
    /// Amount to send
    pub amount: TokenAmount,
    /// Transaction data (for contract calls)
    pub data: Option<Vec<u8>>,
    /// Gas limit (optional, will be estimated if not provided)
    pub gas_limit: Option<u64>,
    /// Max fee per gas (EIP-1559) or gas price (legacy)
    pub max_fee_per_gas: Option<TokenAmount>,
    /// Max priority fee per gas (EIP-1559)
    pub max_priority_fee_per_gas: Option<TokenAmount>,
    /// Nonce (optional, will be fetched if not provided)
    pub nonce: Option<u64>,
}

/// Transaction receipt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionReceipt {
    /// Transaction hash
    pub hash: String,
    /// Block number
    pub block_number: Option<u64>,
    /// Block hash
    pub block_hash: Option<String>,
    /// Transaction index
    pub transaction_index: Option<u64>,
    /// From address
    pub from: Address,
    /// To address
    pub to: Option<Address>,
    /// Gas used
    pub gas_used: Option<u64>,
    /// Effective gas price
    pub effective_gas_price: Option<TokenAmount>,
    /// Transaction status (true = success, false = reverted)
    pub status: bool,
    /// Logs
    pub logs: Vec<TransactionLog>,
    /// Timestamp (block timestamp)
    pub timestamp: Option<u64>,
}

/// Transaction log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionLog {
    /// Log index
    pub log_index: u64,
    /// Address that emitted the log
    pub address: Address,
    /// Topics (indexed parameters)
    pub topics: Vec<String>,
    /// Data (non-indexed parameters)
    pub data: Vec<u8>,
}

/// Blockchain client trait
#[async_trait]
pub trait BlockchainClient: Send + Sync {
    /// Get the chain this client is connected to
    fn chain(&self) -> Chain;

    /// Get the current block number
    async fn get_block_number(&self) -> Result<u64>;

    /// Get the balance of an address
    async fn get_balance(&self, address: &Address) -> Result<TokenAmount>;

    /// Get the nonce for an address
    async fn get_nonce(&self, address: &Address) -> Result<u64>;

    /// Estimate gas for a transaction
    async fn estimate_gas(&self, tx: &TransactionRequest) -> Result<u64>;

    /// Get gas price (or fee data for EIP-1559)
    async fn get_gas_price(&self) -> Result<GasPriceData>;

    /// Send a transaction
    async fn send_transaction(&self, tx: TransactionRequest) -> Result<String>;

    /// Get transaction receipt
    async fn get_transaction_receipt(&self, hash: &str) -> Result<Option<TransactionReceipt>>;

    /// Call a contract function (read-only)
    async fn call_contract(
        &self,
        contract: &Address,
        data: &[u8],
        block_number: Option<u64>,
    ) -> Result<Vec<u8>>;

    /// Get token balance (ERC20, SPL, etc.)
    async fn get_token_balance(
        &self,
        token: &Address,
        address: &Address,
    ) -> Result<TokenAmount>;

    /// Check if a contract is deployed at the address
    async fn is_contract_deployed(&self, address: &Address) -> Result<bool>;
}

/// Gas price data for EIP-1559 chains
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasPriceData {
    /// Base fee per gas
    pub base_fee_per_gas: TokenAmount,
    /// Max fee per gas
    pub max_fee_per_gas: TokenAmount,
    /// Max priority fee per gas
    pub max_priority_fee_per_gas: TokenAmount,
}

/// Ethereum client implementation
#[cfg(feature = "ethereum")]
pub mod ethereum {
    use super::*;
    use std::sync::Arc;

    /// Ethereum client using ethers-rs
    pub struct EthereumClient {
        chain: Chain,
        // Will be populated with actual ethers client
        // For now, just a placeholder
    }

    impl EthereumClient {
        /// Create a new Ethereum client
        pub fn new(chain: Chain, rpc_url: &str) -> Result<Self> {
            if !matches!(
                chain,
                Chain::Ethereum | Chain::EthereumGoerli | Chain::EthereumSepolia
            ) {
                return Err(Error::validation(format!(
                    "Chain {} is not an Ethereum chain",
                    chain
                )));
            }

            // TODO: Initialize ethers client
            tracing::info!("Creating Ethereum client for {}: {}", chain, rpc_url);

            Ok(Self { chain })
        }
    }

    #[async_trait]
    impl BlockchainClient for EthereumClient {
        fn chain(&self) -> Chain {
            self.chain
        }

        async fn get_block_number(&self) -> Result<u64> {
            // TODO: Implement
            tracing::warn!("EthereumClient::get_block_number not implemented");
            Ok(0)
        }

        async fn get_balance(&self, address: &Address) -> Result<TokenAmount> {
            // TODO: Implement
            tracing::warn!("EthereumClient::get_balance not implemented");
            Ok(TokenAmount::from_raw(0, 18))
        }

        async fn get_nonce(&self, address: &Address) -> Result<u64> {
            // TODO: Implement
            tracing::warn!("EthereumClient::get_nonce not implemented");
            Ok(0)
        }

        async fn estimate_gas(&self, tx: &TransactionRequest) -> Result<u64> {
            // TODO: Implement
            tracing::warn!("EthereumClient::estimate_gas not implemented");
            Ok(21000) // Default gas limit for simple transfers
        }

        async fn get_gas_price(&self) -> Result<GasPriceData> {
            // TODO: Implement
            tracing::warn!("EthereumClient::get_gas_price not implemented");
            Ok(GasPriceData {
                base_fee_per_gas: TokenAmount::from_raw(30_000_000_000, 9), // 30 gwei
                max_fee_per_gas: TokenAmount::from_raw(30_000_000_000, 9),
                max_priority_fee_per_gas: TokenAmount::from_raw(1_500_000_000, 9), // 1.5 gwei
            })
        }

        async fn send_transaction(&self, tx: TransactionRequest) -> Result<String> {
            // TODO: Implement
            tracing::warn!("EthereumClient::send_transaction not implemented");
            Err(Error::trading("Ethereum transaction sending not implemented"))
        }

        async fn get_transaction_receipt(&self, hash: &str) -> Result<Option<TransactionReceipt>> {
            // TODO: Implement
            tracing::warn!("EthereumClient::get_transaction_receipt not implemented");
            Ok(None)
        }

        async fn call_contract(
            &self,
            contract: &Address,
            data: &[u8],
            block_number: Option<u64>,
        ) -> Result<Vec<u8>> {
            // TODO: Implement
            tracing::warn!("EthereumClient::call_contract not implemented");
            Ok(vec![])
        }

        async fn get_token_balance(
            &self,
            token: &Address,
            address: &Address,
        ) -> Result<TokenAmount> {
            // TODO: Implement
            tracing::warn!("EthereumClient::get_token_balance not implemented");
            Ok(TokenAmount::from_raw(0, 18))
        }

        async fn is_contract_deployed(&self, address: &Address) -> Result<bool> {
            // TODO: Implement
            tracing::warn!("EthereumClient::is_contract_deployed not implemented");
            Ok(false)
        }
    }
}

/// Solana client implementation
#[cfg(feature = "solana")]
pub mod solana {
    use super::*;
    use std::sync::Arc;

    /// Solana client using solana-client
    pub struct SolanaClient {
        chain: Chain,
        // Will be populated with actual solana client
        // For now, just a placeholder
    }

    impl SolanaClient {
        /// Create a new Solana client
        pub fn new(chain: Chain, rpc_url: &str) -> Result<Self> {
            if !matches!(chain, Chain::Solana | Chain::SolanaDevnet | Chain::SolanaTestnet) {
                return Err(Error::validation(format!(
                    "Chain {} is not a Solana chain",
                    chain
                )));
            }

            // TODO: Initialize solana client
            tracing::info!("Creating Solana client for {}: {}", chain, rpc_url);

            Ok(Self { chain })
        }
    }

    #[async_trait]
    impl BlockchainClient for SolanaClient {
        fn chain(&self) -> Chain {
            self.chain
        }

        async fn get_block_number(&self) -> Result<u64> {
            // TODO: Implement
            tracing::warn!("SolanaClient::get_block_number not implemented");
            Ok(0)
        }

        async fn get_balance(&self, address: &Address) -> Result<TokenAmount> {
            // TODO: Implement
            tracing::warn!("SolanaClient::get_balance not implemented");
            Ok(TokenAmount::from_raw(0, 9)) // SOL has 9 decimals
        }

        async fn get_nonce(&self, address: &Address) -> Result<u64> {
            // TODO: Implement - Solana uses different account model
            tracing::warn!("SolanaClient::get_nonce not implemented");
            Ok(0)
        }

        async fn estimate_gas(&self, tx: &TransactionRequest) -> Result<u64> {
            // TODO: Implement - Solana uses compute units
            tracing::warn!("SolanaClient::estimate_gas not implemented");
            Ok(2000) // Default compute units
        }

        async fn get_gas_price(&self) -> Result<GasPriceData> {
            // TODO: Implement - Solana uses lamports per signature
            tracing::warn!("SolanaClient::get_gas_price not implemented");
            Ok(GasPriceData {
                base_fee_per_gas: TokenAmount::from_raw(5000, 9), // lamports
                max_fee_per_gas: TokenAmount::from_raw(5000, 9),
                max_priority_fee_per_gas: TokenAmount::from_raw(0, 9), // Not applicable
            })
        }

        async fn send_transaction(&self, tx: TransactionRequest) -> Result<String> {
            // TODO: Implement
            tracing::warn!("SolanaClient::send_transaction not implemented");
            Err(Error::trading("Solana transaction sending not implemented"))
        }

        async fn get_transaction_receipt(&self, hash: &str) -> Result<Option<TransactionReceipt>> {
            // TODO: Implement
            tracing::warn!("SolanaClient::get_transaction_receipt not implemented");
            Ok(None)
        }

        async fn call_contract(
            &self,
            contract: &Address,
            data: &[u8],
            block_number: Option<u64>,
        ) -> Result<Vec<u8>> {
            // TODO: Implement - Solana program calls
            tracing::warn!("SolanaClient::call_contract not implemented");
            Ok(vec![])
        }

        async fn get_token_balance(
            &self,
            token: &Address,
            address: &Address,
        ) -> Result<TokenAmount> {
            // TODO: Implement - SPL token balances
            tracing::warn!("SolanaClient::get_token_balance not implemented");
            Ok(TokenAmount::from_raw(0, 9))
        }

        async fn is_contract_deployed(&self, address: &Address) -> Result<bool> {
            // TODO: Implement
            tracing::warn!("SolanaClient::is_contract_deployed not implemented");
            Ok(false)
        }
    }
}

/// Blockchain client factory
pub struct BlockchainClientFactory;

impl BlockchainClientFactory {
    /// Create a blockchain client for the given chain
    pub async fn create_client(
        chain: Chain,
        rpc_url: &str,
    ) -> Result<Arc<dyn BlockchainClient>> {
        match chain {
            Chain::Ethereum
            | Chain::EthereumGoerli
            | Chain::EthereumSepolia
            | Chain::Arbitrum
            | Chain::Polygon
            | Chain::Optimism
            | Chain::Base
            | Chain::Avalanche
            | Chain::Bsc => {
                #[cfg(feature = "ethereum")]
                {
                    use crate::blockchain::ethereum::EthereumClient;
                    let client = EthereumClient::new(chain, rpc_url)?;
                    Ok(Arc::new(client))
                }
                #[cfg(not(feature = "ethereum"))]
                {
                    Err(Error::validation(format!(
                        "Ethereum support not compiled in (chain: {})",
                        chain
                    )))
                }
            }
            Chain::Solana | Chain::SolanaDevnet | Chain::SolanaTestnet => {
                #[cfg(feature = "solana")]
                {
                    use crate::blockchain::solana::SolanaClient;
                    let client = SolanaClient::new(chain, rpc_url)?;
                    Ok(Arc::new(client))
                }
                #[cfg(not(feature = "solana"))]
                {
                    Err(Error::validation(format!(
                        "Solana support not compiled in (chain: {})",
                        chain
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_from_str() {
        assert_eq!(Chain::from_str("ethereum").unwrap(), Chain::Ethereum);
        assert_eq!(Chain::from_str("ETH").unwrap(), Chain::Ethereum);
        assert_eq!(Chain::from_str("solana").unwrap(), Chain::Solana);
        assert_eq!(Chain::from_str("arbitrum").unwrap(), Chain::Arbitrum);

        assert!(Chain::from_str("unknown").is_err());
    }

    #[test]
    fn test_chain_properties() {
        assert_eq!(Chain::Ethereum.chain_id(), 1);
        assert_eq!(Chain::Ethereum.name(), "Ethereum Mainnet");
        assert_eq!(Chain::Ethereum.native_token_symbol(), "ETH");
        assert!(!Chain::Ethereum.is_testnet());

        assert!(Chain::EthereumGoerli.is_testnet());
    }

    #[test]
    fn test_address_new() {
        let addr = Address::new("0x1234").unwrap();
        assert_eq!(addr.as_str(), "0x1234");
        assert_eq!(addr.to_string(), "0x1234");

        assert!(Address::new("").is_err());
    }

    #[test]
    fn test_token_amount() {
        let amount = TokenAmount::from_raw(1000000000000000000, 18);
        assert_eq!(amount.to_decimal(), 1.0);

        let amount = TokenAmount::from_decimal(1.5, 18).unwrap();
        assert_eq!(amount.raw, 1500000000000000000);

        assert!(TokenAmount::from_decimal(-1.0, 18).is_err());
    }
}
