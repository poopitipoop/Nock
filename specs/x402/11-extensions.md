# 11. Extensions

This document describes planned and potential extensions to the Nockchain x402 protocol. These are not part of the core specification but represent the roadmap for future capabilities.

## 11.1 Scheme: `upto` (Consumption-Based Payments)

### 11.1.1 Motivation

The `exact` scheme requires the price to be known upfront. For resources where cost depends on consumption — such as LLM token generation, streaming data, or compute-on-demand — a consumption-based scheme is needed.

### 11.1.2 Semantics

The `upto` scheme authorizes payment of **up to** a maximum amount. The actual amount settled depends on the resources consumed:

```json
{
  "scheme": "upto",
  "network": "nockchain:mainnet",
  "maxAmountRequired": "655360",
  "description": "LLM inference, up to 10 NOCK",
  "extra": {
    "version": "1",
    "minFee": "10",
    "unitPrice": "64",
    "unit": "token",
    "estimatedUnits": "10000"
  }
}
```

### 11.1.3 Flow Differences

1. Client authorizes up to `maxAmountRequired` nicks
2. Server delivers the resource (e.g., streams tokens)
3. Server reports actual consumption to the facilitator
4. Facilitator settles for the actual amount consumed (≤ `maxAmountRequired`)
5. Excess authorization is void — the client's remaining notes are not locked

### 11.1.4 Design Challenges

- **Trust:** The server reports consumption. A malicious server could over-report.
- **Verification:** How does the facilitator verify that the reported consumption is accurate?
- **Partial settlement:** The facilitator must construct a transaction for a different amount than the authorized maximum.

### 11.1.5 Potential Approaches

- **Metered proofs:** The server provides a cryptographic proof of consumption (e.g., hash chain of delivered content).
- **Client-side metering:** The client counts units received and reports to the facilitator.
- **Escrow notes:** The full authorized amount is locked in an escrow note with a time-delayed refund for the unused portion.

Status: **Research phase.** Not ready for specification.

## 11.2 Sign-In-With-Nockchain (SIWN)

### 11.2.1 Motivation

After a client has paid for a resource, they may want to re-access it without paying again. SIWN enables wallet-based authentication to prove the client has previously paid.

### 11.2.2 Based on CAIP-122

Following the x402 SIWX (Sign-In-With-X) extension and CAIP-122:

```
Nockchain namespace: nockchain
Chain reference: mainnet or fakenet
Account address: <base58-pkh>
```

Full CAIP-10 account ID: `nockchain:mainnet:<base58-pkh>`

### 11.2.3 Authentication Flow

1. Server returns 402 with a `sign-in-with-x` extension in `extra`:
   ```json
   {
     "extra": {
       "siwn": {
         "domain": "api.example.com",
         "nonce": "<random-nonce>",
         "issuedAt": "2026-02-17T12:00:00Z",
         "expirationTime": "2026-02-17T13:00:00Z"
       }
     }
   }
   ```

2. Client checks if it has previously paid this server from this address

3. If yes, client signs a SIWN message:
   ```
   api.example.com wants you to sign in with your Nockchain account:
   <base58-pkh>

   URI: https://api.example.com/weather
   Version: 1
   Chain ID: nockchain:mainnet
   Nonce: <random-nonce>
   Issued At: 2026-02-17T12:00:00Z
   Expiration Time: 2026-02-17T13:00:00Z
   ```

4. Client sends `SIGN-IN-WITH-X` header with the signed message

5. Server verifies:
   - Signature is valid for the claimed PKH
   - PKH has a prior payment record for this resource
   - Nonce is fresh, domain matches, timestamps are valid

6. Server grants access without requiring new payment

### 11.2.4 Signature Type

SIWN uses Nockchain's native Schnorr signature. The message is hashed with Tip5 using a `"siwn-v1"` domain separator.

### 11.2.5 Payment Tracking

Servers implementing SIWN must maintain a payment tracking store:

```
hasPaid(pkh, resourceUri) → bool
recordPayment(pkh, resourceUri, txId, timestamp) → void
```

Access policies (e.g., "paid once, access forever" vs. "paid per day") are server-defined.

Status: **Design phase.** Aligns with upstream SIWX extension.

## 11.3 Bazaar (Service Discovery)

### 11.3.1 Motivation

Agents need to discover paid services. A decentralized registry of x402-enabled endpoints would enable:
- Comparison shopping across providers
- Automated service discovery
- Quality and reputation tracking

### 11.3.2 Concept

The Bazaar is a decentralized catalog of x402-gated resources:

```json
{
  "service": "weather-data",
  "providers": [
    {
      "url": "https://api.weather1.com/current",
      "scheme": "exact",
      "network": "nockchain:mainnet",
      "price": "65536",
      "reputation": 4.8,
      "uptime": 0.999
    },
    {
      "url": "https://api.weather2.com/v1/now",
      "scheme": "exact",
      "network": "nockchain:mainnet",
      "price": "32768",
      "reputation": 4.2,
      "uptime": 0.95
    }
  ]
}
```

