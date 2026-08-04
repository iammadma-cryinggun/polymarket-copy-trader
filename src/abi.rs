//! Polymarket 核心合约 ABI 定义
//!
//! 监听 CTF Exchange 的 OrderFilled 事件

use alloy::sol;

sol! {
    // Polymarket CTF Exchange 合约
    // 地址: 0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E
    //
    // OrderFilled 事件定义:
    // 当订单成交时触发，包含所有交易详情

    #[sol(rpc)]
    contract CTFExchange {
        // OrderFilled 事件
        // 参数:
        // - orderHash: 订单哈希
        // - maker: 挂单方地址
        // - taker: 吃单方地址（我们要监听的）
        // - tokenId: Token ID（YES 或 NO）
        // - makerAmount: 挂单方数量
        // - takerAmount: 吃单方数量
        // - feeRateBps: 手续费率（基点）
        event OrderFilled(
            bytes32 orderHash,
            address indexed maker,
            address indexed taker,
            uint256 tokenId,
            uint256 makerAmount,
            uint256 takerAmount,
            uint256 feeRateBps
        );

        // Trade 事件（旧版本合约可能使用）
        event Trade(
            bytes32 indexed tradeId,
            address indexed taker,
            address indexed maker,
            uint256 tokenId,
            uint256 amount,
            uint256 price
        );
    }

    // NegRisk Exchange 合约（Polymarket 新版）
    // 地址: 0xd91E40c3570878C357392B0C93bF2C93f5b18D54
    #[sol(rpc)]
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
    /// CTF Exchange 合约地址
    pub const CTF_EXCHANGE: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";

    /// NegRisk Exchange 合约地址
    pub const NEGRISK_EXCHANGE: &str = "0xd91E40c3570878C357392B0C93bF2C93f5b18D54";
}

/// OrderFilled 事件签名（用于过滤日志）
pub mod event_sigs {
    use alloy::primitives::B256;
    use alloy::sol_utils::keccak256;

    /// OrderFilled 事件签名
    /// = keccak256("OrderFilled(bytes32,address,address,uint256,uint256,uint256,uint256)")
    pub fn order_filled() -> B256 {
        keccak256("OrderFilled(bytes32,address,address,uint256,uint256,uint256,uint256)".as_bytes())
    }
}
