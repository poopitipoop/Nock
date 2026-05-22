# 7. Scheme: `exact` on `nockchain`

This document is the normative specification of the `exact` payment scheme applied to Nockchain networks. It brings together the protocol, primitives, requirements, payload, and facilitator specs into a single coherent scheme definition.

## 7.1 Scheme Identity

| Property | Value |
|----------|-------|
| Scheme name | `exact` |
| Network namespace | `nockchain` |
| Network references | `mainnet`, `fakenet` |
| Full identifiers | `nockchain:mainnet`, `nockchain:fakenet` |
| Asset | `nock` (native NOCK, denominated in nicks) |

## 7.2 Scheme Semantics

The `exact` scheme transfers a **specific, predetermined amount** from the client to the resource server. The amount is fixed at request time and does not vary based on resource consumption.

Use cases:
- Pay 1 NOCK to access a premium API endpoint
- Pay 0.01 NOCK for a single weather query
- Pay 10 NOCK to download a dataset

## 7.3 Complete Flow (Normative)

### Step 1: Resource Server Configuration

The resource server operator configures payment requirements per route:

```
Route: GET /api/weather
Price: 65,536 nicks (1 NOCK)
PayTo: <operator's PKH>
Min Fee: 10 nicks
Timeout: 300 seconds
```

### Step 2: Client Receives 402

```http
HTTP/1.1 402 Payment Required
PAYMENT-REQUIRED: <base64-encoded PaymentRequired array>
Content-Type: text/plain

Payment required.
```

The `PaymentRequired` array contains at minimum:

```json
[
  {
    "scheme": "exact",
    "network": "nockchain:mainnet",
    "maxAmountRequired": "65536",
    "resource": "https://api.example.com/weather",
    "description": "Current weather data for any location",
    "mimeType": "application/json",
    "outputSchema": {
      "type": "object",
      "properties": {
        "temperature": { "type": "number" },
        "humidity": { "type": "number" },
        "conditions": { "type": "string" }
      }
    },
    "payTo": "4vJ9JU1bJJE96FU...",
    "maxTimeoutSeconds": 300,
    "asset": "nock",
    "extra": {
      "version": "1",
      "minFee": "10",
      "facilitatorUrl": "https://facilitator.nockchain.example.com"
    }
  }
]
```

### Step 3: Client Constructs Payment

The client:

1. Selects the `(exact, nockchain:mainnet)` requirement
2. Queries its local wallet for unspent notes owned by its PKH
3. Selects one or more notes totaling ≥ `maxAmountRequired` + `minFee`
4. Generates a unique nonce
5. Sets `validAfter` = `now` and `validBefore` = `now + maxTimeoutSeconds`
6. Constructs the authorization message and signs it with its Schnorr private key
7. Assembles the `PaymentPayload`

### Step 4: Client Retries with Payment

```http
GET /api/weather HTTP/1.1
Host: api.example.com
PAYMENT-SIGNATURE: <base64-encoded PaymentPayload>
```

### Step 5: Verification

The resource server sends the payload and requirements to the facilitator:

```http
POST /verify HTTP/1.1
Host: facilitator.nockchain.example.com
Content-Type: application/json

{
  "payload": { ... },
  "requirements": { ... }
}
```

The facilitator performs all checks defined in [§6.4 of the Facilitator spec](./06-facilitator.md#64-verification-logic).

### Step 6: Resource Delivery

If verification succeeds, the resource server delivers the content:

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "temperature": 22.5,
  "humidity": 65,
  "conditions": "Partly cloudy"
}
```

### Step 7: Settlement

The resource server requests settlement:

```http
POST /settle HTTP/1.1
Host: facilitator.nockchain.example.com
Content-Type: application/json

