//! Polymarket 核心合约 ABI 定义
//!
//! 监听 CTF Exchange 的 OrderFilled 事件

use alloy::sol;

sol! {
    // Polymarket CTF Exchange 合约
    // 地址: 0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E

    contract CTFExchange {
        event OrderFilled(
            bytes32 orderHash,
            address indexed maker,
            address indexed taker,
            uint256 tokenId,
            uint256 makerAmount,
            uint256 takerAmount,
            uint256 feeRateBps
        );

        event Trade(
            bytes32 indexed tradeId,
            address indexed taker,
            address indexed maker,
            uint256 tokenId,
            uint256 amount,
            uint256 price
        );
    }

    // NegRisk Exchange 合约
    // 地址: 0xd91E40c3570878C357392B0C93bF2C93f5b18D54
    contract NegRiskExchange {
        event OrderFilled(
            bytes32 orderHash,
            address indexed maker,
            address indexed taker,
            uint256 tokenId,
            uint256 makerAmount,
            uint256 takerAmount,
            uint256 feeRateBps
        );
    }
}

/// Polymarket 核心合约地址
pub mod addresses {
    pub const CTF_EXCHANGE: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";
    pub const NEGRISK_EXCHANGE: &str = "0xd91E40c3570878C357392B0C93bF2C93f5b18D54";
}

/// OrderFilled 事件签名
pub mod event_sigs {
    use alloy::primitives::{B256, keccak256};

    /// OrderFilled 事件签名
    pub fn order_filled() -> B256 {
        keccak256("OrderFilled(bytes32,address,address,uint256,uint256,uint256,uint256)".as_bytes())
    }
}
