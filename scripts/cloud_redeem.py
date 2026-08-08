#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
云端自动赎回脚本 - 使用正确的API端点
修复：gamma-api → data-api
注意：该脚本使用 legacy USDC.e collateral 赎回；Rust BackgroundRedeemer 会在后续步骤检测/处理 USDCE -> pUSD。
"""
import sys
if sys.platform == 'win32':
    import io
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8')

import requests
from web3 import Web3
import os
from dotenv import load_dotenv
from typing import List, Dict

load_dotenv()

PRIVATE_KEY = os.getenv("PRIVATE_KEY", "")
# 默认使用公共 RPC，避免 Alchemy 429 限流
RPC_URL = os.getenv("RPC_URL", os.getenv("POLYMARKET_RPC", "https://polygon-bor-rpc.publicnode.com"))
PROXY_URL = os.getenv("HTTP_PROXY", os.getenv("HTTPS_PROXY", ""))
CTF_CONTRACT = "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045"
USDC_CONTRACT = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174"
USDCE_CONTRACT = USDC_CONTRACT
PUSD_CONTRACT = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB"
CLOB_V2_CONTRACT = "0xE111180000d2663C0091e4f400237545B87B996B"
COLLATERAL_ONRAMP = "0x93070a847efEf7F70739046A929D47a521F5B8ee"
AUTO_CONVERT_USDCE_TO_PUSD = os.getenv("AUTO_CONVERT_USDCE_TO_PUSD", "true").lower() in ("1", "true", "yes", "on")
USDCE_TO_PUSD_MIN_AMOUNT = float(os.getenv("USDCE_TO_PUSD_MIN_AMOUNT", "0.01"))

# 设置代理（如果有）
if PROXY_URL:
    os.environ['HTTP_PROXY'] = PROXY_URL
    os.environ['HTTPS_PROXY'] = PROXY_URL
    print(f"[CONFIG] Using proxy: {PROXY_URL}")
else:
    print("[CONFIG] No proxy (direct connection)")

# 初始化Web3
w3 = Web3(Web3.HTTPProvider(RPC_URL))
# 注意: web3.py 6.x+ 不再需要 ExtraDataToPOAMiddleware

account = w3.eth.account.from_key(PRIVATE_KEY)
WALLET = account.address

ERC20_ABI = [
    {"name": "balanceOf", "type": "function", "inputs": [{"name": "account", "type": "address"}], "outputs": [{"type": "uint256"}]},
    {"name": "allowance", "type": "function", "inputs": [{"name": "owner", "type": "address"}, {"name": "spender", "type": "address"}], "outputs": [{"type": "uint256"}]},
    {"name": "approve", "type": "function", "inputs": [{"name": "spender", "type": "address"}, {"name": "amount", "type": "uint256"}], "outputs": [{"type": "bool"}]},
]

ONRAMP_ABI = [
    {
        "inputs": [
            {"name": "_asset", "type": "address"},
            {"name": "_to", "type": "address"},
            {"name": "_amount", "type": "uint256"}
        ],
        "name": "wrap",
        "outputs": [],
        "type": "function"
    }
]

# CTF合约ABI
CTF_ABI = [
    {
        "inputs": [
            {"name": "collateralToken", "type": "address"},
            {"name": "parentCollectionId", "type": "bytes32"},
            {"name": "conditionId", "type": "bytes32"},
            {"name": "indexSets", "type": "uint256[]"}
        ],
        "name": "redeemPositions",
        "outputs": [],
        "type": "function"
    }
]

def to_units(amount):
    return int(amount * 1_000_000)


def from_units(amount):
    return amount / 1_000_000


def erc20(address):
    return w3.eth.contract(address=Web3.to_checksum_address(address), abi=ERC20_ABI)


def print_collateral_balances(prefix="BALANCE"):
    usdce = erc20(USDCE_CONTRACT)
    pusd = erc20(PUSD_CONTRACT)
    usdce_balance = usdce.functions.balanceOf(WALLET).call()
    pusd_balance = pusd.functions.balanceOf(WALLET).call()
    pusd_allowance = pusd.functions.allowance(WALLET, Web3.to_checksum_address(CLOB_V2_CONTRACT)).call()
    print(f"[{prefix}] USDCE={from_units(usdce_balance):.6f}, pUSD={from_units(pusd_balance):.6f}, pUSD_allowance={from_units(pusd_allowance):.6f}")
    return usdce_balance, pusd_balance, pusd_allowance


def send_tx(txn, label):
    signed = w3.eth.account.sign_transaction(txn, PRIVATE_KEY)
    tx_hash = w3.eth.send_raw_transaction(signed.rawTransaction)
    print(f"[{label}] 交易已发送: {tx_hash.hex()}")
    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=180)
    if receipt["status"] != 1:
        raise RuntimeError(f"{label} failed on-chain")
    print(f"[{label}] ✅ 成功！区块: {receipt['blockNumber']:,}")
    return tx_hash.hex()


def approve_if_needed(token_address, spender, amount, label):
    token = erc20(token_address)
    spender = Web3.to_checksum_address(spender)
    allowance = token.functions.allowance(WALLET, spender).call()
    if allowance >= amount:
        print(f"[{label}] 授权足够: {from_units(allowance):.6f}")
        return None

    nonce = w3.eth.get_transaction_count(WALLET)
    gas_price = w3.eth.gas_price
    txn = token.functions.approve(spender, amount).build_transaction({
        'from': WALLET,
        'gas': 100000,
        'maxFeePerGas': int(gas_price * 1.2),
        'maxPriorityFeePerGas': int(80 * 1e9),
        'nonce': nonce,
        'chainId': 137
    })
    return send_tx(txn, label)


def convert_usdce_to_pusd():
    print("[CONVERT] 检查 USDCE -> pUSD")
    usdce_balance, pusd_before, _ = print_collateral_balances("BEFORE_CONVERT")
    min_units = to_units(USDCE_TO_PUSD_MIN_AMOUNT)

    if usdce_balance < min_units:
        print(f"[CONVERT] USDCE余额低于阈值 {USDCE_TO_PUSD_MIN_AMOUNT:.6f}，跳过")
        return False, None

    if not AUTO_CONVERT_USDCE_TO_PUSD:
        print("[CONVERT] AUTO_CONVERT_USDCE_TO_PUSD=false，仅检测余额，不发送兑换交易")
        return False, None

    print(f"[CONVERT] 准备兑换 {from_units(usdce_balance):.6f} USDCE -> pUSD")
    approve_result = approve_if_needed(USDCE_CONTRACT, COLLATERAL_ONRAMP, usdce_balance, "APPROVE_USDCE_ONRAMP")

    # 如果刚发送了APPROVE交易，等待1秒让nonce同步
    if approve_result:
        import time
        time.sleep(1)
        print("[CONVERT] 等待 nonce 同步...")

    onramp = w3.eth.contract(address=Web3.to_checksum_address(COLLATERAL_ONRAMP), abi=ONRAMP_ABI)
    nonce = w3.eth.get_transaction_count(WALLET)
    gas_price = w3.eth.gas_price
    txn = onramp.functions.wrap(
        Web3.to_checksum_address(USDCE_CONTRACT),
        WALLET,
        usdce_balance
    ).build_transaction({
        'from': WALLET,
        'gas': 350000,
        'maxFeePerGas': int(gas_price * 1.2),
        'maxPriorityFeePerGas': int(80 * 1e9),
        'nonce': nonce,
        'chainId': 137
    })
    tx_hash = send_tx(txn, "WRAP_USDCE_TO_PUSD")

    # 等待链上状态同步
    import time
    time.sleep(3)

    usdce_after, pusd_after, _ = print_collateral_balances("AFTER_CONVERT")
    converted = pusd_after - pusd_before
    if converted <= 0:
        # 可能是RPC缓存延迟，交易已上链就视为成功
        print(f"[CONVERT] ⚠️ pUSD余额未立即更新（RPC缓存延迟），但交易已成功上链")
        return True, tx_hash
    print(f"[CONVERT] ✅ 完成: USDCE减少 {from_units(usdce_balance - usdce_after):.6f}, pUSD增加 {from_units(converted):.6f}")
    return True, tx_hash


def get_redeemable_positions(force=False):
    """获取可赎回的持仓

    Args:
        force: 如果为True，忽略redeemable标志，只要有size>0且currentValue>0就尝试赎回
    """
    wallet_lower = WALLET.lower()

    # ✅ 修复：使用正确的API端点
    api_url = f"https://data-api.polymarket.com/positions?user={wallet_lower}"

    proxies = {}
    if PROXY_URL:
        proxies = {'http': PROXY_URL, 'https': PROXY_URL}

    try:
        response = requests.get(api_url, proxies=proxies, timeout=30)
        response.raise_for_status()
        positions = response.json()

        # 过滤可赎回的持仓
        redeemable = []
        for pos in positions:
            closed = pos.get('closed') is not None
            if closed:
                continue

            if force:
                # 🚀 强制赎回模式：有持仓 + 有价值
                size = float(pos.get('size', 0))
                current_value = float(pos.get('currentValue', 0))
                if size > 0 and current_value > 0:
                    redeemable.append(pos)
            else:
                # 正常模式：检查redeemable标志
                if pos.get('redeemable', False):
                    redeemable.append(pos)

        return redeemable

    except Exception as e:
        print(f"[ERROR] 获取持仓失败: {e}")
        return []

def redeem_position(position):
    """赎回单个持仓"""
    condition_id = position.get('conditionId', '')
    question = position.get('title', position.get('question', 'Unknown'))
    current_value = position.get('currentValue', 0)

    print(f"[REDEEM] {question[:60]}")
    print(f"  Condition ID: {condition_id[:40]}...")
    print(f"  预估价值: {current_value:.4f} USDC")

    try:
        # 标准化conditionId
        if len(condition_id) < 66:
            condition_id_padded = condition_id + '0' * (66 - len(condition_id))
        else:
            condition_id_padded = condition_id

        condition_bytes = Web3.to_bytes(hexstr=condition_id_padded)
        parent_collection_id = b'\x00' * 32
        index_sets = [1, 2]

        # 初始化CTF合约
        ctf_contract = w3.eth.contract(
            address=Web3.to_checksum_address(CTF_CONTRACT),
            abi=CTF_ABI
        )

        # 构建交易
        nonce = w3.eth.get_transaction_count(WALLET)
        gas_price = w3.eth.gas_price

        txn = ctf_contract.functions.redeemPositions(
            Web3.to_checksum_address(USDC_CONTRACT),
            parent_collection_id,
            condition_bytes,
            index_sets
        ).build_transaction({
            'from': WALLET,
            'gas': 400000,
            'maxFeePerGas': int(gas_price * 1.2),
            'maxPriorityFeePerGas': int(80 * 1e9),
            'nonce': nonce,
            'chainId': 137
        })

        # 签名并发送
        signed = w3.eth.account.sign_transaction(txn, PRIVATE_KEY)
        tx_hash = w3.eth.send_raw_transaction(signed.rawTransaction)

        print(f"  交易已发送: {tx_hash.hex()[:40]}...")

        # 等待确认
        receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=180)

        if receipt['status'] == 1:
            print(f"  ✅ 成功！区块: {receipt['blockNumber']:,}")
            return True, tx_hash.hex()
        else:
            print(f"  ❌ 失败（链上revert）")
            return False, None

    except Exception as e:
        error_msg = str(e)
        if "execution reverted" in error_msg:
            print(f"  ❌ 失败: 市场未结算")
        else:
            print(f"  ❌ 失败: {error_msg[:100]}")
        return False, None

def main(loop=True, interval_secs=60, force=False):
    """主函数

    Args:
        force: 如果为True，强制赎回模式（无视redeemable标志）
    """
    while True:
        print("=" * 80)
        mode_str = "强制赎回模式" if force else "正常赎回模式"
        print(f"云端自动赎回（legacy USDC.e collateral，{mode_str}）")
        print("=" * 80)
        print(f"钱包: {WALLET}")
        print(f"当前区块: {w3.eth.block_number:,}")
        print()

        # 获取可赎回持仓
        print_collateral_balances("START")

        print("[1/3] 检查可赎回持仓...")
        if force:
            print("  🚀 强制赎回模式：无视redeemable标志")
        positions = get_redeemable_positions(force=force)

        if not positions:
            print("没有可赎回的持仓")
            print("[CONVERT] 仍会检查钱包里是否已有历史USDCE可兑换")
            try:
                convert_usdce_to_pusd()
            except Exception as e:
                print(f"[CONVERT] ❌ 失败: {str(e)[:200]}")

            if not loop:
                return

            print(f"⏳ 等待 {interval_secs}秒后再次检查...")
            import time
            time.sleep(interval_secs)
            continue

        print(f"找到 {len(positions)} 个可赎回持仓")
        print()

        # 批量赎回
        print("[2/3] 执行赎回...")
        print("=" * 80)

        success_count = 0
        failed_count = 0

        for i, pos in enumerate(positions, 1):
            print(f"\n[{i}/{len(positions)}]", end=" ")

            success, tx_hash = redeem_position(pos)

            if success:
                success_count += 1
            else:
                failed_count += 1

        # 等待 nonce 同步（赎回多笔交易后需要等待）
        if success_count > 0:
            print("\n⏳ 等待 3 秒让 nonce 同步...")
            import time
            time.sleep(3)

        print()
        print("[3/3] 检查并按配置兑换 USDCE -> pUSD...")
        conversion_success = False
        conversion_tx = None
        try:
            conversion_success, conversion_tx = convert_usdce_to_pusd()
        except Exception as e:
            print(f"[CONVERT] ❌ 失败: {str(e)[:200]}")

        print()
        print("=" * 80)
        print(f"总结: 赎回成功 {success_count}, 赎回失败 {failed_count}, 兑换执行 {conversion_success}, 兑换TX {conversion_tx or 'N/A'}")
        print("=" * 80)

        # 强制退出，防止卡住Rust
        import sys
        sys.exit(0)
        
        if not loop:
            return
            
        print(f"⏳ 等待 {interval_secs}秒后再次扫描...")
        import time
        time.sleep(interval_secs)

if __name__ == "__main__":
    # 默认只跑一次（被Rust调用时）
    # 参数：
    #   --loop: 循环模式
    #   --force: 强制赎回模式（无视redeemable标志）
    import sys
    loop_mode = "--loop" in sys.argv
    force_mode = "--force" in sys.argv
    main(loop=loop_mode, interval_secs=60, force=force_mode)
