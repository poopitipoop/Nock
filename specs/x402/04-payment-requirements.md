# 4. Payment Requirements

This document defines the `PaymentRequirements` schema for the `(exact, nockchain)` scheme-network pair.

## 4.1 PaymentRequired Envelope

When a resource server responds with HTTP 402, the `PAYMENT-REQUIRED` header contains a base64-encoded JSON array of `PaymentRequirements` objects:

```
PAYMENT-REQUIRED: <base64(JSON([PaymentRequirements, ...]))>
```

Each entry represents one acceptable payment option. The client selects the first entry it can satisfy.

## 4.2 PaymentRequirements Schema

```json
{
  "scheme": "exact",
  "network": "nockchain:mainnet",
  "maxAmountRequired": "<nicks as string>",
  "resource": "<URL of the resource being purchased>",
  "description": "<human-readable description>",
  "mimeType": "<MIME type of the gated resource>",
  "outputSchema": null,
  "payTo": "<base58-encoded PKH of the resource server>",
  "maxTimeoutSeconds": 300,
  "asset": "nock",
  "extra": {
    "version": "1",
    "minFee": "<minimum tx fee in nicks as string>",
    "facilitatorUrl": "<URL of the facilitator>"
  }
}
```

### 4.2.1 Field Definitions

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `scheme` | string | Yes | Payment scheme. MUST be `"exact"` for this binding. |
| `network` | string | Yes | Network identifier. One of `"nockchain:mainnet"` or `"nockchain:fakenet"`. |
| `maxAmountRequired` | string | Yes | Maximum payment amount in nicks. String-encoded to avoid integer overflow in JSON. |
| `resource` | string | Yes | Canonical URL of the resource being purchased. Used for replay protection (binds payment to resource). |
| `description` | string | Yes | Human-readable description of what is being purchased. Shown to users in wallet UIs. |
| `mimeType` | string | Yes | MIME type of the response the client will receive upon successful payment. |
| `outputSchema` | object \| null | No | JSON Schema describing the structure of the response body. Useful for agent clients that need to validate response format. |
| `payTo` | string | Yes | Base58-encoded PKH (v1) or Schnorr pubkey (v0) of the payment recipient. |
| `maxTimeoutSeconds` | number | Yes | Maximum time (in seconds) the client has to submit a payment after receiving the 402. After this window, the `PaymentRequirements` are considered expired. |
| `asset` | string | Yes | Asset identifier. `"nock"` for native NOCK. |
| `extra` | object | Yes | Nockchain-specific extensions (see §4.3). |

### 4.2.2 Amount Encoding

The `maxAmountRequired` field is a **string representation of an unsigned integer** denominating nicks. This avoids JSON number precision issues.

Examples:
- `"65536"` = 1 NOCK
- `"1"` = 1 nick (minimum possible payment)
- `"6553600000"` = 100,000 NOCK

## 4.3 Nockchain Extra Fields

The `extra` object carries Nockchain-specific metadata:

```json
{
  "version": "1",
  "minFee": "10",
  "facilitatorUrl": "https://facilitator.example.com"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `version` | string | Yes | Address version. `"0"` for Schnorr pubkey addresses, `"1"` for PKH addresses. Determines how `payTo` is interpreted. |
| `minFee` | string | Yes | Minimum transaction fee the facilitator will accept (in nicks). The client MUST include at least this fee. |
| `facilitatorUrl` | string | No | URL of the facilitator the resource server uses. If absent, the client should use a default facilitator or its own. |

## 4.4 Asset Identifiers

| Asset ID | Description |
|----------|-------------|
| `"nock"` | Native NOCK (denominated in nicks) |

Future extensions may add support for assets held in Nockchain note data (e.g., application-specific tokens), in which case the asset identifier would reference a lock root or contract identifier.

## 4.5 Example: Minimal 402 Response

HTTP response:

```http
HTTP/1.1 402 Payment Required
PAYMENT-REQUIRED: eyJzY2hlbWUiOiJleGFjdCIsIm5ldHdvcmsiOiJub2NrY2hhaW46bWFpbm5ldCIsIm1heEFtb3VudFJlcXVpcmVkIjoiNjU1MzYiLCJyZXNvdXJjZSI6Imh0dHBzOi8vYXBpLmV4YW1wbGUuY29tL3dlYXRoZXIiLCJkZXNjcmlwdGlvbiI6IkN1cnJlbnQgd2VhdGhlciBkYXRhIiwibWltZVR5cGUiOiJhcHBsaWNhdGlvbi9qc29uIiwib3V0cHV0U2NoZW1hIjpudWxsLCJwYXlUbyI6IjxiYXNlNTgtcGtoPiIsIm1heFRpbWVvdXRTZWNvbmRzIjozMDAsImFzc2V0Ijoibm9jayIsImV4dHJhIjp7InZlcnNpb24iOiIxIiwibWluRmVlIjoiMTAiLCJmYWNpbGl0YXRvclVybCI6Imh0dHBzOi8vZmFjaWxpdGF0b3IuZXhhbXBsZS5jb20ifX0=
Content-Type: text/plain

Payment required to access this resource.
```

Decoded `PAYMENT-REQUIRED` payload:

```json
[
  {
    "scheme": "exact",
    "network": "nockchain:mainnet",
    "maxAmountRequired": "65536",
    "resource": "https://api.example.com/weather",
    "description": "Current weather data",
    "mimeType": "application/json",
    "outputSchema": null,
    "payTo": "<base58-pkh>",
    "maxTimeoutSeconds": 300,
    "asset": "nock",
    "extra": {
      "version": "1",
      "minFee": "10",
      "facilitatorUrl": "https://facilitator.example.com"
    }
  }
]
```

## 4.6 Multiple Payment Options

A resource server MAY offer multiple payment options. The client selects the first it can satisfy, scanning the array in order:

```json
[
  {
    "scheme": "exact",
    "network": "nockchain:mainnet",
    "maxAmountRequired": "65536",
    "asset": "nock",
    "payTo": "<base58-pkh>",
    ...
  },
  {
    "scheme": "exact",
    "network": "eip155:8453",
    "maxAmountRequired": "1000000",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "payTo": "0x1234...abcd",
    ...
  }
]
```

Resource servers SHOULD order entries by preference (most preferred first).

## 4.7 Expiration Semantics

The `maxTimeoutSeconds` field defines the validity window. The clock starts when the 402 response is sent. If the client submits a `PaymentPayload` after this window, the resource server (or facilitator) SHOULD reject it.

Implementations MAY add clock-skew tolerance of up to 30 seconds.

For long-lived resources (e.g., subscriptions), the resource server issues fresh `PaymentRequirements` on each 402 response. Stale requirements are not reusable.
