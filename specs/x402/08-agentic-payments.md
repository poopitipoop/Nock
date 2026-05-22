# 8. Agentic Payments

This document describes how autonomous AI agents interact with the x402 payment protocol on Nockchain — from wallet management to payment decision-making to multi-agent commerce.

## 8.1 Overview

Agentic payments are the primary motivation for Nockchain's x402 integration. AI agents need to:

1. **Discover** paid resources on the web
2. **Evaluate** whether a resource is worth its price
3. **Authorize** payments without human intervention
4. **Execute** the HTTP payment flow end-to-end
5. **Track** spending against budgets and policies

The x402 protocol makes all of this possible over standard HTTP, with no custom payment APIs, no session tokens, and no OAuth flows.

## 8.2 Agent Architecture

```
┌─────────────────────────────────────────────────────┐
│                    AI Agent                           │
│                                                       │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────┐ │
│  │   Reasoning  │  │   Payment    │  │   Budget    │ │
│  │   Engine     │──│   Client     │──│   Policy    │ │
│  │   (LLM)      │  │   (x402)     │  │   Engine    │ │
│  └─────────────┘  └──────┬───────┘  └─────────────┘ │
│                           │                           │
│                    ┌──────┴───────┐                   │
│                    │   Nockchain  │                   │
│                    │   Wallet     │                   │
│                    │   (keys +    │                   │
│                    │    UTXOs)    │                   │
│                    └──────────────┘                   │
└─────────────────────────────────────────────────────┘
```

### 8.2.1 Components

| Component | Responsibility |
|-----------|---------------|
| **Reasoning Engine** | Decides when and what resources to access. May be an LLM, a rule-based system, or a hybrid. |
| **Payment Client** | Implements the x402 client protocol: parses 402 responses, constructs payloads, signs authorizations, retries requests. |
| **Budget Policy Engine** | Enforces spending limits, per-resource caps, rate limits, and approval policies. |
| **Nockchain Wallet** | Manages private keys, tracks UTXOs, signs authorizations. |

## 8.3 Agent Wallet Management

### 8.3.1 Dedicated Agent Keys

Agents SHOULD use **dedicated child keys** derived from a master key, not the master key itself:

```
Master Key (human-controlled)
  └── Child Key index=0 (agent: weather-bot)
  └── Child Key index=1 (agent: research-assistant)
  └── Child Key index=2 (agent: trading-bot)
```

Benefits:
- Key compromise of one agent does not affect others
- Per-agent spending can be tracked by address
- Keys can be rotated without affecting the master wallet

### 8.3.2 Key Derivation for Agents

```bash
# Human derives a key for the agent
nockchain-wallet derive-child 42 --hardened --label "weather-agent"
```

The agent receives:
- A child private key (for signing payment authorizations)
- The corresponding PKH (for receiving change)

### 8.3.3 UTXO Management

Agents must manage their UTXO set carefully:

1. **Sync regularly.** Query the Nockchain API to discover new notes (e.g., funding from the master wallet).
2. **Track pending notes.** Notes referenced in unconfirmed x402 payments must not be reused.
3. **Consolidate proactively.** If the agent accumulates many small change outputs, consolidate them into fewer larger notes during low-activity periods.
4. **Reserve for fees.** Always maintain at least one note large enough to cover transaction fees.

### 8.3.4 Funding Agents

The human (or a funding agent) sends NOCK to the agent's PKH:

```bash
nockchain-wallet create-tx \
  --names "[first1 last1]" \
  --recipient '{"kind":"p2pkh","address":"<agent-pkh>","amount":6553600}' \
  --fee 10
```

This creates a note spendable only by the agent's private key.

## 8.4 Payment Decision Flow

When an agent encounters a 402 response:

