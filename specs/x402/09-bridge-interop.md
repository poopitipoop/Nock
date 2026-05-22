# 9. Bridge Interoperability

This document describes how x402 payments interact with the Nockchain-to-Base bridge, enabling cross-chain payment flows and interoperability with the EVM ecosystem.

## 9.1 Bridge Overview

Nockchain operates a **bridge to Base** (Ethereum L2) that enables bidirectional asset movement:

- **Deposit (Nockchain → Base):** Lock NOCK on Nockchain, mint wrapped NOCK (ERC-20) on Base
- **Withdrawal (Base → Nockchain):** Burn wrapped NOCK on Base, unlock NOCK on Nockchain

The bridge uses a **3-of-5 multisig** validator set with dedicated bridge nodes monitoring both chains.

### 9.1.1 Bridge Parameters

| Parameter | Value |
|-----------|-------|
| Validator threshold | 3 of 5 |
| Base confirmation depth | 300 blocks |
| Nockchain confirmation depth | 100 blocks |
| Minimum deposit | 1,000,000 nocks |
| Fee rate | 195 nicks per nock |
| Nock ERC-20 decimals | 16 |
| Conversion factor | 1 nick = 152,587,890,625 ERC-20 base units |

### 9.1.2 Bridge Contracts (Base)

| Contract | Purpose |
|----------|---------|
| `MessageInbox` | Processes deposits, verifies multisig, mints NOCK |
| `Nock` (ERC-20) | Wrapped NOCK token on Base (16 decimals) |

## 9.2 Cross-Chain Payment Scenarios

### 9.2.1 Scenario A: Native NOCK Client, Native NOCK Server

The simplest case — both parties use Nockchain natively.

```
Client (Nockchain)  ──x402──►  Server (Nockchain)
```

No bridge involvement. Standard `(exact, nockchain:mainnet)` flow.

### 9.2.2 Scenario B: EVM Client, Nockchain Server

A client holding USDC or wrapped NOCK on Base wants to pay a Nockchain resource server.

```
Client (Base/EVM)  ──x402 (eip155:8453)──►  Server (Nockchain)
                                               │
                                               ▼
                                          Bridge deposit
                                          (Base → Nockchain)
```

**Flow:**

1. Server returns 402 with two `PaymentRequirements` entries:
   - `(exact, nockchain:mainnet)` — pay in native NOCK
   - `(exact, eip155:8453)` — pay in USDC on Base

2. Client selects `(exact, eip155:8453)` and pays via standard EVM x402 (EIP-3009 `transferWithAuthorization`)

3. Server receives USDC on Base and either:
   - Holds the USDC directly (no bridge needed), or
   - Swaps to wrapped NOCK on Base, then bridges to native NOCK

This scenario uses the **upstream x402 EVM binding** for the payment itself. The bridge is only involved if the server wants to consolidate funds on Nockchain.

### 9.2.3 Scenario C: Nockchain Client, EVM Server

A Nockchain-native client wants to pay an EVM-based resource server.

```
Client (Nockchain)  ──bridge──►  Wrapped NOCK (Base)  ──x402 (eip155:8453)──►  Server (Base/EVM)
```

**Flow:**

1. Server returns 402 with `(exact, eip155:8453)` requirements
2. Client does not have EVM assets; it must bridge first
3. Client creates a bridge deposit transaction on Nockchain, targeting its own Base address
4. After bridge confirmation (~300 Base blocks), client has wrapped NOCK on Base
5. Client pays via standard EVM x402

This requires **advance planning** — the bridge has significant latency. Agents SHOULD maintain a pre-funded EVM wallet for cross-chain payments.

### 9.2.4 Scenario D: Dual-Network Server

A resource server accepts payment on either network, using the bridge for reconciliation:

```json
[
  {
    "scheme": "exact",
    "network": "nockchain:mainnet",
    "maxAmountRequired": "65536",
    "asset": "nock",
    "payTo": "<nockchain-pkh>"
  },
  {
    "scheme": "exact",
    "network": "eip155:8453",
    "maxAmountRequired": "1000000",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "payTo": "0x1234...abcd"
  }
]
```

