# Handoff brief — Pearl / Oyster security review

**Read this first if you are an AI assistant asked to continue this review.**

You are picking up an independent source-security review of
`github.com/pearl-research-labs/pearl` (Pearl L1 blockchain; `Oyster` is its
HD wallet daemon). This document tells you what was already done, exactly how,
what the limitations were, and what to do next. Everything here was produced by
source reading and static analysis only — **no dynamic testing, no exploitation,
nothing was run against a live network or a third party's systems.**

---

## 1. Origin of the task

The user asked whether the Oyster wallet was affected by the
[Coldcard firmware vulnerability](https://thehackernews.com/2026/08/coldcard-hardware-wallet-flaw-linked-to.html)
disclosed 30–31 July 2026, in which seed generation silently fell back from the
hardware TRNG to a predictable software RNG, letting attackers reconstruct seeds
and sweep ~1,083 BTC (~$70M+).

**Answer: Oyster never had that bug** (proof in §4). The review then widened at
the user's request across the whole repository.

The user is a **prospective/current wallet user**, not a Pearl maintainer.
Prioritise findings by risk to someone holding funds, not by abstract severity.

---

## 2. Environment setup — reproduce this first

```bash
# The subject repo (full history needed for the git-archaeology in §4)
git clone https://github.com/pearl-research-labs/pearl.git pearl

# Upstream trees used for divergence analysis (§6)
git clone --depth 1 https://github.com/btcsuite/btcd.git btcd
git clone --depth 1 https://github.com/0xPolygonZero/plonky2.git p2up
```

Repo scale (non-test, non-generated LOC):

| Dir | LOC | Dir | LOC |
|---|---|---|---|
| `node` | 106,144 | `spv` | 17,937 |
| `plonky2` | 48,204 | `apps` | 17,259 |
| `wallet` (Oyster) | 41,906 | `dnsseeder` | 2,003 |
| `zk-pow` | 21,339 | `pearl-blake3` | 1,269 |
| `miner` | 19,990 | others | < 1,100 each |

**Total ≈ 278k LOC. Roughly 75k was examined with real depth.**

### Build notes (relevant if you attempt dynamic verification)

- The tree needs `-tags xmss,zkpow` to be functional. `Taskfile.yml` does this:
  `go build -tags xmss,zkpow -o bin/oyster -v ./wallet`. CI uses
  `task build:blockchain`.
- Without those tags the stubs compile and **fail closed** (`zkpow` returns an
  error, `xmss.Verify` returns `false`) — verified, see §5.
- Building requires cgo (`xmss/libxmss.a`) and a Rust toolchain
  (`zk-pow/bindings/go/target/release/libzk_pow_ffi.a`). **This was never built
  during the review** — that is why OYS-11 is analysis-only (§7).

---

## 3. Deliverables already produced

| File | Contents |
|---|---|
| `01-security-review.md` | The full review — 23 findings, ~30 verified-clean areas, scope and limitations |
| `02-advisories-to-file.md` | 4 paste-ready GitHub private advisory bodies + form fields, and 1 public hardening issue |
| `03-HANDOFF-for-claude.md` | This file |

All are also committed on branch `claude/oyster-wallet-security-z8aotv` of the
user's own repo (`poopitipoop/Nock`), under `docs/`.

**Nothing has been reported to Pearl Research Labs.** The user was given the
advisory text and the submission URL
(`https://github.com/pearl-research-labs/pearl/security/advisories/new`) to file
under their own identity. Do not file on their behalf without explicit
instruction — and note that no tool in the original session could do so anyway
(the GitHub MCP server has no security-advisory endpoint, and repo scope was
limited to the user's own repo).

---

## 4. The Coldcard question — how it was settled (do not redo)

This is closed. The technique is recorded in case you need the same pattern
elsewhere.

```bash
cd pearl

# Every commit that ever touched the seed-generation file (follows renames)
git log --oneline --all --follow -- '*hdkeychain/extendedkey.go'   # → exactly 5

# Hash the GenerateSeed body at each of those commits — all 5 identical
for c in $(git log --format=%H --all --follow -- '*hdkeychain/extendedkey.go'); do
  git show $c:node/btcutil/hdkeychain/extendedkey.go \
    | awk '/^func GenerateSeed/,/^}/' | sha256sum | cut -c1-12
done

# math/rand never imported in that file, at any commit  → all zero
for c in $(git log --format=%H --all --follow -- '*hdkeychain/extendedkey.go'); do
  git show $c:node/btcutil/hdkeychain/extendedkey.go | grep -c '"math/rand"'
done

# math/rand never entered ANY key-derivation path, whole history
git log -S'math/rand' --oneline --all -- wallet/ xmss/ \
    node/btcutil/hdkeychain/ node/btcec/
# → only chain-polling jitter, coin selection, change-index randomisation

# All 8 release tags carry a byte-identical GenerateSeed + crypto/rand
git tag   # pearl-wallet-v1.0.0..v2.0.4, v1.1.5..v1.2.1
```

Also confirmed: signing nonces are **RFC6979 deterministic**
(`node/btcec/schnorr/signature.go:521`), so the other classic key-recovery
class is absent too.

---

## 5. Coverage map

### Reviewed with real depth

`wallet/` (Oyster: key management, RPC, tx construction, BIP-322, snacl,
waddrmgr address derivation) · `node/btcutil/hdkeychain` · `node/btcec` ·
`node/txscript` (XMSS opcode + btcd divergence) · `node/blockchain`
(validate.go, difficulty.go) · `node/mempool` (policy limits) · `node/addrmgr`
(eclipse resistance) · `node/wire` (bounds) · `node/v2transport` (BIP324) ·
`node/zkpow` + `xmss/` build stubs · `spv/` (header/block/filter validation) ·
`zk-pow/src/api` + parameter validation · `plonky2/` (divergence only) ·
`apps/apps/pearl-desktop-wallet` (main process) ·
`apps/packages/pearl-address-validation` · `miner/pearl-gateway` ·
`.github/workflows/` · `install.sh` · `go.mod`

### NOT reviewed — candidates for you

| Area | LOC | Why it matters |
|---|---|---|
| **`zk-pow/src/circuit/` AIR constraints** | ~15k | **Highest value, lowest tractability.** Do the constraints actually enforce the claimed matmul? Not answerable by reading — needs formal analysis |
| `plonky2/` fork delta, line by line | ~48k | ZKP-2 flags it; only spot-checked |
| `node/rpcclient`, `node/btcjson` | ~18k | Client-side parsing |
| `node/database` | 6.9k | Corruption/consistency |
| `node/peer`, `netsync`, `mining` | ~6k | Partially covered via limits only |
| `miner/vllm-miner`, `pearl-gemm` | ~10k | Not user-facing |
| `node/btcutil` address/amount encoding | 8.6k | Was Tier-1 in the plan; not reached |
| `apps` renderer beyond the send path | ~11k | Main process covered |
| `dnsseeder` HTTP handlers | 2k | Public infra; binds `0.0.0.0` by design |

---

## 6. Techniques that worked — reuse these

**Divergence analysis beats linear reading.** `node/` derives from btcd,
`wallet/` from btcwallet, `plonky2/` is a fork of Polygon Zero's. Inherited code
is battle-tested; **the modifications are where novel bugs live.**

```bash
# Normalise import paths + strip copyright, then diff
norm() { sed -e 's#github.com/btcsuite/btcd#PKG#g;
                 s#github.com/pearl-research-labs/pearl/node#PKG#g' \
             -e '/^\/\/ Copyright/d' "$1"; }

# Extract semantic deltas rather than reading raw diffs — e.g. which script
# verification flags exist in each tree:
grep -oE 'ScriptVerify[A-Za-z]+' btcd/txscript/engine.go   | sort -u > /tmp/a
grep -oE 'ScriptVerify[A-Za-z]+' pearl/node/txscript/engine.go | sort -u > /tmp/b
comm -23 /tmp/a /tmp/b   # removed in Pearl
```

**A triage idea that FAILED — don't repeat it.** Using copyright headers to
separate new from derived code does not work here: 460 of 532 `node/*.go` files
carry Pearl headers and **zero** retain btcsuite notices (see §8).

**Check stubs and build tags before trusting any verifier.** A `_stub.go` that
returns "valid" would be catastrophic. Both fail closed here — verify this still
holds if the build system changes.

---

## 7. Findings — 23 total

Full detail in `01-security-review.md`. Summary, worst first:

| ID | Component | Sev | Finding |
|---|---|---|---|
| **PDW-1** | desktop wallet | **High** | Plaintext seed + passphrase written `0644` in a `0755` dir during wallet **import** |
| **OYS-11** | Oyster | **High**\* | Recovery lookahead derives addresses with a different convention than PQ address creation → funds not rediscovered on seed-only restore |
| OYS-12 | Oyster | Medium | XMSS commitment silently omitted when wallet locked/watch-only |
| OYS-1 | Oyster | Medium | `--createfromfile` does not check file permissions |
| OYS-2 | Oyster | Medium | Seed printed to stdout → captured by journald/Docker/K8s |
| OYS-3 | Oyster | Medium | Experimental gRPC `WalletLoaderService` has TLS but no client auth |
| GW-1 | miner gateway | Medium | pearld RPC password logged at INFO |
| PKG-1 | apps/packages | Medium\*\* | `pearl-address-validation` accepts **Bitcoin** base58 addresses as valid Pearl addresses |
| PDW-2 | desktop wallet | Medium | RPC credentials on argv (visible via `ps`) |
| PDW-3 | desktop wallet | Medium | Releases unsigned / un-notarized |
| DEP-1 | go.mod | Low\*\*\* | `grpc v1.82.0` affected by GHSA-hrxh-6v49-42gf |
| RS-1 | zk-pow | Low | Unsound `unsafe` aliasing in BLAKE3 trace gen (prover-side only; **two copies**) |
| OYS-4/5/6, GW-2, PDW-4 | — | Low | scrypt `N=2^14`; `InsecurePubPassphrase`; `walletpassphrase timeout=0`; weak gateway defaults; Electron `sandbox:false` |
| ZKP-1/2, OYS-7/8/9/10, GW-3 | — | Info | See report |

\* OYS-11 affects only users who opted into PQ addresses (`pq=true`, not
default; the desktop wallet never sets it).
\*\* PKG-1 has no in-repo consumer — it is a distributable library.
\*\*\* DEP-1 is CVSS 8.8 upstream but only the DoS sub-issue applies, against a
server that is off by default.

### OYS-11 needs empirical confirmation — this is the top follow-up

It was derived from control flow, **not reproduced**, because the tree was never
built. The BIP-341 arithmetic is certain (a tapscript root changes the output
key, so it changes the address). What needs confirming is the code path:

1. `getnewaddress "" "bech32" true` → PQ address
2. Fund it, let it confirm
3. Delete the wallet DB
4. Recover from the same mnemonic, rescan
5. **Predicted:** funds not rediscovered, because `expandScopeHorizons`
   (`wallet/wallet/wallet.go:960,990`) passes `includePQTapscript=false` while
   `recovery.go:87,107` passes `true` — and on a fresh restore
   `ExternalKeyCount` is 0, so only the former path runs.

---

## 8. Non-security observation worth carrying forward

`node/` and `wallet/` derive from btcd/btcwallet/neutrino (ISC, Conformal /
btcsuite). In the current tree 460 of 532 `node/*.go` files carry a Pearl
copyright header, **zero** retain a btcsuite notice, and the root `LICENSE`
credits Pearl Research Labs and The Decred developers but not btcsuite. ISC
requires the notice appear in all copies.

The README does credit the projects, and Decred's notice *was* preserved, so
this reads as an oversight rather than intent. Not legal advice; not filed as a
security advisory.

---

## 9. Limitations — be honest about these, don't quietly inherit them

1. **No dependency reachability scan.** `govulncheck` was rebuilt against Go
   1.26.5 (see gotcha below) but the session's **organisation egress policy
   blocked `vuln.go.dev` and `api.osv.dev` (403 on CONNECT)**. The proxy
   documentation explicitly says not to route around such denials, so it was
   not attempted. DEP-1 came from *manual advisory version-matching via web
   search* — that is **not** call-graph reachability analysis. It can miss
   advisories and cannot say whether a vulnerable path is reachable.
   **If you have network access, run `govulncheck ./...` — this is the single
   cheapest unclosed gap.**
2. **Nothing was built or executed.** Requires cgo + Rust static libs.
3. **No dynamic testing, fuzzing, or exploitation** of any kind.
4. **ZK circuit soundness was not assessed** and cannot be by source reading.
   The two Least Authority reports in `plonky2/audits/` cover **upstream's
   unmodified library**, not Pearl's fork and not Pearl's circuits — while the
   fork modifies `fri/verifier.rs`, `plonk/verifier.rs`, `get_challenges.rs`,
   `validate_shape.rs` and `starky/config.rs`, and adds an `oracles_to_skip`
   parameter that **skips FRI Merkle verification** for oracles the verifier
   claims to recompute. No misuse was found and the spot checks were positive,
   but this needs a specialist audit, not another LLM pass.
5. **Released binaries were never compared to source.** No reproducible-build
   or provenance attestation exists.
6. **Source review ≠ audit.** Absence of findings in an area is not proof of
   absence of bugs.

### Gotchas that cost time

- `govulncheck` installed via `go install …@latest` builds with **Go 1.25** and
  then refuses to type-check this Go 1.26 module. Fix:
  `GOTOOLCHAIN=go1.26.5 go install golang.org/x/vuln/cmd/govulncheck@latest`.
- `govulncheck -scan module` rejects package patterns; call it with no args.
- Pearl's `_stub.go` files are gated by `//go:build !xmss` / `!zkpow`. Anything
  you grep for in the real implementations lives behind the positive tags.

---

## 10. Recommended next steps, in priority order

1. **`govulncheck ./...`** — minutes, needs only network. Closes limitation 1.
2. **Reproduce OYS-11** (§7). Requires a build. Converts the top Oyster finding
   from predicted to confirmed — or refutes it, which is equally valuable.
3. **Commission a cryptographic audit** scoped to the plonky2 fork delta and
   `zk-pow/src/circuit/`. This is procurement, not analysis — it cannot be done
   by an AI assistant. Least Authority did the upstream plonky2 audits.
4. **Review `node/btcutil` address/amount encoding** — was Tier-1 in the plan,
   never reached, and it is fund-relevant.
5. **Read the plonky2 fork delta line by line**, especially every
   `oracles_to_skip` call site: confirm each skipped oracle is genuinely
   recomputed by the verifier and bound to the transcript.
6. **Audit the 163 Rust `unsafe` blocks** (161 plonky2 inherited, 2 flagged as
   RS-1). Running Miri over `zk-pow` trace generation would settle RS-1.

---

## 11. Standing instructions for whoever continues

- **Do not overstate.** Distinguish *verified*, *analysed but not reproduced*,
  and *not examined*. The existing report is careful about this; keep it that
  way. If you cannot verify something, say so.
- **Do not file anything with Pearl Research Labs** without the user's explicit
  instruction. Their policy is private disclosure via GitHub advisories; the
  text is ready in `02-advisories-to-file.md`.
- **Do not route around blocked hosts or disable TLS verification.** Report the
  blocked host and let the user change their egress policy.
- **The user is a wallet holder, not a maintainer.** Lead with what changes
  their exposure. As of this handoff that is: **PDW-1** (avoid importing a
  wallet into the desktop app on a shared machine until fixed), **OYS-11** (only
  if using `pq=true` addresses), and the **PRL name collision** below.

### The PRL name collision — highest *practical* risk, and not a code bug

"Oyster"/"PRL" collides with
[Oyster Protocol / Oyster Pearl (PRL)](https://www.ccn.com/oyster-protocol-founder-exit-scams-steals-300000-from-ico-smart-contract/),
a 2018 ERC-20 whose founder kept a `transferDirector` trapdoor, reopened the
ICO, minted ~3M PRL, dumped them, and vanished; he was
[later arrested](https://decrypt.co/50951/feds-arrest-crypto-founder-behind-multimillion-dollar-exit-scam).
Etherscan still flags that contract. It is unrelated to Pearl Research Labs —
but anyone searching "PRL" can land on the dead scam token or an impersonator.
There is also a separate `PearlBridgeXYZ/pearlwallet` browser wallet that was
**not** reviewed.