```
Agent receives HTTP 402
       │
       ▼
Parse PAYMENT-REQUIRED header
       │
       ▼
Select PaymentRequirements
matching agent's capabilities
(scheme=exact, network=nockchain:mainnet)
       │
       ▼
┌──────────────────┐
│ Budget Policy     │──► DENY → Return error / skip resource
│ Check             │
└──────┬───────────┘
       │ ALLOW
       ▼
Query wallet for
available notes
       │
       ▼
┌──────────────────┐
│ Sufficient funds? │──► NO → Return insufficient_funds error
└──────┬───────────┘
       │ YES
       ▼
Select notes,
construct + sign PaymentPayload
       │
       ▼
Retry HTTP request with
PAYMENT-SIGNATURE header
       │
       ▼
Process response
```

### 8.4.1 Budget Policy Checks

Before authorizing any payment, the agent MUST consult its budget policy. Policies are defined as:

```json
{
  "maxPerPayment": "655360",
  "maxPerHour": "6553600",
  "maxPerDay": "65536000",
  "maxTotal": "655360000",
  "allowedDomains": ["api.example.com", "data.example.org"],
  "blockedDomains": [],
  "requireHumanApproval": false,
  "humanApprovalThreshold": "6553600"
}
```

| Policy | Description |
|--------|-------------|
| `maxPerPayment` | Maximum nicks for a single x402 payment |
| `maxPerHour` | Rolling hourly spending cap |
| `maxPerDay` | Rolling daily spending cap |
| `maxTotal` | Lifetime spending cap (until manually reset) |
| `allowedDomains` | Whitelist of resource server domains |
| `blockedDomains` | Blacklist of domains (overrides allowedDomains) |
| `requireHumanApproval` | If true, all payments require human confirmation |
| `humanApprovalThreshold` | Payments above this amount require human confirmation |

### 8.4.2 Cost-Benefit Evaluation

Sophisticated agents may perform cost-benefit analysis before paying:

1. **Is the resource necessary?** Does the agent's current task require this data?
2. **Is the price fair?** Compare to cached prices, alternative sources, or a price oracle.
3. **Is the source trustworthy?** Check domain reputation, TLS certificate, past interaction history.
4. **Is the expected value sufficient?** Does the `outputSchema` describe data the agent can use?

## 8.5 Agent-to-Agent Payments

x402 naturally supports agent-to-agent commerce, where one AI agent pays another for services:

```
┌──────────────┐         x402          ┌──────────────┐
│  Agent A     │◄─────────────────────►│  Agent B     │
│  (Consumer)  │    HTTP + Payment     │  (Provider)  │
│              │                        │  (NockApp)   │
└──────────────┘                        └──────────────┘
```

### 8.5.1 Provider Agent as NockApp

An agent running as a NockApp can serve HTTP endpoints gated by x402:

```
NockApp {
  http_driver: {
    routes: [
      GET /api/analyze → x402_gated(price=65536, handler=analyze)
      GET /api/summarize → x402_gated(price=32768, handler=summarize)
    ]
  }
  kernel: analysis_kernel.jam
}
```

The NockApp HTTP driver handles the 402 response automatically. The kernel processes requests only after payment verification.

### 8.5.2 Service Discovery

Agents discover paid services through:

1. **Direct URL.** The agent is configured with known service endpoints.
2. **Bazaar (future).** The x402 Bazaar extension provides a decentralized service registry where agents can discover and compare paid resources.
3. **Web search.** Agents search the web for APIs, encounter 402 responses, and negotiate payment.
4. **Agent-to-agent referral.** One agent recommends a paid service to another.

## 8.6 Multi-Step Agent Workflows

Complex agent tasks may require multiple paid resources:

```
Task: "Analyze weather impact on crop yields"

Step 1: GET /weather-data (402 → pay 1 NOCK)
Step 2: GET /crop-database  (402 → pay 2 NOCK)
Step 3: GET /analysis-model  (402 → pay 5 NOCK)
Step 4: Combine results, produce report
```

### 8.6.1 Budget Allocation

For multi-step workflows, the agent pre-allocates budget:

