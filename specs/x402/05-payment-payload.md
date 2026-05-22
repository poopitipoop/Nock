# 5. Payment Payload

This document defines the `PaymentPayload` schema for the `(exact, nockchain)` scheme-network pair, including the signing procedure and authorization structure.

## 5.1 PaymentPayload Envelope

The client sends the `PAYMENT-SIGNATURE` header containing a base64-encoded JSON `PaymentPayload`:

```
PAYMENT-SIGNATURE: <base64(JSON(PaymentPayload))>
```

## 5.2 PaymentPayload Schema

```json
{
  "x402Version": 2,
  "scheme": "exact",
  "network": "nockchain:mainnet",
  "payload": {
    "signature": { ... },
    "authorization": { ... }
  }
}
```

### 5.2.1 Envelope Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `x402Version` | number | Yes | Protocol version. MUST be `2`. |
| `scheme` | string | Yes | MUST match the selected `PaymentRequirements.scheme`. |
| `network` | string | Yes | MUST match the selected `PaymentRequirements.network`. |
| `payload` | object | Yes | Scheme- and network-specific payload (see §5.3). |

## 5.3 Nockchain Exact Payload

The `payload` field for `(exact, nockchain)` contains two sub-objects:

```json
{
  "signature": {
    "pubkey": "<base58-schnorr-pubkey>",
    "schnorr": {
      "chal": ["<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>"],
      "sig":  ["<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>", "<u64>"]
    }
  },
  "authorization": {
    "from": "<base58-pkh-of-payer>",
    "to": "<base58-pkh-of-payee>",
    "value": "<nicks as string>",
    "fee": "<nicks as string>",
    "nonce": "<base58-tip5-hash>",
    "validAfter": <unix-timestamp>,
    "validBefore": <unix-timestamp>,
    "notes": [
      {
        "name": {
          "first": "<base58-tip5>",
          "last": "<base58-tip5>"
        },
        "assets": "<nicks as string>"
      }
    ],
    "changeAddress": "<base58-pkh>"
  }
}
```

### 5.3.1 Signature Object

| Field | Type | Description |
|-------|------|-------------|
| `pubkey` | string | Base58-encoded Schnorr public key of the payer. |
| `schnorr` | object | The Schnorr signature over the authorization message. |
| `schnorr.chal` | string[8] | Challenge hash — 8 Belt values as decimal strings. |
| `schnorr.sig` | string[8] | Signature scalar — 8 Belt values as decimal strings. |

### 5.3.2 Authorization Object

| Field | Type | Description |
|-------|------|-------------|
| `from` | string | Base58-encoded PKH of the payer. MUST correspond to the public key in `signature.pubkey`. |
| `to` | string | Base58-encoded PKH of the payee. MUST match `PaymentRequirements.payTo`. |
| `value` | string | Amount to transfer in nicks. MUST be ≥ `PaymentRequirements.maxAmountRequired`. |
| `fee` | string | Transaction fee in nicks. MUST be ≥ `PaymentRequirements.extra.minFee`. |
| `nonce` | string | Unique nonce for replay protection. Base58-encoded Tip5 hash. See §5.5. |
| `validAfter` | number | Unix timestamp (seconds). The payment is not valid before this time. |
| `validBefore` | number | Unix timestamp (seconds). The payment is not valid after this time. |
| `notes` | array | The input notes (UTXOs) being spent. See §5.3.3. |
| `changeAddress` | string | Base58-encoded PKH where change (excess value) should be returned. |

### 5.3.3 Note References

Each entry in the `notes` array identifies a specific UTXO:

```json
{
  "name": {
    "first": "<base58-tip5>",
    "last": "<base58-tip5>"
  },
  "assets": "<nicks as string>"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name.first` | string | First hash of the note name. |
| `name.last` | string | Last hash of the note name. |
| `assets` | string | The amount held by this note (for verification). |

The total value of all referenced notes MUST be ≥ `value` + `fee`.

## 5.4 Signing Procedure

### 5.4.1 Message Construction

The authorization message is the data that the payer signs. It is constructed deterministically from the `authorization` object:

```
sign_message = Tip5(
  "x402-nockchain-v2"     ||   // Domain separator (UTF-8, padded to Belt)
  from                     ||   // PKH as 5 Belts
  to                       ||   // PKH as 5 Belts
  value                    ||   // Amount as Belt
  fee                      ||   // Fee as Belt
  nonce                    ||   // Nonce as 5 Belts
  validAfter               ||   // Timestamp as Belt
  validBefore              ||   // Timestamp as Belt
  notes_hash               ||   // Tip5 hash of sorted note names
  changeAddress                 // Change PKH as 5 Belts
)
```

