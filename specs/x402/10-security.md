# 10. Security Considerations

This document analyzes the threat model for x402 payments on Nockchain and specifies required mitigations.

## 10.1 Threat Model

### 10.1.1 Actors

| Actor | Trust Level | Capabilities |
|-------|-------------|-------------|
| **Client** | Untrusted | Can forge requests, attempt double-spends, submit invalid signatures |
| **Resource Server** | Semi-trusted | Serves content, knows payment requirements, can inflate prices |
| **Facilitator** | Semi-trusted | Verifies and settles payments, has UTXO set visibility |
| **Network Observer** | Untrusted | Can observe P2P traffic, HTTP traffic (if unencrypted), on-chain transactions |
| **Nockchain Miner** | Untrusted | Can reorder, censor, or delay transaction inclusion |

### 10.1.2 Assets Under Protection

1. Client funds (notes/UTXOs)
2. Resource server revenue (payment settlement)
3. Payment authorization confidentiality
4. Transaction integrity
5. Agent autonomy (freedom from manipulation)

## 10.2 Cryptographic Security

### 10.2.1 Signature Scheme

Nockchain uses Schnorr signatures over the Cheetah curve (twisted Edwards in F_{p^6}).

**Assumptions:**
- Discrete logarithm problem is hard in the Cheetah curve group
- The Goldilocks prime field (p = 2^64 - 2^32 + 1) provides adequate security margin
- Schnorr signatures provide existential unforgeability under chosen-message attack (EUF-CMA)

**Risks:**
- Novel curve — less cryptanalysis than secp256k1 or ed25519
- Extension field arithmetic increases implementation complexity

**Mitigations:**
- Implementations MUST use the reference `nockchain-math` library for all curve operations
- Signature verification MUST be constant-time to prevent timing side channels

### 10.2.2 Hash Function

Tip5 is a sponge-based hash over the Goldilocks field.

**Assumptions:**
- Collision resistance: finding x ≠ y such that Tip5(x) = Tip5(y) requires ~2^{160} operations (birthday bound on 320-bit output)
- Preimage resistance: finding x given Tip5(x) requires ~2^{320} operations

**Risks:**
- Novel hash function — less cryptanalysis than SHA-256 or BLAKE3
- Algebraic structure (field-based) may be vulnerable to algebraic attacks

**Mitigations:**
- The 40-byte (320-bit) digest provides ample security margin
- STARK proof system provides additional verification guarantees

### 10.2.3 Domain Separation

x402 payment signatures use the domain separator `"x402-nockchain-v2"` to prevent cross-protocol signature reuse.

**Critical property:** A valid x402 payment signature MUST NOT be usable as:
- A regular Nockchain transaction signature
- A message signature (via `sign-message`)
- A hash signature (via `sign-hash`)

This is enforced by the domain separator being included in the Tip5 sponge input before any authorization data.

## 10.3 Replay Attacks

### 10.3.1 Nonce Replay

**Attack:** An attacker captures a `PaymentPayload` and resubmits it to settle the payment multiple times.

**Mitigation:** The facilitator maintains a persistent nonce store. Each nonce can only be settled once. Settlement is idempotent — resubmitting returns the original receipt, not a new transaction.

### 10.3.2 Cross-Facilitator Replay

**Attack:** A `PaymentPayload` settled on facilitator A is replayed on facilitator B.

**Mitigation:** The nonce prevents on-chain double-settlement because the same notes cannot be spent twice. However, facilitator B may return a false "valid" from `/verify` if it doesn't share nonce state with facilitator A.

**Recommendation:** Resource servers SHOULD use a single facilitator per payment flow. If multiple facilitators are in use, they MUST share nonce state or the resource server MUST verify settlement on-chain before delivering content.

### 10.3.3 Cross-Resource Replay

**Attack:** A payment authorized for resource A is used to access resource B.

**Mitigation:** The `resource` field in `PaymentRequirements` binds the payment to a specific URL. The facilitator SHOULD verify that the `resource` field matches between the payload and requirements. Additionally, the nonce is derived from the resource URL (see §5.5.1).

### 10.3.4 Cross-Network Replay

**Attack:** A payment authorized for `nockchain:fakenet` is replayed on `nockchain:mainnet`.

**Mitigation:** The `network` field is part of the `PaymentPayload` and is checked during verification. The UTXO sets of different networks are disjoint, so even if verification passes, settlement would fail.

## 10.4 Double-Spending

### 10.4.1 UTXO Double-Spend

**Attack:** A client authorizes an x402 payment referencing note N, then spends note N in a separate Nockchain transaction before the x402 settlement occurs.

**Impact:** The facilitator cannot settle the payment. The resource server has already delivered content (in optimistic mode).

**Mitigations:**
- **Short timeout windows** reduce the double-spend window
- **Confirmed settlement mode** eliminates this risk entirely (at the cost of latency)
- **Note locking** at the facilitator level: after `/verify`, the facilitator reserves the notes and rejects other payments referencing them until settlement completes or times out
- **Reputation systems** track clients who attempt double-spends

### 10.4.2 Concurrent x402 Double-Spend