### 11.3.3 Implementation Options

1. **On-chain registry:** Publish service listings as Nockchain note data. Costly but censorship-resistant.
2. **NockApp registry:** A dedicated NockApp that maintains a service catalog, queryable via its HTTP API.
3. **DHT-based:** Use Nockchain's libp2p Kademlia DHT to distribute service listings across the P2P network.
4. **Centralized index:** A traditional web service that crawls and indexes x402 endpoints (simplest, least decentralized).

Status: **Concept phase.** Aligns with upstream x402 Bazaar extension.

## 11.4 Multisig Payments

### 11.4.1 Motivation

Organizations and multi-agent systems may require M-of-N approval for payments.

### 11.4.2 Design Sketch

The `PaymentPayload` is extended to include multiple signatures:

```json
{
  "payload": {
    "signatures": [
      { "pubkey": "<pk1>", "schnorr": { ... } },
      { "pubkey": "<pk2>", "schnorr": { ... } }
    ],
    "authorization": {
      "from": "<multisig-lock-hash>",
      "threshold": 2,
      ...
    }
  }
}
```

The facilitator verifies that at least `threshold` valid signatures are present.

### 11.4.3 Workflow

1. Agent A initiates the payment and signs
2. Agent A sends the partial payload to Agent B for co-signing
3. Agent B signs and submits the completed payload

This requires an out-of-band coordination mechanism (e.g., shared file, gRPC, or another x402-gated endpoint).

Status: **Design phase.** Nockchain already supports multisig transactions natively.

## 11.5 Timelocked Payments

### 11.5.1 Motivation

Escrow, subscription, and delayed-payment patterns require timelocks.

### 11.5.2 Design Sketch

The authorization object includes timelock constraints:

```json
{
  "authorization": {
    "timelock": {
      "type": "absolute",
      "unlockHeight": 50000
    }
  }
}
```

The facilitator constructs the output seed with a `TimelockIntent`:
- **Absolute:** Note is unspendable until block height N
- **Relative:** Note is unspendable for N blocks after creation

### 11.5.3 Use Cases

- **Escrow:** Payment locked until a condition is met (manual release or oracle trigger)
- **Subscription:** Weekly unlock of notes for recurring service access
- **Refund window:** Payment is timelocked; if the service fails to deliver, the client reclaims funds before unlock

Status: **Design phase.** Nockchain supports timelocks natively.

## 11.6 ZK-Private Payments

### 11.6.1 Motivation

x402 payments are visible on-chain — the payer, payee, and amount are all public. For privacy-sensitive use cases, zero-knowledge proofs could hide these details.

### 11.6.2 Concept

Nockchain's STARK-based proof system (`zkvm-jetpack`) could be leveraged to:

1. **Prove payment authorization** without revealing the payer's address
2. **Prove sufficient balance** without revealing exact amounts
3. **Prove note ownership** without revealing which notes are being spent

### 11.6.3 Challenges

- STARK proofs are computationally expensive to generate
- Verification of ZK proofs on-chain adds to transaction size
- Privacy-preserving UTXO models (like Zcash's) require significant protocol changes

Status: **Research phase.** Long-term goal.

## 11.7 Payment Channels

### 11.7.1 Motivation

For high-frequency micropayments (e.g., per-token LLM inference), on-chain settlement per payment is impractical. Payment channels enable off-chain payments with periodic on-chain settlement.

### 11.7.2 Concept

1. Client opens a payment channel by locking a note on-chain with a 2-of-2 multisig (client + server)
2. For each micropayment, the client signs an updated channel state (incrementing the server's balance)
3. Either party can close the channel by submitting the latest signed state on-chain

### 11.7.3 x402 Integration

```json
{
  "scheme": "channel",
  "network": "nockchain:mainnet",
  "extra": {
    "channelId": "<tip5-hash>",
    "currentBalance": "500000",
    "unitPrice": "1"
  }
}
```

The `PAYMENT-SIGNATURE` header would contain a channel state update instead of a transaction authorization.

Status: **Concept phase.** Requires significant protocol work.

## 11.8 Extension Registry

Future extensions should be registered to prevent conflicts:

| Extension | `extra` field | Status |
|-----------|---------------|--------|
| Bridge deposit | `extra.bridgeDeposit` | Specified (§9.3) |
| SIWN | `extra.siwn` | Design phase |
| Multisig | `extra.multisig` | Design phase |
| Timelock | `extra.timelock` | Design phase |
| Channel | `extra.channel` | Concept phase |
| ZK-private | `extra.zkPrivate` | Research phase |

Extensions MUST NOT reuse field names. New extensions SHOULD be proposed via the Nockchain governance process.
