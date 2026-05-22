# 6. Facilitator

This document defines the Nockchain x402 facilitator — its role, API surface, verification logic, and settlement procedure.

## 6.1 Role

The facilitator is a trusted intermediary that:

1. **Verifies** payment authorizations without executing them (off-chain check)
2. **Settles** payments by constructing and broadcasting Nockchain transactions (on-chain execution)
3. **Abstracts complexity** so that resource servers do not need to run full nodes or manage UTXO state

The facilitator **cannot** redirect funds. The Schnorr signature in the `PaymentPayload` commits to the exact recipient (`to`), amount (`value`), and change address (`changeAddress`). The facilitator can only construct a transaction that satisfies these constraints.

## 6.2 Infrastructure Requirements

A Nockchain facilitator MUST:

- Run a Nockchain full node or maintain a gRPC connection to one
- Have access to the current UTXO set (to verify note existence and unspent status)
- Have the ability to broadcast transactions to the P2P network
- Maintain a persistent nonce store for replay protection
- Expose an HTTP API with `/verify` and `/settle` endpoints

A facilitator does NOT need:

- Any private keys (it never signs transactions — the client's pre-signed authorization is used)
- Custody of any funds
- Access to the client's private key material

## 6.3 API Endpoints

### 6.3.1 POST /verify

Validates a `PaymentPayload` against `PaymentRequirements` without executing any on-chain action.

**Request:**

```json
{
  "payload": { ... },
  "requirements": { ... }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `payload` | PaymentPayload | The client's payment authorization |
| `requirements` | PaymentRequirements | The resource server's payment requirements |

**Response (success):**

```json
{
  "valid": true
}
```

**Response (failure):**

```json
{
  "valid": false,
  "error": {
    "code": "<error_code>",
    "message": "<human-readable description>"
  }
}
```

### 6.3.2 POST /settle

Executes the payment on-chain. The facilitator constructs a Nockchain transaction, broadcasts it, and returns the result.

**Request:**

```json
{
  "payload": { ... },
  "requirements": { ... }
}
```

Same schema as `/verify`.

**Response (success):**

```json
{
  "success": true,
  "transaction": {
    "txId": "<base58-tip5-hash>",
    "blockHeight": null,
    "status": "broadcast"
  }
}
```

The `blockHeight` field is `null` until the transaction is confirmed. The facilitator MAY poll for confirmation and update the response if the caller is willing to wait (via an optional `?waitForConfirmation=true` query parameter).

**Response (confirmed):**

```json
{
  "success": true,
  "transaction": {
    "txId": "<base58-tip5-hash>",
    "blockHeight": 123456,
    "status": "confirmed"
  }
}
```

**Response (failure):**

```json
{
  "success": false,
  "error": {
    "code": "<error_code>",
    "message": "<human-readable description>"
  }
}
```

## 6.4 Verification Logic

When the facilitator receives a `/verify` request, it performs the following checks in order:

### 6.4.1 Schema Validation

1. `payload.x402Version` MUST be `2`.
2. `payload.scheme` MUST equal `requirements.scheme`.
3. `payload.network` MUST equal `requirements.network`.
4. `payload.scheme` MUST be `"exact"`.
5. `payload.network` MUST be a recognized Nockchain network identifier.

### 6.4.2 Amount Validation

6. `payload.payload.authorization.value` MUST be ≥ `requirements.maxAmountRequired`.
7. `payload.payload.authorization.fee` MUST be ≥ `requirements.extra.minFee`.

### 6.4.3 Recipient Validation

8. `payload.payload.authorization.to` MUST equal `requirements.payTo`.

### 6.4.4 Time Validation

9. `payload.payload.authorization.validAfter` MUST be ≤ `now + 30s` (clock tolerance).
10. `payload.payload.authorization.validBefore` MUST be > `now - 30s` (clock tolerance).
11. `validBefore - validAfter` MUST be ≤ 3600 seconds.

### 6.4.5 Nonce Validation

12. `payload.payload.authorization.nonce` MUST NOT have been previously settled.

### 6.4.6 Note Validation

For each note in `payload.payload.authorization.notes`:

13. The note MUST exist in the current UTXO set (query via gRPC `Peek` or public API).
14. The note MUST be unspent.
15. The note's lock MUST be satisfiable by the pubkey in `payload.payload.signature.pubkey`.
16. The note's `assets` MUST match the declared `assets` in the payload.

17. The sum of all note assets MUST be ≥ `value` + `fee`.

### 6.4.7 Signature Validation

18. Reconstruct the sign message from the `authorization` object (per §5.4.1 of the Payment Payload spec).
19. Verify the Schnorr signature against the reconstructed message and the declared `pubkey`.
20. Verify that `Tip5(pubkey)` equals `authorization.from` (PKH binding).

If all checks pass, return `{ "valid": true }`.

## 6.5 Settlement Procedure

When the facilitator receives a `/settle` request:

1. **Re-verify** the payload (run all checks from §6.4). If verification fails, return an error without broadcasting.

2. **Check nonce** against the persistent nonce store. If the nonce has already been settled, return an idempotent success response with the original transaction ID.

3. **Construct the transaction:**

   a. Create a `Spend` for each input note referenced in `authorization.notes`:
      - Use the client's Schnorr signature as the witness
      - Set the fee from `authorization.fee`

   b. Create output seeds:
      - **Payment seed:** `gift = authorization.value`, `recipient = Lock(1, [payee_pubkey])` where `payee_pubkey` is resolved from `authorization.to`
      - **Change seed** (if change > 0): `gift = change`, `recipient = Lock(1, [change_pubkey])` where `change_pubkey` is resolved from `authorization.changeAddress`

   c. Compute the transaction ID and assemble the `RawTx`.

4. **Record the nonce** in persistent storage (before broadcast, to prevent double-settlement on crash recovery).

5. **Broadcast** the transaction via:
   - gRPC `Poke` to a local Nockchain node, OR
   - `WalletSendTransaction` RPC to a public API endpoint

6. **Return** the settlement response with the transaction ID.

### 6.5.1 Transaction Construction Details

The facilitator builds a v1 `RawTx` with:

```
RawTx {
  version: V1
  id: Tip5(serialized transaction data)
  spends: {
    (note_name) → Spend {
      witness: Witness {
        lock_merkle_proof: <proof from UTXO set>
        pkh_signature: PkhSignature {
          pubkey: <from payload.signature.pubkey>
          signature: <from payload.signature.schnorr>
        }
      }
      seeds: [payment_seed, change_seed?]
      fee: <from authorization.fee>
    }
  }
}
```

### 6.5.2 Pubkey Resolution

The facilitator needs the full Schnorr pubkey (Cheetah point) for constructing output locks. The pubkey is available in `payload.payload.signature.pubkey`. For the payee's pubkey (required to construct the payment output lock), the facilitator has two options:

1. **PKH-only lock:** Construct the output with a PKH-based lock (v1 style). The recipient provides their pubkey when spending.
2. **Pubkey lookup:** Query the UTXO set or a pubkey registry to resolve the PKH to a full pubkey.

Option 1 is RECOMMENDED as it requires no additional lookups.

## 6.6 Error Codes

| Code | Description |
|------|-------------|
| `invalid_scheme` | Scheme is not `"exact"` or is unsupported |
| `invalid_network` | Network identifier is not recognized |
| `invalid_version` | x402Version is not `2` |
| `invalid_amount` | Value is less than `maxAmountRequired` |
| `invalid_fee` | Fee is less than `minFee` |
| `invalid_recipient` | `to` does not match `payTo` |
| `invalid_time_valid_after` | `validAfter` is in the future |
| `invalid_time_valid_before` | `validBefore` is in the past |
| `invalid_time_window` | `validBefore - validAfter` exceeds maximum |
| `invalid_signature` | Schnorr signature verification failed |
| `invalid_pubkey_binding` | `Tip5(pubkey)` does not equal `from` |
| `nonce_already_used` | Nonce has been previously settled |
| `note_not_found` | Referenced note does not exist in UTXO set |
| `note_already_spent` | Referenced note has been spent |
| `note_lock_mismatch` | Pubkey cannot satisfy the note's lock |
| `note_assets_mismatch` | Declared note assets do not match on-chain |
| `insufficient_funds` | Total note assets < value + fee |
| `settlement_failed` | Transaction broadcast or confirmation failed |
| `facilitator_internal_error` | Unexpected internal error |

## 6.7 Idempotency

Settlement MUST be idempotent with respect to nonces:

- If `/settle` is called with a nonce that has already been settled, the facilitator MUST return the original settlement response (same `txId`), not an error.
- This allows resource servers to safely retry `/settle` calls without risk of double payment.

## 6.8 Facilitator Selection

Clients and resource servers may use different facilitators. The protocol supports:

1. **Server-specified:** The `extra.facilitatorUrl` in `PaymentRequirements` points to the resource server's preferred facilitator.
2. **Client-specified:** The client may override with its own facilitator (e.g., one it operates or trusts).
3. **Self-facilitation:** A resource server that runs its own Nockchain node can perform verification and settlement locally, without an external facilitator.

When the client uses a different facilitator than the server, the server MUST still validate the settlement on its own (by checking the transaction on-chain).

## 6.9 Monitoring and Observability

Facilitators SHOULD expose metrics for:

- Verification request count and latency
- Settlement request count, latency, and success rate
- Nonce store size
- UTXO set query latency
- Transaction broadcast success rate
- Current block height of the connected Nockchain node