The server maintains wallets on both chains and periodically rebalances via the bridge.

## 9.3 Bridge Deposit via x402

A specialized extension allows a resource server to request payment that is **directly bridged** as part of the x402 flow:

### 9.3.1 Bridge Deposit Recipient

The `PaymentRequirements` can specify a bridge deposit as the payment destination:

```json
{
  "scheme": "exact",
  "network": "nockchain:mainnet",
  "maxAmountRequired": "65536000",
  "asset": "nock",
  "payTo": "<bridge-lock-root>",
  "extra": {
    "version": "1",
    "minFee": "60000000",
    "facilitatorUrl": "https://facilitator.example.com",
    "bridgeDeposit": {
      "evmAddress": "0x1234...abcd",
      "minDepositNocks": 1000000
    }
  }
}
```

When the `extra.bridgeDeposit` field is present, the facilitator constructs a transaction with a **bridge deposit output** instead of a standard P2PKH output. The bridge validators then process the deposit and mint wrapped NOCK on Base.

### 9.3.2 Constraints

- Only one bridge deposit output per transaction
- Minimum deposit amount enforced by the bridge protocol
- Bridge fee (195 nicks/nock) is deducted by the bridge, not the x402 payment
- Deposit confirmation requires ~300 Base blocks and ~100 Nockchain blocks

## 9.4 Price Equivalence

When offering both Nockchain and EVM payment options, the resource server must establish price equivalence:

### 9.4.1 Static Pricing

The server sets fixed prices on each chain:

```
Nockchain: 65,536 nicks (1 NOCK)
Base USDC: 1,000,000 base units ($1.00)
```

The exchange rate is implicit and fixed at server configuration time.

### 9.4.2 Oracle-Based Pricing

The server queries a price oracle to dynamically set equivalent prices:

```
NOCK/USD = $X.XX
Price in nicks = (USD price / NOCK price) × 65,536
Price in USDC base units = USD price × 1,000,000
```

Oracle integration is outside the scope of this specification. Servers SHOULD cache oracle prices to avoid per-request latency.

## 9.5 Agent Cross-Chain Strategy

Agents operating across both Nockchain and EVM ecosystems should:

### 9.5.1 Maintain Dual Balances

Keep funded wallets on both Nockchain and Base to avoid bridge latency at payment time.

### 9.5.2 Prefer Native Payments

When both options are offered, prefer the native-chain payment to minimize fees and latency:
- If agent holds NOCK and server accepts NOCK: pay in NOCK
- If agent holds USDC and server accepts USDC: pay in USDC
- Only bridge when the agent's available balance is insufficient on the server's preferred chain

### 9.5.3 Proactive Bridging

Schedule bridge transfers during idle periods to maintain target balances on each chain:

```json
{
  "targetBalances": {
    "nockchain:mainnet": "6553600",
    "eip155:8453": {
      "USDC": "10000000",
      "NOCK": "10000000000000000000"
    }
  },
  "rebalanceThreshold": 0.2
}
```

When any balance drops below 20% of target, trigger a bridge transfer to replenish.

## 9.6 Bridge Security in x402 Context

### 9.6.1 Confirmation Requirements

Bridge deposits are not instant. The resource server MUST NOT rely on bridge deposit confirmation for content delivery — use standard x402 settlement (which confirms the Nockchain transaction, not the bridge deposit).

### 9.6.2 Bridge Liveness

If the bridge is halted (e.g., for maintenance or due to a security incident), cross-chain payment options become unavailable. Resource servers SHOULD:
- Monitor bridge status via the `GetStatus` gRPC endpoint
- Temporarily remove cross-chain `PaymentRequirements` entries when the bridge is down
- Fall back to native-chain-only pricing

### 9.6.3 Bridge Fee Transparency

The bridge charges fees (195 nicks/nock) that are separate from x402 payment fees. Resource servers SHOULD disclose bridge fees in the `description` field when bridge deposits are involved.