**Attack:** A client authorizes two x402 payments referencing the same note to two different resource servers simultaneously.

**Mitigations:**
- The first settlement to be mined wins; the second fails
- Clients using proper UTXO management (marking notes as pending) avoid this naturally
- Facilitators implementing note locking prevent this at the verification layer

## 10.5 Facilitator Risks

### 10.5.1 Malicious Facilitator

**Attack:** A facilitator attempts to steal funds by constructing a transaction that pays itself instead of the resource server.

**Why this fails:** The client's Schnorr signature commits to the exact recipient (`to`) and amount (`value`). The facilitator cannot construct a valid transaction that deviates from these parameters without invalidating the signature.

**Remaining risk:** A facilitator could:
- **Censor payments** (refuse to settle)
- **Delay payments** (settle slowly)
- **Deny service** (return false negatives from `/verify`)

**Mitigations:**
- Clients and servers can switch facilitators
- Self-facilitation eliminates the dependency entirely
- Multiple facilitators can be used as fallbacks

### 10.5.2 Facilitator Availability

If a facilitator goes offline, payments cannot be verified or settled.

**Mitigations:**
- Resource servers SHOULD configure fallback facilitators
- High-availability deployment of facilitator infrastructure
- Self-facilitation for critical services

### 10.5.3 Facilitator Privacy

The facilitator sees all payment details: payer address, payee address, amount, and resource URL. This is a privacy concern.

**Mitigations:**
- Self-facilitation eliminates the third-party observer
- Future: ZK-proof-based verification could hide payment details from the facilitator (see §11 Extensions)

## 10.6 Resource Server Risks

### 10.6.1 Price Inflation

**Attack:** A resource server increases prices dynamically to extract maximum value from agents.

**Mitigations:**
- Agents enforce `maxPerPayment` budget policies
- Agents cache and compare historical prices
- Competitive markets incentivize fair pricing

### 10.6.2 Content Manipulation

**Attack:** A resource server delivers incorrect, incomplete, or malicious content after payment.

**Mitigations:**
- `outputSchema` in `PaymentRequirements` enables content validation
- Reputation tracking across interactions
- Dispute resolution (out of scope for this spec)

### 10.6.3 402 Flood

**Attack:** A server returns 402 for every request, causing agents to drain funds.

**Mitigations:**
- Per-domain spending limits in agent budget policies
- Circuit breakers after repeated failures
- Agent reasoning about whether a 402 is legitimate

## 10.7 Transport Security

### 10.7.1 TLS Requirement

All HTTP communication in the x402 flow MUST use TLS 1.2 or later. This includes:
- Client ↔ Resource Server
- Resource Server ↔ Facilitator

Without TLS:
- Payment signatures in `PAYMENT-SIGNATURE` headers could be intercepted
- `PAYMENT-REQUIRED` headers could be tampered with (e.g., changing `payTo` to an attacker's address)

### 10.7.2 Certificate Validation

Agents MUST perform standard TLS certificate validation. Agents SHOULD pin certificates for known facilitator endpoints.

## 10.8 UTXO Privacy

### 10.8.1 Payment Graph Analysis

On-chain observers can trace the flow of funds from x402 payments:
- Payer address → Resource server address (payment output)
- Payer address → Payer address (change output)

This reveals which addresses are paying for which services.

**Mitigations:**
- Use dedicated payment keys (not the main wallet key) for x402
- Rotate payment keys periodically
- Future: ZK-based payment schemes could hide the transaction graph

### 10.8.2 Note Linkability

If an agent reuses the same change address across multiple payments, all payments are linkable to the same entity.

**Mitigations:**
- Generate a fresh change address for each payment
- Use hierarchical deterministic key derivation to produce unlimited change addresses

## 10.9 Denial of Service

### 10.9.1 Facilitator DoS

**Attack:** Flood the facilitator's `/verify` endpoint with invalid payloads.

**Mitigations:**
- Rate limiting per IP / per payer address
- Lightweight schema validation before expensive cryptographic verification
- Proof-of-work challenge for anonymous requests (leveraging Nockchain's existing PoW infrastructure)

### 10.9.2 UTXO Set Query Amplification

**Attack:** Submit payloads referencing many notes, causing the facilitator to perform expensive UTXO lookups.

**Mitigations:**
- Limit the maximum number of input notes per payment (RECOMMENDED: 5)
- Cache UTXO query results
- Reject payloads with unreasonable note counts before querying

## 10.10 Implementation Requirements

### 10.10.1 MUST

- Use TLS for all HTTP communication
- Validate Schnorr signatures using constant-time operations
- Enforce domain separation in signature construction
- Maintain persistent nonce stores with crash recovery
- Reject expired time windows
- Verify PKH binding (Tip5(pubkey) == from)

### 10.10.2 SHOULD

- Implement note locking during verify-settle window
- Rate limit facilitator endpoints
- Log all payment events for audit
- Support certificate pinning for facilitator connections
- Generate fresh change addresses per payment

### 10.10.3 MUST NOT

- Store private keys in plaintext
- Accept payment signatures without domain separation verification
- Allow unsigned or self-signed TLS certificates in production
- Trust facilitator responses without on-chain verification (for high-value payments)
