//! EVM chain registry.
//!
//! Velocidad is multi-chain: every order/position/alert is tagged with a
//! network, and users switch between chains per-command (`bsc:0x…`) or via a
//! default chain setting. EVM wallets are chain-agnostic (the same address is
//! valid on every EVM chain), so a single wallet works across all chains.
//!
//! Each chain carries everything needed to trade and inspect it:
//! - numeric chain id (for signing + 0x API)
//! - public RPC endpoints (balances, gas, broadcasting)
//! - DexScreener segment id (live token prices/liquidity)
//! - explorer URL (tx links)
//! - wrapped-native address (native price discovery via DexScreener)
//! - curated default ERC20s for `/balance` display

/// What kind of chain this is — EVM (ERC-20, EIP-1559) or Solana (SPL, ed25519).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainKind {
    Evm,
    Solana,
}

/// A registered chain (EVM or Solana).
pub struct Chain {
    /// Canonical slug, e.g. `"ethereum"`, `"bsc"`, `"solana"`.
    pub id: &'static str,
    /// Chain kind: EVM or Solana.
    pub kind: ChainKind,
    /// EIP-155 numeric chain id (EVMs only; 101 is Solana's ecosystem id).
    pub chain_id: u64,
    /// Display name, e.g. `"BNB Smart Chain"`.
    pub name: &'static str,
    /// Native coin symbol, e.g. `"ETH"`, `"BNB"`.
    pub native: &'static str,
    /// Wrapped native token address (WETH/WBNB/WMATIC…), used to price the
    /// native coin in USD via DexScreener.
    pub wrapped_native: &'static str,
    /// Public JSON-RPC endpoints, tried in order.
    pub rpc_urls: &'static [&'static str],
    /// Block explorer URL prefix, e.g. `https://etherscan.io`.
    pub explorer: &'static str,
    /// DexScreener `chainId` segment.
    pub dex_segment: &'static str,
    /// Rough native price in USD — only used as an offline fallback when
    /// DexScreener is unreachable.
    pub fallback_native_usd: f64,
    /// Curated default ERC20s (symbol, address) shown in `/balance`.
    pub default_erc20s: &'static [(&'static str, &'static str)],
}

macro_rules! chain {
    ($id:expr, $chain_id:expr, $name:expr, $native:expr, $wrapped:expr,
     $explorer:expr, $dex:expr, $fallback_usd:expr, $rpc:expr, $erc20s:expr) => {
        Chain {
            id: $id,
            kind: ChainKind::Evm,
            chain_id: $chain_id,
            name: $name,
            native: $native,
            wrapped_native: $wrapped,
            rpc_urls: $rpc,
            explorer: $explorer,
            dex_segment: $dex,
            fallback_native_usd: $fallback_usd,
            default_erc20s: $erc20s,
        }
    };
}

