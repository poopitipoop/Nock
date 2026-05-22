# 1. Introduction

## 1.1 Motivation

The HTTP 402 "Payment Required" status code has existed since HTTP/1.1 (RFC 7231) but was reserved for future use. The x402 protocol activates this dormant status code as a machine-readable payment layer for the web.

Nockchain — a ZK-Proof of Work blockchain with a Nock-based VM — is uniquely positioned for x402 adoption:

1. **Programmable gold meets programmable payments.** Nockchain combines sound-money PoW incentives with a fully programmable execution environment. x402 gives this programmable money a native HTTP interface.

2. **NockApps are HTTP-native.** The NockApp framework already supports HTTP server drivers. Adding x402 payment gating to a NockApp is a natural extension of its existing HTTP capabilities.

3. **Agent-friendly architecture.** Nockchain's functional, deterministic execution model and simple UTXO structure make it well-suited for autonomous AI agents that need to reason about payments programmatically.

4. **Bridge to the broader ecosystem.** The existing Nockchain-to-Base bridge means x402 payments can flow between Nockchain-native services and the EVM ecosystem, enabling cross-chain agentic commerce.

## 1.2 Goals

This specification aims to:

- **G1:** Define the `(exact, nockchain)` scheme-network pair so that any x402-compatible client can pay for resources priced in NOCK.
- **G2:** Specify the facilitator interface for Nockchain, including UTXO-based verification and settlement.
- **G3:** Describe agentic payment patterns — how autonomous AI agents discover, authorize, and execute x402 payments on Nockchain.
- **G4:** Define interoperability with the Base bridge, so services can accept payment in either native NOCK or bridged NOCK (ERC-20 on Base).
- **G5:** Maintain alignment with the upstream x402 V2 specification so that Nockchain is a first-class participant in the multi-chain x402 ecosystem.

## 1.3 Design Principles

### 1.3.1 Trust Minimization

The facilitator must never be able to redirect funds. Payment authorization signatures commit to the exact recipient, amount, and conditions. The facilitator's role is limited to broadcasting a pre-authorized transaction.

### 1.3.2 UTXO Awareness

Unlike EVM-based x402 implementations that use `transferWithAuthorization` (EIP-3009), Nockchain uses a UTXO model. Payment authorization must reference specific notes (UTXOs) and produce valid Nockchain transactions. This has implications for change handling, note selection, and concurrent payment safety.

### 1.3.3 Minimal Integration Surface

For resource servers: a single middleware function that returns 402 responses with payment requirements and validates incoming payment headers.

For clients: a single function that inspects 402 responses, constructs a payment payload, and retries the request.

### 1.3.4 Deterministic Verification

All verification logic must be deterministic and reproducible. Given the same `PaymentPayload` and `PaymentRequirements`, any compliant implementation must reach the same verification result.

### 1.3.5 Forward Compatibility

The specification uses versioned schemas and extensible `extra` fields to accommodate future payment schemes, lock types, and network features without breaking existing implementations.

## 1.4 Scope

### In Scope

- The `exact` payment scheme on the `nockchain:mainnet` and `nockchain:fakenet` networks
- Facilitator `/verify` and `/settle` API contracts
- Schnorr signature-based payment authorization
- P2PKH (pay-to-pubkey-hash) payment flows
- Bridge-mediated cross-chain payments
- Agentic payment patterns for AI agents

### Out of Scope

- Multisig-gated x402 payments (future extension)
- Timelock-conditioned x402 payments (future extension)
- The `upto` (consumption-based) scheme (future extension)
- ZK-proof integration for private payment authorization (future research)
- Fiat on/off ramp integration

## 1.5 Relationship to Upstream x402

This specification is a **scheme-network binding** within the x402 framework. It does not modify the core x402 protocol. Specifically:

- The HTTP headers (`PAYMENT-REQUIRED`, `PAYMENT-SIGNATURE`, `PAYMENT-RESPONSE`) are unchanged.
- The `PaymentRequirements` and `PaymentPayload` envelope structures are unchanged.
- The facilitator `/verify` and `/settle` endpoints conform to the standard x402 API contract.
- Only the `payload` field within `PaymentPayload` and the `extra` field within `PaymentRequirements` carry Nockchain-specific data.

Any x402 client that supports dynamic scheme-network registration can interact with Nockchain resources without code changes beyond adding a Nockchain payment adapter.
