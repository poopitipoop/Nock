# 3. Nockchain Primitives

This document defines the Nockchain-specific cryptographic types, addressing formats, and units of account used throughout the x402 payment protocol.

## 3.1 Units of Account

### 3.1.1 NOCK and Nicks

| Unit | Symbol | Relation |
|------|--------|----------|
| NOCK | NOCK | 1 NOCK = 65,536 nicks |
| nick | nick | Atomic unit — indivisible on-chain |

All x402 payment amounts on Nockchain are denominated in **nicks**. The `maxAmountRequired` field in `PaymentRequirements` and all value fields in `PaymentPayload` use nicks as the unit.

**Conversion:**

```
nicks = nock_amount × 65,536
nock_amount = nicks / 65,536
```

### 3.1.2 Display Convention

When presenting amounts to users, implementations SHOULD display in NOCK with up to 5 decimal places (since 65,536 ≈ 10^4.8). For example:

| Nicks | NOCK Display |
|-------|-------------|
| 65,536 | 1.00000 NOCK |
| 32,768 | 0.50000 NOCK |
| 1 | 0.00002 NOCK |
| 1,000,000 | 15.25879 NOCK |

### 3.1.3 Bridged NOCK (ERC-20)

On Base L2, the Nock ERC-20 token uses **16 decimals**. The conversion factor between nicks and ERC-20 base units is:

```
erc20_base_units = nicks × 152,587,890,625
```

This is derived from: `10^16 / 65,536 = 152,587,890,625`

## 3.2 Hash Function: Tip5

### 3.2.1 Overview

Nockchain uses **Tip5**, a sponge-based hash function operating over a finite field. It is used for block hashing, transaction IDs, note hashing, Merkle trees, and address derivation.

### 3.2.2 Parameters

| Parameter | Value |
|-----------|-------|
| Field prime (p) | 18,446,744,069,414,584,321 (= 2^64 - 2^32 + 1, the Goldilocks prime) |
| State width | 16 field elements |
| Rate | 8 field elements |
| Capacity | 8 field elements |
| Digest size | 5 field elements (40 bytes) |

### 3.2.3 Representation

A Tip5 hash is an array of 5 **Belt** values (see §3.3). In JSON representations within x402 payloads, Tip5 hashes are encoded as:

```json
{
  "tip5": [<u64>, <u64>, <u64>, <u64>, <u64>]
}
```

Or as a **base58-encoded** string of the 40-byte big-endian serialization (each Belt as 8 bytes big-endian, concatenated). The base58 form is used for human-readable contexts (addresses, transaction IDs).

## 3.3 Field Elements: Belt

A **Belt** is an element of the finite field F_p where p = 18,446,744,069,414,584,321.

| Property | Value |
|----------|-------|
| Size | 64 bits |
| Range | [0, p) |
| Encoding | Unsigned 64-bit integer |

In JSON: a Belt is represented as a decimal string (to avoid JavaScript integer precision issues for values > 2^53).

## 3.4 Elliptic Curve: Cheetah

### 3.4.1 Curve Parameters

Nockchain uses a **twisted Edwards curve** defined over the degree-6 extension field F_{p^6}.

| Parameter | Value |
|-----------|-------|
| Base field | F_p (Goldilocks prime) |
| Extension degree | 6 |
| Curve type | Twisted Edwards |
| Point representation | (x, y) where x, y ∈ F_{p^6} (6 Belt elements each) |

### 3.4.2 Point Encoding

A Cheetah point is 12 Belt elements (96 bytes). With a 1-byte header, the base58-encoded form is 97 bytes. Points may also carry an `inf` flag indicating the point at infinity (identity element).

## 3.5 Schnorr Signatures

### 3.5.1 Signature Format

Nockchain uses Schnorr signatures over the Cheetah curve.

```
SchnorrSignature {
  chal: [Belt; 8]   // Challenge hash (8 × 64-bit field elements)
  sig:  [Belt; 8]   // Signature scalar (8 × 64-bit field elements)
}
```

### 3.5.2 JSON Representation

```json
{
  "chal": ["<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>"],
  "sig":  ["<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>"]
}
```

### 3.5.3 Public Keys