/// All supported chains, in display order.
pub static CHAINS: &[Chain] = &[
    // Solana — not an EVM chain: SPL tokens, ed25519 keys, base58 addresses.
    Chain {
        id: "solana",
        kind: ChainKind::Solana,
        chain_id: 101,
        name: "Solana",
        native: "SOL",
        wrapped_native: "So11111111111111111111111111111111111111112", // WSOL
        rpc_urls: &[
            "https://api.mainnet-beta.solana.com",
            "https://solana-rpc.publicnode.com",
            "https://rpc.ankr.com/solana",
        ],
        explorer: "https://solscan.io",
        dex_segment: "solana",
        fallback_native_usd: 150.0,
        default_erc20s: &[],
    },
    chain!(
        "ethereum",
        1,
        "Ethereum",
        "ETH",
        "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", // WETH
        "https://etherscan.io",
        "ethereum",
        3500.0,
        &[
            "https://ethereum-rpc.publicnode.com",
            "https://eth.llamarpc.com",
            "https://rpc.ankr.com/eth",
        ],
        &[
            ("USDC", "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
            ("USDT", "0xdAC17F958D2ee523a2206206994597C13D831ec7"),
        ]
    ),
    chain!(
        "bsc",
        56,
        "BNB Smart Chain",
        "BNB",
        "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c", // WBNB
        "https://bscscan.com",
        "bsc",
        600.0,
        &[
            "https://bsc-dataseed.binance.org",
            "https://bsc-dataseed1.binance.org",
            "https://bsc-rpc.publicnode.com",
        ],
        &[
            ("USDT", "0x55d398326f99059fF775485246999027B3197955"),
            ("USDC", "0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d"),
        ]
    ),
    chain!(
        "polygon",
        137,
        "Polygon",
        "POL",
        "0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270", // WMATIC
        "https://polygonscan.com",
        "polygon",
        0.5,
        &[
            "https://polygon-rpc.com",
            "https://polygon-bor-rpc.publicnode.com",
        ],
        &[
            ("USDC", "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174"),
            ("USDT", "0xc2132D05D31c914a87C6611C10748AEb04B58e8F"),
        ]
    ),
    chain!(
        "arbitrum",
        42161,
        "Arbitrum One",
        "ETH",
        "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", // WETH
        "https://arbiscan.io",
        "arbitrum",
        3500.0,
        &[
            "https://arb1.arbitrum.io/rpc",
            "https://arbitrum-one-rpc.publicnode.com",
        ],
        &[
            ("USDC", "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            ("USDT", "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
        ]
    ),
    chain!(
        "optimism",
        10,
        "Optimism",
        "ETH",
        "0x4200000000000000000000000000000000000006", // WETH
        "https://optimistic.etherscan.io",
        "optimism",
        3500.0,
        &[
            "https://mainnet.optimism.io",
            "https://optimism-rpc.publicnode.com",
        ],
        &[
            ("USDC", "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85"),
            ("USDT", "0x94b008aA00579c1307B0EF2c499aD98a8ce58e58"),
        ]
    ),
    chain!(
        "base",
        8453,
        "Base",
        "ETH",
        "0x4200000000000000000000000000000000000006", // WETH
        "https://basescan.org",
        "base",
        3500.0,
        &[
            "https://mainnet.base.org",
            "https://base-rpc.publicnode.com",
        ],
        &[("USDC", "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),]
    ),
    chain!(
        "avalanche",
        43114,
        "Avalanche C-Chain",
        "AVAX",
        "0xB31f66AA3C1e785363F0875A1B74E27b85FD66c7", // WAVAX
        "https://snowtrace.io",
        "avalanche",
        35.0,
        &[
            "https://api.avax.network/ext/bc/C/rpc",
            "https://avalanche-c-chain-rpc.publicnode.com",
        ],
        &[
            ("USDC", "0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E"),
            ("USDT", "0x9702230A8Ea53601f5cD2dc00fDBc13d4dF4A8c7"),
        ]
    ),
    chain!(
        "fantom",
        250,
        "Fantom",
        "FTM",
        "0x21be370D5312f44cB42ce377BC9b8a0cEF1A4C83", // WFTM
        "https://ftmscan.com",
        "fantom",
        0.6,
        &["https://rpc.ftm.tools", "https://fantom-rpc.publicnode.com",],
        &[
            ("USDC", "0x04068DA6C83AFCFA0e13ba15A6696662335D5B75"),
            ("USDT", "0x049d68029688eAbF473097a2fC38ef61633A5567"),
        ]
    ),
    chain!(
        "linea",
        59144,
        "Linea",
        "ETH",
        "0xe5D7C2a44FfDDf6b295A15c148167daaAf5Cf34f", // WETH
        "https://lineascan.build",
        "linea",
        3500.0,
        &["https://rpc.linea.build"],
        &[("USDC", "0x176211869cA2b568f2A7D4EE941E073a821EE1ff"),]
    ),
    chain!(
        "blast",
        81457,
        "Blast",
        "ETH",
        "0x4300000000000000000000000000000000000004", // WETH
        "https://blastscan.io",
        "blast",
        3500.0,
        &["https://rpc.blast.io"],
        &[("USDB", "0x4300000000000000000000000000000000000003"),]
    ),
    chain!(
        "scroll",
        534352,
        "Scroll",
        "ETH",
        "0x5300000000000000000000000000000000000004", // WETH
        "https://scrollscan.com",
        "scroll",
        3500.0,
        &["https://rpc.scroll.io"],
        &[("USDC", "0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4"),]
    ),
    chain!(
        "gnosis",
        100,
        "Gnosis",
        "xDAI",
        "0xe91D153E0b41518A2Ce8Dd3D7944Fa863463a97d", // WXDAI
        "https://gnosisscan.io",
        "gnosis",
        1.0,
        &[
            "https://rpc.gnosischain.com",
            "https://gnosis-rpc.publicnode.com",
        ],
        &[("USDC", "0xDDAfbb505ad214D7b80b1f830fcCc89B60fb7A83"),]
    ),
];

impl Chain {
    /// Resolve a user-provided chain name/number to a registered chain.
    /// Accepts slugs (`bsc`), aliases (`bnb`, `arb`, `avax`), `chain:…`
    /// prefixes and numeric chain ids. Case-insensitive.
    pub fn resolve(input: &str) -> Option<&'static Chain> {
        let cleaned = input.trim().trim_start_matches("chain:").trim();
        let lower = cleaned.to_ascii_lowercase();
        CHAINS.iter().find(|c| {
            c.id == lower || c.chain_id.to_string() == lower || alias_matches(c.id, &lower)
        })
    }

    /// Resolve `"chain:token"` or bare `"token"` arguments — returns the chain
    /// (explicit or default) and the bare token address. Token addresses are
    /// 0x-hex on EVMs and base58 on Solana, so no format check happens here
    /// (the risk manager validates per chain kind).
    pub fn resolve_token_arg(
        input: &str,
        default_chain: &'static Chain,
    ) -> Option<(&'static Chain, String)> {
        let t = input.trim();
        if let Some((prefix, addr)) = t.split_once(':') {
            let addr = addr.trim();
            if addr.is_empty() {
                return None;
            }
            let chain = Chain::resolve(prefix)?;
            Some((chain, addr.to_string()))
        } else {
            if t.is_empty() {
                return None;
            }
            Some((default_chain, t.to_string()))
        }
    }

    /// Explorer link for an address or tx hash.
    pub fn explorer_link(&self, hash: &str) -> String {
        format!("{}/{hash}", self.explorer)
    }

    /// Human-friendly summary line, e.g. `ethereum · Ethereum (1) · ETH`.
    pub fn display(&self) -> String {
        format!(
            "{} · {} ({}) · {}",
            self.id, self.name, self.chain_id, self.native
        )
    }
}

fn alias_matches(id: &str, lower: &str) -> bool {
    match id {
        "solana" => matches!(lower, "sol"),
        "ethereum" => matches!(lower, "eth" | "ether"),
        "bsc" => matches!(lower, "bnb" | "binance" | "bep20"),
        "polygon" => matches!(lower, "matic" | "pol"),
        "arbitrum" => matches!(lower, "arb"),
        "optimism" => matches!(lower, "op"),
        "avalanche" => matches!(lower, "avax"),
        "fantom" => matches!(lower, "ftm"),
        "gnosis" => matches!(lower, "xdai" | "gno"),
        _ => false,
    }
}

/// Get a chain by canonical id (DB values always use canonical ids).
pub fn by_id(id: &str) -> Option<&'static Chain> {
    CHAINS.iter().find(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_slugs_and_aliases() {
        assert_eq!(Chain::resolve("bsc").unwrap().id, "bsc");
        assert_eq!(Chain::resolve("BNB").unwrap().id, "bsc");
        assert_eq!(Chain::resolve("56").unwrap().id, "bsc");
        assert_eq!(Chain::resolve("ethereum").unwrap().id, "ethereum");
        assert_eq!(Chain::resolve("eth").unwrap().id, "ethereum");
        assert_eq!(Chain::resolve("42161").unwrap().id, "arbitrum");
        assert_eq!(Chain::resolve("arb").unwrap().id, "arbitrum");
        assert_eq!(Chain::resolve("avax").unwrap().id, "avalanche");
        assert_eq!(Chain::resolve("chain:base").unwrap().id, "base");
        assert_eq!(Chain::resolve("xdai").unwrap().id, "gnosis");
        assert_eq!(Chain::resolve("solana").unwrap().id, "solana");
        assert_eq!(Chain::resolve("sol").unwrap().id, "solana");
        assert!(Chain::resolve("999999999").is_none());
    }

    #[test]
    fn resolve_token_args() {
        let eth = by_id("ethereum").unwrap();
        let (c, addr) = Chain::resolve_token_arg("bsc:0xabc", eth).unwrap();
        assert_eq!(c.id, "bsc");
        assert_eq!(addr, "0xabc");
        let (c, addr) = Chain::resolve_token_arg("0xdeadbeef", eth).unwrap();
        assert_eq!(c.id, "ethereum");
        assert_eq!(addr, "0xdeadbeef");
        // Non-hex addresses are accepted (Solana base58 mints) and validated
        // later by the risk manager per chain kind.
        let (c, addr) =
            Chain::resolve_token_arg("sol:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", eth)
                .unwrap();
        assert_eq!(c.id, "solana");
        assert!(addr.starts_with("EPj"));
        assert!(Chain::resolve_token_arg("nosuchchain:0xabc", eth).is_none());
    }

    #[test]
    fn chain_ids_are_unique() {
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_slugs = std::collections::HashSet::new();
        for c in CHAINS {
            assert!(
                seen_ids.insert(c.chain_id),
                "duplicate chain id {}",
                c.chain_id
            );
            assert!(seen_slugs.insert(c.id), "duplicate id {}", c.id);
            if c.kind == ChainKind::Evm {
                assert_eq!(
                    c.wrapped_native.len(),
                    42,
                    "bad wrapped native for {}",
                    c.id
                );
            }
            assert!(!c.rpc_urls.is_empty());
        }
    }
}
