//! Polymarket 核心合约 ABI 定义
//!
//! 监听 CTF Exchange / NegRisk Exchange 的 OrderFilled 事件
//! V1 与 V2 合约的 OrderFilled 事件签名不同，需要分别定义

use alloy::sol;

sol! {
    // Polymarket CTF Exchange V1 / NegRisk Exchange V1 共享的 OrderFilled 事件
    // V1 事件为 8 个参数：含 makerAssetId / takerAssetId，无独立 side 字段
    // 方向推断：makerAssetId == 0 表示 maker 在买入（BUY），否则为卖出（SELL）
    contract CTFExchangeV1 {
        event OrderFilled(
            bytes32 indexed orderHash,
            address indexed maker,
            address indexed taker,
            uint256 makerAssetId,
            uint256 takerAssetId,
            uint256 makerAmountFilled,
            uint256 takerAmountFilled,
            uint256 fee
        );
    }

    // Polymarket CTF Exchange V2 / NegRisk Exchange V2 共享的 OrderFilled 事件
    // V2 事件为 10 个参数：单 tokenId + 显式 uint8 side（0=BUY, 1=SELL）+ builder + metadata
    contract CTFExchangeV2 {
        event OrderFilled(
            bytes32 indexed orderHash,
            address indexed maker,
            address indexed taker,
            uint8 side,
            uint256 tokenId,
            uint256 makerAmountFilled,
            uint256 takerAmountFilled,
            uint256 fee,
            bytes32 builder,
            bytes32 metadata
        );
    }
}

/// Polymarket 核心合约地址
pub mod addresses {
    /// CTF Exchange V1（2026-04-28 已切换至 V2，流量极少）
    pub const CTF_EXCHANGE: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";
    /// NegRisk Exchange V1
    pub const NEGRISK_EXCHANGE: &str = "0xC5d563A36AE78145C45a50134d48A1215220f80a";
    /// CTF Exchange V2（当前主力交易所）
    pub const CTF_EXCHANGE_V2: &str = "0xE111180000d2663C0091e4f400237545B87B996B";
    /// NegRisk Exchange V2（短线和多选项市场主力）
    pub const NEGRISK_EXCHANGE_V2: &str = "0xe2222d279d744050d28e00520010520000310F59";
}

/// OrderFilled 事件签名
pub mod event_sigs {
    use alloy::primitives::{keccak256, B256};

    /// V1 OrderFilled 事件签名（8 参数）
    pub fn order_filled_v1() -> B256 {
        keccak256(
            "OrderFilled(bytes32,address,address,uint256,uint256,uint256,uint256,uint256)"
                .as_bytes(),
        )
    }

    /// V2 OrderFilled 事件签名（10 参数，含 uint8 side、bytes32 builder、bytes32 metadata）
    pub fn order_filled_v2() -> B256 {
        keccak256(
            "OrderFilled(bytes32,address,address,uint8,uint256,uint256,uint256,uint256,bytes32,bytes32)"
                .as_bytes()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::event_sigs::{order_filled_v1, order_filled_v2};
    use super::{CTFExchangeV1, CTFExchangeV2};
    use alloy::primitives::B256;
    use alloy::sol_types::SolEvent;

    /// 链上实际 topic0（来自 CTF Exchange V1 合约源码与已抓取区块日志）
    const V1_TOPIC0: &str = "0xd0a08e8c493f9c94f29311604c9de1b4e8c8d4c06bd0c789af57f2d65bfec0f6";
    /// 链上实际 topic0（来自 CTF Exchange V2 源码 Events.sol 与区块日志）
    const V2_TOPIC0: &str = "0xd543adfd945773f1a62f74f0ee55a5e3b9b1a28262980ba90b1a89f2ea84d8ee";

    #[test]
    fn v1_order_filled_signature_matches_onchain() {
        let expected: B256 = V1_TOPIC0.parse().unwrap();
        assert_eq!(CTFExchangeV1::OrderFilled::SIGNATURE_HASH, expected);
        assert_eq!(order_filled_v1(), expected);
    }

    #[test]
    fn v2_order_filled_signature_matches_onchain() {
        let expected: B256 = V2_TOPIC0.parse().unwrap();
        assert_eq!(CTFExchangeV2::OrderFilled::SIGNATURE_HASH, expected);
        assert_eq!(order_filled_v2(), expected);
    }

    /// 用一条真实抓取的链上 V2 OrderFilled 日志验证解码。
    /// 该日志 topics 为 [sig, orderHash(indexed), maker(indexed), taker(indexed)]，
    /// 若 orderHash 未标 indexed 会导致 decode_raw_log 失败（topic 数量不符），从而静默漏检。
    #[test]
    fn v2_decode_works_for_real_onchain_log() {
        use alloy::primitives::{Bytes, U256};
        use alloy::sol_types::SolEvent;

        fn hex_to_bytes(s: &str) -> Vec<u8> {
            let s = s.strip_prefix("0x").unwrap_or(s);
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        }

        let topics: Vec<B256> = vec![
            V2_TOPIC0.parse().unwrap(),
            "0x066df9f5d6c3563b61e95d96eb4c659cd4d0f4b338853795897d0024d848ce99"
                .parse()
                .unwrap(),
            "0x000000000000000000000000f418d3a1a941292f9c8707d62a14980c5beb95a3"
                .parse()
                .unwrap(),
            "0x000000000000000000000000e111180000d2663c0091e4f400237545b87b996b"
                .parse()
                .unwrap(),
        ];
        let data = Bytes::from(hex_to_bytes(
            "0000000000000000000000000000000000000000000000000000000000000000\
             77e3116e59406c3c215b3e416501bc269dd01c5a68fd0747547ce931a648ea03\
             00000000000000000000000000000000000000000000000000000000023c3460\
             0000000000000000000000000000000000000000000000000000000002faf080\
             00000000000000000000000000000000000000000000000000000000000a037a\
             0000000000000000000000000000000000000000000000000000000000000000\
             0000000000000000000000000000000000000000000000000000000000000000",
        ));

        let e = CTFExchangeV2::OrderFilled::decode_raw_log(&topics, &data)
            .expect("V2 OrderFilled 应能解码（orderHash 为 indexed）");
        assert_eq!(
            e.maker,
            "0xf418d3a1a941292f9c8707d62a14980c5beb95a3"
                .parse::<alloy::primitives::Address>()
                .unwrap()
        );
        assert_eq!(
            e.taker,
            "0xe111180000d2663c0091e4f400237545b87b996b"
                .parse::<alloy::primitives::Address>()
                .unwrap()
        );
        assert_eq!(e.side, 0u8);
        assert_eq!(
            e.tokenId,
            U256::from_str_radix(
                "77e3116e59406c3c215b3e416501bc269dd01c5a68fd0747547ce931a648ea03",
                16
            )
            .unwrap()
        );
        assert_eq!(e.makerAmountFilled.to::<u64>(), 37_500_000);
        assert_eq!(e.takerAmountFilled.to::<u64>(), 50_000_000);
    }
}
