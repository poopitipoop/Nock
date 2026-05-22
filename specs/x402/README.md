# x402 Agentic Payments for Nockchain

> **Status:** Draft
> **Version:** 0.1.0
> **Authors:** Nockchain Contributors

## Abstract

This specification defines how the [x402 protocol](https://www.x402.org/) — an open standard for HTTP-native payments — is adapted for the Nockchain network. It enables autonomous AI agents, wallets, and applications to pay for HTTP-accessible resources using NOCK (Nockchain's native asset) in a trust-minimized, programmatic fashion.

Nockchain's unique properties — a Nock-based VM, Tip5 hashing, Schnorr signatures over Cheetah curves, a UTXO model, and a bridge to Base L2 — require a dedicated `scheme × network` binding within the x402 framework. This specification provides that binding along with the agentic payment patterns it unlocks.

## Specification Documents

| # | Document | Description |
|---|----------|-------------|
| 1 | [Introduction](./01-introduction.md) | Motivation, goals, design principles |
| 2 | [Protocol Overview](./02-protocol-overview.md) | End-to-end payment flow and architecture |
| 3 | [Nockchain Primitives](./03-nockchain-primitives.md) | Cryptographic types, addressing, units |
| 4 | [Payment Requirements](./04-payment-requirements.md) | `PaymentRequirements` schema for Nockchain |
| 5 | [Payment Payload](./05-payment-payload.md) | `PaymentPayload` schema, signing, authorization |
| 6 | [Facilitator](./06-facilitator.md) | Facilitator role, `/verify` and `/settle` APIs |
| 7 | [Scheme: `exact_nock`](./07-scheme-exact-nock.md) | The `exact` payment scheme on the `nockchain` network |
| 8 | [Agentic Payments](./08-agentic-payments.md) | Autonomous agent payment flows and patterns |
| 9 | [Bridge Interoperability](./09-bridge-interop.md) | Cross-chain payments via the Base bridge |
| 10 | [Security Considerations](./10-security.md) | Threat model, mitigations, cryptographic assumptions |
| 11 | [Extensions](./11-extensions.md) | Future schemes, SIWX authentication, discovery |

## Key Design Decisions

- **Network identifier:** `nockchain:mainnet` (CAIP-2 style)
- **Payment scheme:** `exact` (transfer a specific amount of NOCK/nicks)
- **Signature scheme:** Schnorr over Cheetah (twisted Edwards in F_6), matching Nockchain's native signature format
- **Settlement model:** UTXO-based — facilitators construct and broadcast Nockchain transactions
- **Unit of account:** nicks (1 NOCK = 65,536 nicks)

## Terminology

| Term | Definition |
|------|-----------|
| **NOCK** | Nockchain's native asset |
| **nick** | Atomic unit of NOCK (1 NOCK = 65,536 nicks) |
| **Tip5** | Nockchain's sponge-based hash function producing 5×64-bit digests |
| **Belt** | A 64-bit finite field element (mod 18446744069414584321) |
| **Cheetah point** | A point on the twisted Edwards curve in F_6 extension field |
| **Note** | A UTXO on Nockchain — the fundamental unit of value |
| **Seed** | An output descriptor within a transaction |
| **Lock** | Spending conditions on a note (pubkeys + threshold) |
| **PKH** | Pay-to-pubkey-hash — Tip5 hash of a Schnorr pubkey |
| **NockApp** | A persistent, functional state machine running on the Nock VM |
| **Facilitator** | A service that verifies payment signatures and settles on-chain |
| **Resource server** | An HTTP server gating access to resources behind x402 payments |

## Protocol Version

This specification targets **x402 V2** semantics, using CAIP-2 network identifiers and the extensible `scheme × network` model.
