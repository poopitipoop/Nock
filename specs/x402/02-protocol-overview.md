# 2. Protocol Overview

## 2.1 Participants

The Nockchain x402 protocol involves four participants:

```
┌──────────┐       HTTP        ┌─────────────────┐      gRPC/HTTP     ┌─────────────┐
│  Client   │◄────────────────►│ Resource Server  │◄──────────────────►│ Facilitator │
│ (Agent /  │                  │ (NockApp / API)  │                    │  (Nockchain  │
│  Wallet)  │                  │                  │                    │   full node) │
└──────────┘                   └─────────────────┘                    └──────┬──────┘
                                                                            │
                                                                            │ broadcast tx
                                                                            ▼
                                                                     ┌─────────────┐
                                                                     │  Nockchain   │
                                                                     │   Network    │
                                                                     └─────────────┘
```

| Participant | Role |
|-------------|------|
| **Client** | Initiates HTTP requests, signs payment authorizations. Can be a human wallet, an AI agent, or any HTTP client with access to a Nockchain private key. |
| **Resource Server** | Serves HTTP resources gated behind x402 payment requirements. Can be a NockApp with an HTTP driver, a standalone API server, or any HTTP endpoint. |
| **Facilitator** | Verifies payment authorization signatures, constructs Nockchain transactions, and broadcasts them. Runs a Nockchain full node or connects to one via gRPC. |
| **Nockchain Network** | The underlying blockchain that settles payments. Provides finality through Proof of Work consensus. |

## 2.2 End-to-End Payment Flow

```
 Client                    Resource Server              Facilitator             Nockchain
   │                             │                          │                      │
   │  1. GET /resource           │                          │                      │
   │────────────────────────────►│                          │                      │
   │                             │                          │                      │
   │  2. 402 Payment Required    │                          │                      │
   │     PAYMENT-REQUIRED header │                          │                      │
   │◄────────────────────────────│                          │                      │
   │                             │                          │                      │
   │  3. Select PaymentReqs,     │                          │                      │
   │     sign authorization      │                          │                      │
   │  ┌─────────────────────┐    │                          │                      │
   │  │ Build PaymentPayload│    │                          │                      │
   │  └─────────────────────┘    │                          │                      │
   │                             │                          │                      │
   │  4. GET /resource           │                          │                      │
   │     PAYMENT-SIGNATURE hdr   │                          │                      │
   │────────────────────────────►│                          │                      │
   │                             │                          │                      │
   │                             │  5. POST /verify         │                      │
   │                             │     {payload, reqs}      │                      │
   │                             │─────────────────────────►│                      │
   │                             │                          │                      │
   │                             │                          │  6. Verify signature │
   │                             │                          │     Check UTXO state │
   │                             │                          │     Validate amount  │
   │                             │                          │                      │
   │                             │  7. Verification result  │                      │
   │                             │◄─────────────────────────│                      │
   │                             │                          │                      │
   │                             │  [If valid]              │                      │
   │                             │                          │                      │
   │  8. 200 OK + response body  │                          │                      │
   │◄────────────────────────────│                          │                      │
   │                             │                          │                      │
   │                             │  9. POST /settle         │                      │
   │                             │     {payload, reqs}      │                      │
   │                             │─────────────────────────►│                      │
   │                             │                          │                      │
   │                             │                          │  10. Construct tx    │
   │                             │                          │      Broadcast       │
   │                             │                          │─────────────────────►│
   │                             │                          │                      │
   │                             │                          │  11. Confirmation    │
   │                             │                          │◄─────────────────────│
   │                             │                          │                      │
   │                             │  12. Settlement receipt  │                      │
   │                             │◄─────────────────────────│                      │
   │                             │                          │                      │
```

### Step-by-Step Description

1. **Client requests resource.** A standard HTTP request (any method) to the resource server.

2. **Server returns 402.** The resource server responds with HTTP 402 and a base64-encoded `PaymentRequired` object in the `PAYMENT-REQUIRED` header. This object contains one or more `PaymentRequirements` entries — each specifying a `(scheme, network)` pair, amount, recipient, and asset.

3. **Client builds payload.** The client selects a `PaymentRequirements` entry it can satisfy (e.g., `scheme: "exact"`, `network: "nockchain:mainnet"`), selects one or more notes (UTXOs) to spend, and produces a Schnorr signature authorizing the payment. The result is a `PaymentPayload`.

4. **Client retries with payment.** The client re-sends the original HTTP request with the `PAYMENT-SIGNATURE` header containing the base64-encoded `PaymentPayload`.

5. **Server forwards to facilitator.** The resource server POSTs the payload and requirements to the facilitator's `/verify` endpoint.

6. **Facilitator verifies.** The facilitator checks: (a) the Schnorr signature is valid, (b) the referenced notes exist and are unspent, (c) the authorized amount meets the requirement, (d) timing constraints are satisfied.

7. **Facilitator returns result.** A verification response indicating valid/invalid with an error code if applicable.

8. **Server delivers resource.** If verification passed, the resource server returns the requested content with HTTP 200.

9. **Server requests settlement.** The resource server POSTs to the facilitator's `/settle` endpoint, requesting that the payment be executed on-chain.

10. **Facilitator constructs and broadcasts.** The facilitator builds a valid Nockchain transaction using the client's pre-authorized signature, the referenced input notes, and output seeds (one paying the resource server, one returning change to the client). It broadcasts this transaction to the Nockchain P2P network.

11. **Transaction confirms.** The Nockchain network includes the transaction in a block.

12. **Facilitator returns receipt.** The settlement response includes the transaction ID and block height (once confirmed).

## 2.3 Optimistic vs. Confirmed Settlement

The protocol supports two settlement modes:

### 2.3.1 Optimistic (Default)

The resource server delivers content immediately after verification (step 8) and settles asynchronously (steps 9-12). This provides the lowest latency for the client. The resource server trusts the facilitator to complete settlement. If settlement fails (e.g., the notes were double-spent between verify and settle), the resource server absorbs the loss.

### 2.3.2 Confirmed

The resource server waits for settlement confirmation before delivering content. This eliminates risk for the resource server at the cost of latency (Nockchain block time + confirmation depth). Appropriate for high-value resources.

The choice is a resource-server configuration decision and is not encoded in the protocol.

## 2.4 HTTP Header Format

### Request Headers

| Header | Value | Required |
|--------|-------|----------|
| `PAYMENT-SIGNATURE` | Base64-encoded `PaymentPayload` JSON | When paying |

### Response Headers

| Header | Value | When |
|--------|-------|------|
| `PAYMENT-REQUIRED` | Base64-encoded `PaymentRequired` JSON | 402 responses |
| `PAYMENT-RESPONSE` | Base64-encoded settlement receipt JSON | Successful paid responses |

### Header Encoding

All header values are the **standard base64 encoding** (RFC 4648 §4) of the UTF-8 JSON representation of the relevant data structure. Implementations MUST NOT use URL-safe base64 for headers.

## 2.5 Content Negotiation

A `PaymentRequired` object MAY contain multiple `PaymentRequirements` entries for different `(scheme, network)` pairs. For example, a resource server that accepts both native NOCK and USDC on Base could return:

```json
[
  {
    "scheme": "exact",
    "network": "nockchain:mainnet",
    "maxAmountRequired": "65536",
    "asset": "nock",
    ...
  },
  {
    "scheme": "exact",
    "network": "eip155:8453",
    "maxAmountRequired": "1000000",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    ...
  }
]
```

The client selects the entry it can fulfill based on its available funds and supported networks.