A Schnorr public key is a Cheetah point (12 Belt elements). In x402 payloads, public keys are represented as base58 strings.

## 3.6 Addressing

### 3.6.1 Address Versions

| Version | Format | Derivation |
|---------|--------|------------|
| v0 | Base58(Cheetah point) | Direct Schnorr pubkey (97 bytes encoded) |
| v1 (current) | Base58(Tip5(pubkey)) | Pay-to-pubkey-hash (40 bytes encoded) |

The v1 PKH format is the standard for all new transactions (activated at block 40,000). x402 implementations MUST support v1 PKH addresses. Support for v0 pubkey addresses is OPTIONAL.

### 3.6.2 PKH Computation

```
pkh = Tip5(schnorr_pubkey)
```

Where `schnorr_pubkey` is the Cheetah point serialized as 12 Belt elements.

### 3.6.3 Address in x402

In `PaymentRequirements.payTo` and related fields, addresses are base58-encoded PKH strings:

```json
{
  "payTo": "<base58-pkh>"
}
```

## 3.7 Notes (UTXOs)

### 3.7.1 Note Structure

A **note** is Nockchain's UTXO — an unspent output that holds value:

| Field | Type | Description |
|-------|------|-------------|
| `version` | Version | Protocol version (v0, v1, v2) |
| `origin_page` | BlockHeight | Block in which the note was created |
| `name` | Name | Unique identifier: `(first_hash, last_hash)` |
| `note_data` | NoteData | Arbitrary key-value metadata |
| `assets` | Nicks | Amount of value held |

### 3.7.2 Note Name

A note's **name** is a pair of Tip5 hashes that uniquely identifies it:

```
Name {
  first: Tip5Hash   // Hash of lock + timelock conditions
  last:  Tip5Hash   // Hash of source / provenance
}
```

In x402 payloads, note names are referenced as:

```json
{
  "first": "<base58-tip5>",
  "last": "<base58-tip5>"
}
```

### 3.7.3 Locks

A **lock** defines the spending conditions for a note:

```
Lock {
  keys_required: u32                  // Signature threshold (m)
  schnorr_pubkeys: [SchnorrPubkey]    // Set of authorized pubkeys (n)
}
```

For standard P2PKH notes: `keys_required = 1`, `schnorr_pubkeys = [owner_pubkey]`.

## 3.8 Transaction Structure

### 3.8.1 RawTx (v1)

```
RawTx {
  version: Version
  id:      TxId         // Tip5 hash of the transaction
  spends:  Spends       // Map of (Name → Spend)
}
```

### 3.8.2 Spend

A spend consumes a note and produces outputs:

```
Spend {
  witness: Witness      // Proof of authorization
  seeds:   [Seed]       // Output descriptors
  fee:     Nicks        // Transaction fee
}
```

### 3.8.3 Witness (v1)

```
Witness {
  lock_merkle_proof: LockMerkleProof
  pkh_signature:     PkhSignature      // Schnorr signature with PKH binding
  hax:               [HaxPreimage]     // Hash preimages (for HTLC-like conditions)
}
```

### 3.8.4 Seed (Output)

```
Seed {
  output_source:   Option<Source>        // Provenance commitment
  recipient:       Lock                  // Spending conditions for the new note
  timelock_intent: Option<TimelockIntent>
  gift:            Nicks                 // Amount to assign
  parent_hash:     Tip5Hash              // Hash of the parent spend
}
```

## 3.9 Network Identifiers

Following CAIP-2 style conventions:

| Network | Identifier | Description |
|---------|------------|-------------|
| Mainnet | `nockchain:mainnet` | Production network |
| Fakenet | `nockchain:fakenet` | Local development / testing |

The namespace `nockchain` is used for all Nockchain networks. The reference is the network name as a human-readable string.

## 3.10 Key Derivation

Nockchain supports BIP39 seed phrases with SLIP-10 derivation:

1. Generate 12-word BIP39 mnemonic
2. Derive master key via Argon2 + BIP32
3. Generate child keys (hardened/non-hardened)
4. Compute Schnorr pubkey and PKH for each derived key

For x402 clients (especially agents), key derivation allows generating purpose-specific payment keys without exposing the master key. Agents SHOULD use dedicated child keys for x402 payments.