```json
{
  "workflow": "crop-analysis",
  "totalBudget": "524288",
  "steps": [
    { "resource": "/weather-data", "maxCost": "65536" },
    { "resource": "/crop-database", "maxCost": "131072" },
    { "resource": "/analysis-model", "maxCost": "327680" }
  ]
}
```

If any step exceeds its allocated budget, the agent can:
- Abort the workflow
- Reallocate from unused step budgets
- Request human approval for the overage

### 8.6.2 Failure Handling

If a payment succeeds but the resource returns unusable data, the agent:
- Logs the failed interaction
- Adjusts trust score for the resource server
- Does NOT retry payment to the same endpoint without policy approval
- May try an alternative service

## 8.7 Agent Payment Client SDK

A reference agent payment client provides:

```rust
/// Agent-facing x402 payment client
pub struct AgentPaymentClient {
    wallet: NockchainWallet,
    policy: BudgetPolicy,
    facilitator_url: String,
    spending_log: SpendingLog,
}

impl AgentPaymentClient {
    /// Make an HTTP request, automatically handling x402 payments
    pub async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, PaymentError> {
        let response = http_client.send(request.clone()).await?;

        if response.status() != 402 {
            return Ok(response);
        }

        let requirements = parse_payment_required(&response)?;
        let selected = self.select_requirement(&requirements)?;

        self.policy.check(&selected)?;

        let payload = self.wallet.create_payment_payload(&selected)?;
        let paid_request = request.with_header("PAYMENT-SIGNATURE", encode(&payload));

        http_client.send(paid_request).await
    }
}
```

### 8.7.1 Automatic Retry

The client transparently handles the 402→pay→retry flow. From the agent's reasoning engine perspective, `fetch()` either returns the resource or an error — the payment is invisible.

### 8.7.2 Spending Log

Every payment is logged for auditability:

```json
{
  "timestamp": "2026-02-17T12:00:00Z",
  "resource": "https://api.example.com/weather",
  "amount": "65536",
  "fee": "10",
  "txId": "<base58-tip5>",
  "status": "settled",
  "agentId": "weather-bot",
  "workflowId": "crop-analysis-001"
}
```

## 8.8 MCP Server Integration

Agents using the Model Context Protocol (MCP) can access x402-gated resources through an MCP server that wraps the payment client:

```
┌──────────────┐       MCP         ┌──────────────┐       x402        ┌──────────────┐
│  LLM Agent   │◄────────────────►│  MCP Server  │◄──────────────────►│  Resource    │
│              │                    │  (x402-aware)│                    │  Server      │
└──────────────┘                    └──────────────┘                    └──────────────┘
```

The MCP server:
1. Exposes tools like `fetch_paid_resource(url, max_cost)`
2. Handles the 402 flow transparently
3. Enforces budget policies on behalf of the LLM agent
4. Reports costs back to the agent for decision-making

## 8.9 Security Considerations for Agents

### 8.9.1 Key Isolation

Agent private keys MUST be stored securely:
- In a hardware security module (HSM) for high-value agents
- In an encrypted keystore with per-agent access controls
- Never in environment variables or config files in plaintext

### 8.9.2 Prompt Injection Defense

Malicious resource servers could return content designed to manipulate the agent into making additional payments. Agents MUST:
- Separate payment decisions from content processing
- Never allow resource content to trigger payment authorization
- Apply budget policies regardless of content instructions

### 8.9.3 Price Manipulation

A resource server could dynamically inflate prices for agent clients. Agents SHOULD:
- Cache historical prices for comparison
- Set strict `maxPerPayment` limits
- Prefer resources with published, stable pricing

### 8.9.4 Denial of Service via 402

A compromised or malicious service could return 402 for every request, draining agent funds. Mitigations:
- Per-domain rate limits in the budget policy
- Circuit breakers: stop paying a domain after N consecutive failures
- Reputation tracking across interactions