Where:
- `||` denotes concatenation of field elements into the Tip5 sponge input
- `notes_hash` = `Tip5(name_1.first || name_1.last || name_2.first || name_2.last || ...)`
- Notes are sorted lexicographically by `(first, last)` before hashing

### 5.4.2 Signing

The payer computes the Schnorr signature:

```
schnorr_sig = SchnorrSign(private_key, sign_message)
```

The signature is computed over the Cheetah curve using the standard Nockchain Schnorr signing algorithm.

### 5.4.3 Domain Separation

The `"x402-nockchain-v2"` domain separator ensures that x402 payment signatures cannot be replayed as regular Nockchain transaction signatures or message signatures. This is critical because the same private key may be used for both purposes.

## 5.5 Nonce Generation

The nonce serves dual purposes:

1. **Replay protection:** Prevents the same payment authorization from being settled multiple times.
2. **Uniqueness:** Ensures each payment authorization produces a unique signature.

### 5.5.1 Construction

```
nonce = Tip5(
  "x402-nonce"         ||   // Domain separator
  from                  ||   // Payer PKH
  resource_url          ||   // UTF-8 of PaymentRequirements.resource
  timestamp             ||   // Current Unix timestamp as Belt
  random_bytes               // 32 bytes of cryptographic randomness
)
```

### 5.5.2 Requirements

- Each `PaymentPayload` MUST use a unique nonce.
- The facilitator MUST track settled nonces and reject duplicates.
- Nonce storage MUST persist across facilitator restarts.

## 5.6 Time Bounds

### 5.6.1 Constraints

- `validAfter` MUST be ≤ the current time when the facilitator receives the payload.
- `validBefore` MUST be > the current time when the facilitator receives the payload.
- `validBefore - validAfter` SHOULD be ≤ `PaymentRequirements.maxTimeoutSeconds`.
- `validBefore - validAfter` MUST be ≤ 3600 (1 hour). Implementations SHOULD reject wider windows.

### 5.6.2 Clock Tolerance

Facilitators SHOULD allow up to 30 seconds of clock skew when evaluating time bounds.

## 5.7 Note Selection

### 5.7.1 Requirements

The client MUST select notes such that:

```
sum(note.assets for note in notes) >= value + fee
```

### 5.7.2 Recommendations

- **Minimize inputs.** Fewer notes means smaller transactions and lower fees.
- **Prefer exact amounts.** If a single note covers `value + fee` exactly, no change output is needed.
- **Avoid concurrent spending.** Notes referenced in an x402 payment MUST NOT be simultaneously used in other transactions. Clients SHOULD mark notes as "pending" when used in a payment authorization.

### 5.7.3 Change Handling

If the total input value exceeds `value + fee`, the difference is sent to `changeAddress`:

```
change = sum(note.assets) - value - fee
```

The facilitator constructs the transaction with:
- One output seed paying `value` to `to` (the resource server)
- One output seed paying `change` to `changeAddress` (if change > 0)

## 5.8 Example PaymentPayload

```json
{
  "x402Version": 2,
  "scheme": "exact",
  "network": "nockchain:mainnet",
  "payload": {
    "signature": {
      "pubkey": "5Ht7Rk3qX9...",
      "schnorr": {
        "chal": ["12345678901234567", "12345678901234567", "12345678901234567", "12345678901234567",
                  "12345678901234567", "12345678901234567", "12345678901234567", "12345678901234567"],
        "sig":  ["12345678901234567", "12345678901234567", "12345678901234567", "12345678901234567",
                  "12345678901234567", "12345678901234567", "12345678901234567", "12345678901234567"]
      }
    },
    "authorization": {
      "from": "3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy",
      "to": "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa",
      "value": "65536",
      "fee": "10",
      "nonce": "2NEpo7TZRhna7JNR...",
      "validAfter": 1708000000,
      "validBefore": 1708000300,
      "notes": [
        {
          "name": {
            "first": "4vJ9JU1bJJE...",
            "last": "7iYDhLfEgN..."
          },
          "assets": "131072"
        }
      ],
      "changeAddress": "3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy"
    }
  }
}
```