{
  "payload": { ... },
  "requirements": { ... }
}
```

The facilitator constructs and broadcasts the Nockchain transaction.

### Step 8: Confirmation

The resource server receives the settlement receipt and MAY include it in the response:

```
PAYMENT-RESPONSE: <base64-encoded settlement receipt>
```

## 7.4 Transaction Structure

The facilitator constructs the following Nockchain transaction:

```
RawTx(v1) {
  spends: {
    note_name_1 → WitnessSpend {
      witness: {
        pkh_signature: (client_pubkey, client_schnorr_sig)
      }
      seeds: [
        Seed {
          recipient: Lock(1, [payee_pubkey_from_pkh])
          gift: 65536 nicks
        },
        Seed {
          recipient: Lock(1, [client_pubkey])
          gift: <change> nicks
          // Only present if change > 0
        }
      ]
      fee: 10 nicks
    }
  }
}
```

### 7.4.1 Output Ordering

1. **Payment output** (index 0): Pays the resource server
2. **Change output** (index 1, optional): Returns excess to the client

### 7.4.2 Fee Attribution

The fee is deducted from the client's input notes. The client explicitly authorizes the fee in the `PaymentPayload`.

```
sum(inputs) = value + fee + change
```

## 7.5 Edge Cases

### 7.5.1 Exact-Amount Notes

If the client has a note with exactly `value + fee` nicks, no change output is needed. This produces a smaller transaction and is preferred.

### 7.5.2 Multiple Input Notes

If no single note covers `value + fee`, the client may reference multiple notes. The facilitator MUST construct a spend for each input note, but all spends share the same output seeds.

### 7.5.3 Note Consumed Between Verify and Settle

If a note is spent (by another transaction) between `/verify` and `/settle`, settlement will fail. The facilitator returns `note_already_spent`. The resource server has already delivered the content in the optimistic model — this is the primary risk of optimistic settlement.

Mitigations:
- Short `maxTimeoutSeconds` reduces the window
- Clients SHOULD NOT reuse notes across concurrent payments
- Facilitators MAY implement a note-locking mechanism during the verify-settle window

### 7.5.4 Insufficient Fee

If the client provides a fee below the network's current minimum relay fee (which may differ from `extra.minFee`), the transaction may not propagate. Facilitators SHOULD set `minFee` conservatively.

## 7.6 Pricing Patterns

### 7.6.1 Fixed Price

Every request costs the same amount. Simplest model.

```json
{ "maxAmountRequired": "65536" }
```

### 7.6.2 Dynamic Pricing

The resource server computes the price based on the request parameters and returns it in the 402 response. For example, a data API might charge based on the date range requested.

```json
{ "maxAmountRequired": "655360", "description": "10 years of historical data" }
```

### 7.6.3 Tiered Access

Different routes or quality levels have different prices:

```json
// Standard quality
{ "maxAmountRequired": "65536", "resource": "/api/image?quality=standard" }

// High quality
{ "maxAmountRequired": "327680", "resource": "/api/image?quality=high" }
```

## 7.7 Implementation Checklist

### Resource Server

- [ ] Return 402 with valid `PAYMENT-REQUIRED` header for gated routes
- [ ] Forward `PAYMENT-SIGNATURE` to facilitator `/verify`
- [ ] Deliver content on successful verification
- [ ] Call facilitator `/settle` after content delivery (optimistic) or before (confirmed)
- [ ] Handle settlement failures gracefully

### Client

- [ ] Parse `PAYMENT-REQUIRED` header from 402 responses
- [ ] Select appropriate `PaymentRequirements` entry
- [ ] Query wallet for available notes
- [ ] Construct and sign `PaymentPayload`
- [ ] Retry request with `PAYMENT-SIGNATURE` header
- [ ] Mark notes as pending to prevent concurrent use

### Facilitator

- [ ] Expose `/verify` and `/settle` HTTP endpoints
- [ ] Connect to Nockchain node via gRPC
- [ ] Implement full verification logic (§6.4)
- [ ] Construct valid Nockchain transactions
- [ ] Maintain persistent nonce store
- [ ] Ensure settlement idempotency
- [ ] Broadcast transactions to P2P network
