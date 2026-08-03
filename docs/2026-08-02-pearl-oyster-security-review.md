# Security Review — Pearl (Oyster wallet daemon & Pearl Desktop Wallet)

**Target:** `github.com/pearl-research-labs/pearl`
**Revision reviewed:** `83cf4cb` (master), full history (6,778 commits) and all 8 release tags
**Date:** 2026-08-02
**Reviewer:** independent source review (unsolicited)
**Method:** source review only — no dynamic testing, no exploitation against live systems

---

## 0. Why this review happened

The [Coldcard firmware vulnerability disclosed 30–31 July 2026](https://thehackernews.com/2026/08/coldcard-hardware-wallet-flaw-linked-to.html)
caused seed generation to silently fall back from the hardware TRNG to a
predictable software RNG, allowing an attacker to reconstruct seeds and sweep
~1,083 BTC. This review began as a check for that specific bug class in Oyster,
then widened.

**Oyster does not have the Coldcard bug, and never did.** See §1. The findings
in §2–§3 were discovered while widening the review.

---

## 1. Verified clean

These were checked deliberately and passed. Listed so maintainers know what
does *not* need re-verification.

### 1.1 Seed generation — no weak-RNG fallback, across all history

`node/btcutil/hdkeychain/extendedkey.go:736`

```go
func GenerateSeed(length uint8) ([]byte, error) {
	if length < MinSeedBytes || length > MaxSeedBytes {
		return nil, ErrInvalidSeedLen
	}
	buf := make([]byte, length)
	_, err := rand.Read(buf)   // "crypto/rand"
	...
}
```

Verification performed:

- Only 5 commits in repo history ever touched this file. The `GenerateSeed`
  body hashes **identically** at all 5 (`git log --follow` + per-commit body hash).
- `"math/rand"` is imported in that file at **0** of those 5 commits.
- Pickaxe (`git log -S'math/rand' --all`) over `wallet/`, `xmss/`,
  `hdkeychain/`, `btcec/` returns only chain-polling jitter, coin selection,
  and change-index randomization — never a key-derivation path.
- All 8 release tags (`pearl-wallet-v1.0.0`…`v2.0.4`, `v1.1.5`…`v1.2.1`)
  carry a byte-identical `GenerateSeed`, the same 128-bit entropy, and the
  same `snacl prng = rand.Reader`.

There is no released version of Oyster in which seed entropy came from a
non-cryptographic source.

### 1.2 Other paths verified

| Area | Result |
|---|---|
| Signing nonces | RFC6979 deterministic (`btcec/schnorr/signature.go:521`) — no random-`k` reuse risk |
| BIP39 derivation | 128-bit entropy → 12-word mnemonic → PBKDF2 → 64-byte BIP32 seed (standard) |
| Salts / nonces / crypto keys | `crypto/rand` throughout (`snacl.go:22`, `waddrmgr/manager.go:967,1549,1835`) |
| Legacy RPC auth | `subtle.ConstantTimeCompare` (`legacyrpc/server.go:269`); **fails closed** — server disabled without credentials |
| Default RPC bind | localhost only (`config.go:629-644`) |
| On-disk permissions (daemon) | `wallet.db` `0600` (`walletdb/bdb/db.go:473`), dirs `0700` |
| Secret logging | None — no seed, passphrase, or private key reaches logs |
| Secret zeroing (waddrmgr) | Thorough `zero.Bytes` coverage |
| `oystercli` setup-file handling | Correct: `MkdirTemp` (0700) + `0o600` + `defer os.RemoveAll` |
| XMSS consensus opcode | Tapscript-only, validates pubkey + all 5 chunk lengths, tallies op cost, enforces NullFail (`txscript/opcode.go:2024`) |
| BIP-322 replay safety | `to_spend` uses null outpoint (32 zero bytes, index `0xffffffff`) and zero amount — a message signature cannot be replayed as a spend |
| Release pipeline | `workflow_dispatch` only; all third-party actions SHA-pinned; `persist-credentials: false`; `environment: release`; `draft: true`; binaries **promoted** from the tested build, not rebuilt |
| `pull_request_target` usage | Safe — `types: [labeled]` + `safe-to-test` gate, and `remove_safe_label.yml` strips the label on every `synchronize`, closing the label-then-push TOCTOU |
| Auto-updater | Notification-only — queries the releases API, compares semver, opens the release page. Never downloads or executes code (`"publish": null`) |
| Fee/change arithmetic | Correct — `insufficientFunds` and `remainingAmount < maxRequiredFee` checks dominate, so `changeAmount` cannot underflow (`txauthor/author.go:100-136`) |
| Change position randomization | `cprng` (crypto-seeded); `ChangeIndex >= 0` guarded at the call site (`createtx.go:279`) |
| SPV block delivery | Sound — block hash checked against the validated header chain, plus `CheckBlockSanity` and `ValidateWitnessCommitment`, with peer banning (`spv/query.go:857-900`) |
| SPV filter withholding | Cross-peer filter validation intact (`detectBadPeers`, `resolveFilterMismatchFromBlock`) |
| SPV header validation | PoW enforced; `BFNoPoWCheck` set only for SimNet (`spv/blockmanager.go:2849`) |
| ZK-PoW / XMSS build stubs | **Fail closed** — `zkpow` stub returns an error, `xmss.Verify` returns `false`. CI builds via `task build:blockchain` → `-tags xmss,zkpow` |
| Certificate verification | Error propagated as `ruleError(ErrHighHash, …)`, not logged and swallowed (`blockchain/validate.go:333`) |
| Difficulty retarget (WTEMA) | Sound — mainnet `T=194s`, half-life 1 week (filter constant ≈ 0.00032), `MaxTimeOffsetMinutes=5` (Bitcoin uses 120). Max single-block timestamp manipulation moves the target ≈0.05%; target clamped to `[1, PowLimit]`; `ReduceMinDifficulty` panics if ever set on mainnet |
| Timestamp rules | **Stricter than Bitcoin** — median-time-past is replaced by strict monotonicity (`MinTimestampDeltaSeconds = 1`, `validate.go:669-675`), guaranteeing `t ≥ 1` for the WTEMA; future drift capped at `now + 5min` |
| `wtxmgr` reorg handling | Standard upstream rollback with coinbase-credit tracking and recursive `removeConflict` / `removeDoubleSpends` |
| ZK-PoW parameter soundness | FRI `(rate_bits, pow_bits)` are proof-supplied **but allowlisted** to 4 exact tuples (`pearl_circuit.rs:129-133`); query count is not proof-controlled; `stark_degree_bits ≤ 19` and `degree+rate ≤ 20` enforced |
| ZK-PoW difficulty binding | `hash_jackpot` is a circuit public input and is checked against the header's `nbits` (`sanity_checks.rs`), so PoW difficulty is bound to the proof |
| BIP324 v2 transport | Faithful port — nonce is `msgctr(4) ‖ rekeyctr(8)`, rekey nonce prefixed `0xffffffff`, `MaxGarbageLen = 4095` per spec (`v2transport/chacha.go`, `transport.go:40`) |
| Miner RPC exposure | UDS default at `0600`; TCP mode binds hard-coded `127.0.0.1` regardless of configured `host` (`miner_rpc/server.py:106`) |
| Repo-wide secret sweep | No hardcoded keys, tokens, or passwords matching high-entropy patterns across Go/Rust/TS/Python |
| Mempool policy | Full DoS limit set intact — `MaxOrphanTxs`, `MaxOrphanTxSize`, `MinRelayTxFee`, `MaxMempoolSize`, orphan expiry scan |
| Eclipse resistance (addrmgr) | Netgroup bucketing intact (1024 new / 64 tried, 64 per group); bucket key seeded from `crypto/rand` (`addrmanager.go:713`) |
| Rust `unsafe` footprint | `pearl-blake3` 0, `miner/` 0; zk-pow 20 (18 in FFI/bindings, 2 flagged as RS-1); plonky2 161, inherited from upstream |
| p2p message limits | Standard btcd bounds intact — `MaxMessagePayload` 32 MB, `MaxInvPerMsg` 50 000, `MaxAddrPerMsg` 1 000, `MaxBlockPayload` 4 MB |

---

## 2. Findings — Pearl Desktop Wallet (`apps/apps/pearl-desktop-wallet`)

### PDW-1 — Plaintext seed and passphrase written to a world-readable file  ·  **High**

**Location:** `src/main/services/wallet-process.ts:140-147`, dir created at
`src/main/services/manager-service.ts:66`

```js
walletConfigFile = path.join(this.config.dataDir, 'wallet-setup.json');
const walletConfig = {
  seed,                          // full BIP39 mnemonic, on IMPORT
  privatepassphrase: passphrase,
  bday: isImport ? '1724644369' : undefined,
};
fs.writeFileSync(walletConfigFile, JSON.stringify(walletConfig, null, 2));
```

`fs.writeFileSync` is called with **no `mode`** → Node defaults to
`0o666 & ~umask`, i.e. **0644** under the usual `umask 022`. The containing
directory is created by `fs.mkdirSync(walletDataDir, { recursive: true })` —
also with no mode → **0755**, world-traversable.

**Impact.** During wallet creation/import,
`~/.pearl-wallet/wallet-data/<name>/wallet-setup.json` holds the user's
mnemonic and passphrase in cleartext, readable by any other local user account
or any process running as a different user. On the import path this is the
complete key material — full, irreversible loss of funds.

**Repro.**
1. Import an existing wallet in the desktop app.
2. Concurrently, as a *different* local user: `cat ~<victim>/.pearl-wallet/wallet-data/*/wallet-setup.json`
3. Observe the plaintext mnemonic and passphrase.

**Aggravating factors.**
- Cleanup (`fs.unlinkSync`) is wired only to the child process `close` (:194)
  and `error` (:223) handlers. A SIGKILL of the Electron app, a power loss, or
  an OS crash mid-creation leaves the file **permanently** on disk.
- `unlink` does not wipe — the plaintext remains recoverable from free blocks.
- Written to the **persistent** data dir, not a temp dir or tmpfs.
- 0644 means user-level backup/sync agents (Time Machine, Dropbox, cloud
  backup) can capture and replicate it.

**Note.** Oyster's own CLI already implements this correctly
(`wallet/cmd/oystercli/createwallet.go:126-146`) with a 0700 temp dir, `0o600`
file mode, and `defer os.RemoveAll`. The desktop app reimplemented the same
interface and dropped every protection. The fix is to adopt the existing
pattern.

**Remediation.**
```js
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pearl-setup-'));  // 0700
const walletConfigFile = path.join(dir, 'wallet-setup.json');
fs.writeFileSync(walletConfigFile, JSON.stringify(walletConfig), { mode: 0o600 });
// remove the whole directory in a finally / process-exit handler
```
Also register cleanup on `process.on('exit')` and `uncaughtException`, and
consider overwriting the file contents before unlinking.

---

### PDW-2 — Wallet RPC credentials passed as command-line arguments  ·  **Medium**

**Location:** `src/main/services/wallet-process.ts:87-92`

```js
const args = [
  `--username=${this.config.rpcUser}`,
  `--password=${this.config.rpcPassword}`,
  `--rpclisten=127.0.0.1:${networkConfig.rpcPort}`,
```

Process command lines are world-readable via `ps` and `/proc/<pid>/cmdline` on
default Linux/macOS configurations.

**Impact.** Any local user recovers the wallet RPC credentials and can reach
the daemon on `127.0.0.1`. Read access to balances, addresses, and transaction
history is immediate. If the wallet is currently unlocked (see OYS-6), the
attacker can **spend**.

Credentials themselves are well-generated — ephemeral per session via
`randomBytes(16)`/`randomBytes(32)` (`manager-service.ts:19-21`) — the defect
is purely the transport.

**Remediation.** Pass credentials via environment variable, stdin, or a
`0600` config file.

---

### PDW-3 — Releases are unsigned and un-notarized  ·  **Medium**

**Location:** `electron-builder.json`

```json
"mac":  { "hardenedRuntime": true, "notarize": false, ... },
"win":  { "target": [{ "target": "nsis", ... }] }   // no signing config
```

No signing step exists in `.github/workflows/pearl-desktop-wallet.yml`.
`SIGNING_GUIDE.md` documents signing as a **manual, local** procedure driven by
`APPLE_ID` / `APPLE_APP_SPECIFIC_PASSWORD` env vars.

**Impact.** Distributed builds carry no cryptographic publisher binding.
Users are trained to click through Gatekeeper/SmartScreen warnings — which is
exactly the conditioning a malicious lookalike build needs. Manual signing is
also not reproducible and cannot be audited from CI logs.

**Remediation.** Move signing and notarization into the release workflow with
secrets scoped to the `release` environment.

---

### PDW-4 — Electron hardening gaps  ·  **Low**

**Location:** `src/main/services/window-service/create-window.ts:18,29`

- `sandbox: false` — renderer sandbox disabled. (`contextIsolation: true` and
  `nodeIntegration: false` are correctly set.)
- `setWindowOpenHandler` calls `shell.openExternal(details.url)` with **no
  scheme validation** — any URL scheme is handed to the OS.
- No `will-navigate` guard.

**Remediation.** Enable `sandbox: true`; allowlist `https:` (and `mailto:`)
before calling `openExternal`; add a `will-navigate` handler that blocks
off-origin navigation.

---

## 3. Findings — Oyster daemon (`wallet/`)

### OYS-1 — `--createfromfile` does not validate file permissions  ·  **Medium** (defense in depth)

**Location:** `wallet/config.go:542-546`, `wallet/walletsetup.go:120`

Oyster reads the setup JSON — which by its own flag documentation contains
`PrivatePassphrase` and `Seed` — with a bare `os.ReadFile`, performing no
permission check and no cleanup. The interface places the entire burden on
callers.

Oyster's own CLI caller does this correctly. The desktop app (PDW-1) did not,
and nothing at the boundary caught it.

**Remediation.** `os.Stat` the file and refuse (or loudly warn) when
`mode & 0o077 != 0`. This single check would have converted PDW-1 from a silent
key-material exposure into a startup error.

---

### OYS-2 — Seed printed to stdout unconditionally in `--createfromfile` mode  ·  **Medium**

**Location:** `wallet/config.go:552-554`

```go
// Print the seed so the operator can back it up.
fmt.Println(seedHex)
os.Exit(0)
```

**Impact.** Under any supervisor that captures stdout — systemd (journald),
Docker, Kubernetes, a process manager, or a shell redirect — the **mnemonic is
written into system logs**, which are typically group-readable, retained,
rotated, and shipped to centralized logging. This converts a one-time console
display into durable, often remotely-replicated plaintext key material.

The interactive path (`internal/prompt/prompt.go:258`) is appropriate — it is
an explicit, human-facing backup ceremony. The `--createfromfile` path is
machine-driven and should not assume a human terminal.

**Remediation.** Gate the print behind an explicit `--show-seed` flag, or write
it to a caller-specified `0600` path, or emit it only when
`term.IsTerminal(os.Stdout.Fd())`.

---

### OYS-3 — Experimental gRPC server has TLS but no authentication  ·  **Medium**

**Location:** `wallet/rpcserver.go:112-118`

```go
creds := credentials.NewServerTLSFromCert(&keyPair)
server = grpc.NewServer(grpc.Creds(creds))
rpcserver.StartVersionService(server)
rpcserver.StartWalletLoaderService(server, walletLoader, activeNet)
```

`NewServerTLSFromCert` provides **server-side TLS only** — `ClientAuth` is not
set, so there is no mutual TLS, and no username/password interceptor is
installed. The legacy RPC server, by contrast, explicitly refuses to start
without credentials (`rpcserver.go:129-130`).

**Impact.** A user who sets `--experimentalrpclisten` on a non-loopback
interface exposes `WalletLoaderService` (wallet create/open/close) to
**unauthenticated** callers. The asymmetry is the hazard: operators reasonably
assume the credential requirement they configured for legacy RPC also covers
gRPC.

**Mitigating.** `ExperimentalRPCListeners` is empty by default, so the gRPC
server is opt-in and not started in default deployments. Inherited from
upstream btcwallet.

**Remediation.** Require the same credential gate as legacy RPC (fail closed),
or require client certificates, or at minimum refuse to bind a non-loopback
address without authentication configured.

---

### OYS-11 — Recovery lookahead uses a different address-derivation convention than PQ address creation  ·  **High** (conditional: opt-in PQ addresses only)

**Location:**
`wallet/wallet/wallet.go:960,990` (`expandScopeHorizons`) vs
`wallet/wallet/recovery.go:87,107`, with the gate at
`wallet/waddrmgr/scoped_manager.go:547`

A BIP-86 taproot output key is `internalKey + H_TapTweak(internalKey ‖ merkleRoot)·G`.
Supplying a tapscript merkle root therefore yields a **different address** than
the key-only tweak — this is BIP-341 arithmetic, not an implementation
question. `newManagedAddress` reflects exactly this
(`waddrmgr/address.go:521-523`):

```go
if tapscriptRoot != nil && len(tapscriptRoot) == 32 {
    tapKey = txscript.ComputeTaprootOutputKey(pubKey, tapscriptRoot)
}
```

Three independent inputs decide whether the XMSS tapscript root is applied,
and they disagree:

1. **Per-address opt-in at creation.** `usePQ := cmd.PQ != nil && *cmd.PQ`
   (`legacyrpc/methods.go:606,635`) — defaults to **false**.
2. **The recovery flag.** `expandScopeHorizons` — the lookahead window that
   discovers funds at indexes the wallet does not yet know — passes
   `includePQTapscript=false`. `recovery.go`, which re-derives only *already
   known* indexes, passes `true`.
3. **Lock/watch-only state.** `maybeDeriveTapscriptRoot` additionally requires
   `key.IsPrivate()`, and `nextAddresses` selects `acctKeyPub` whenever the
   manager is locked or watch-only (`scoped_manager.go:1239-1243`).

The tapscript root *is* persisted per address
(`waddrmgr/db.go:1408-1420`, encrypted), so a wallet with an intact database
spends correctly. That persistence is precisely what is missing during a
**seed-only restore**.

**Impact.** On a fresh restore the wallet database is empty, so
`ExternalKeyCount` is 0 and the `recovery.go` loops are no-ops — *every*
address is derived through `expandScopeHorizons`, which uses the **non-PQ**
convention. A user who opted into PQ addresses and later restores from the
mnemonic alone will have the rescan search for the wrong scripts, and **funds
received at those addresses are not discovered**.

The funds are not cryptographically lost — the seed still controls them — but
the shipped recovery path will not find them, which for most users is
indistinguishable from loss.

**Scope limiter.** PQ addresses are opt-in and default to false, and the
desktop wallet never sets the flag (`preload/index.ts` invokes
`wallet-get-new-address` with no PQ argument). Users who never passed
`pq=true` are unaffected, and for them recovery is self-consistent.

**Caveat.** This is control-flow analysis, not a dynamic reproduction —
building the tree requires the cgo XMSS and Rust ZK-PoW static libraries.
The BIP-341 arithmetic is certain; maintainers should confirm the code path
empirically with a round-trip test.

**Remediation.**
- Record the derivation convention per account (or per address index) in a
  form that survives into recovery — e.g. a scope-level flag persisted in the
  account record and re-read before the lookahead runs.
- Failing that, have `expandScopeHorizons` derive **both** variants into the
  recovery window; the cost is a doubled window, and the alternative is
  undiscoverable funds.
- Add a round-trip regression test: create a PQ address, discard the database,
  recover from seed, assert the address is rediscovered.

---

### OYS-12 — XMSS tapscript commitment silently omitted when the wallet is locked or watch-only  ·  **Medium**

**Location:** `wallet/waddrmgr/scoped_manager.go:540-551`

```go
// 3. The key is private (not watch-only/imported account)
// Returns nil (no tapscript root) if any condition is not met.
if s.scope == KeyScopeBIP0086 && includePQTapscript && key.IsPrivate() {
	return s.deriveTapscriptRoot(ns, path)
}
return nil, nil
```

`nextAddresses` selects the **public** account key whenever the manager is
locked or the account is watch-only (`scoped_manager.go:1239-1243`), so
`key.IsPrivate()` is false in those states.

**Impact.** A caller that explicitly requests a post-quantum address
(`getnewaddress … pq=true`) while the wallet is locked or watch-only receives
an ordinary BIP-86 address instead. No error is returned and no warning is
logged — the request silently degrades. The user believes the address carries
the XMSS fallback commitment; it does not.

This also means a watch-only companion wallet derives a different address set
than the spending wallet for the same account, so it will not observe funds
sent to PQ addresses.

**Remediation.** Return an explicit error when `includePQTapscript` is
requested but cannot be honoured, rather than silently returning `nil`. Fail
closed: a user asking for post-quantum protection should never receive a
non-PQ address without being told.

---

### OYS-4 — Weak scrypt parameters for wallet encryption  ·  **Low**

**Location:** `wallet/snacl/snacl.go:38-40` — `N=16384 (2^14)`, `r=8`, `p=1`

The original 2009 "interactive" scrypt parameters. Soft against modern offline
GPU/ASIC brute-force of a stolen `wallet.db`.

**Remediation.** Raise `N` to `2^17`+ for new wallets (with a stored parameter
set so existing wallets still open), or migrate to Argon2id.

---

### OYS-5 — `InsecurePubPassphrase` default  ·  **Low** (privacy)

**Location:** `wallet/wallet/wallet.go:46`, applied at `walletsetup.go:85,143`
and `rpc/rpcserver/server.go:693,718`

The outer/public passphrase defaults to the literal `"public"`. Anyone with the
wallet file can enumerate addresses and transaction history. Private keys
remain protected by the private passphrase.

Inherited from btcwallet and explicitly named "Insecure", so this is a
documentation/UX issue rather than a surprise — but the desktop app never
surfaces the choice to users.

---

### OYS-6 — `walletpassphrase` with `timeout=0` unlocks indefinitely  ·  **Low**

**Location:** `wallet/rpc/legacyrpc/methods.go:1837-1842`

```go
timeout := time.Second * time.Duration(cmd.Timeout)
if timeout != 0 {
	unlockAfter = time.After(timeout)
}
err := w.Unlock([]byte(cmd.Passphrase), unlockAfter)
```

`timeout == 0` leaves `unlockAfter` nil — the wallet never auto-relocks.
Combined with PDW-2, this widens the window in which a local attacker can
spend.

**Remediation.** Enforce a maximum timeout and reject `0`.

---

### OYS-7 — BIP-322 implementation deviates from the specification  ·  **Informational**

**Location:** `wallet/rpc/legacyrpc/bip322.go:48-59`

The message is placed in an `OP_RETURN` output of `to_sign`, rather than in
`to_spend`'s scriptSig as the BIP-340 tagged hash
(`SHA256(tag) || SHA256(tag) || message`) that BIP-322 requires.

**Not a security defect** — the sighash still commits to the message, and the
null-outpoint/zero-amount construction preserves replay safety. But signatures
**will not verify against conforming BIP-322 implementations**, and
`NullDataScript` caps messages at 80 bytes. Recommend either conforming to the
spec or renaming the API so it does not claim BIP-322 compatibility.

---

### OYS-8 — Dead, deprecated global PRNG seeding  ·  **Informational**

**Location:** `wallet/wallet/rand.go`

```go
func init() { rand.Seed(time.Now().Unix()) }
```

`rand.Seed` has been deprecated since Go 1.20; the module targets Go 1.26, so
the runtime's secure auto-seeding governs. The only consumer of the global
source is the coin-selection shuffle (`wallet/wallet/createtx.go:577`) — a
privacy consideration at most, never key material. The file should be deleted.

---

### OYS-9 — Stale documentation on `RecommendedSeedLen`  ·  **Informational**

**Location:** `node/btcutil/hdkeychain/extendedkey.go:32,734`

The constant is `16` (128 bits) but the `GenerateSeed` doc comment three lines
above still reads *"The recommended length is 32 (256 bits) as defined by the
RecommendedSeedLen constant."* The code is correct — 128 bits is standard
12-word BIP39 strength — the comment is not.

---

### GW-1 — Pearl node RPC password written to logs at INFO level  ·  **Medium**

**Location:** `miner/pearl-gateway/src/pearl_gateway/pearl_client.py:29-31`

```python
logger.info(
    f"PearlNodeClient initialized with rpc_url: {self.rpc_url}, "
    f"rpc_user: {config.rpc_user}, rpc_password: {config.rpc_password}"
)
```

The pearld RPC password is interpolated into an `INFO`-level log line, so it
is emitted under default logging configuration — not only in debug mode.

**Impact.** Under systemd/journald, Docker, Kubernetes, or any log shipper,
the credential controlling the Pearl node's RPC interface is written to
durable, often centrally-aggregated storage. Anyone with log read access
gains node RPC control. On a mining host whose node also holds a wallet, that
is a direct path to funds.

Same class as OYS-2, different component and different credential.

**Remediation.** Remove the credential from the log line; log the username and
URL only, or redact.

---

### GW-2 — Weak default RPC credentials in gateway config  ·  **Low**

**Location:** `miner/pearl-gateway/src/pearl_gateway/config.py:20-22`

```python
rpc_url: str = "http://0.0.0.0:44107"
rpc_user: str = "user"
rpc_password: str = "pass"
```

These are client-side defaults, so they are only usable if the operator
configured pearld with matching credentials — but shipping `user`/`pass` as
the default actively encourages exactly that. `0.0.0.0` is also incorrect as a
*destination* address (it is a bind address; it happens to resolve to loopback
on Linux) and should be `127.0.0.1`.

**Remediation.** Leave the credentials unset and fail fast with a clear error
when they are missing, as is already done for `mining_address` (which has no
default). Change the URL default to `http://127.0.0.1:44107`.

---

### GW-3 — Unix socket created in world-writable `/tmp` before permissions are set  ·  **Informational**

**Location:** `miner/pearl-gateway/src/pearl_gateway/miner_rpc/server.py:88-99`,
default `socket_path = "/tmp/pearlgw.sock"`

```python
if os.path.exists(self.config.socket_path):
    os.unlink(self.config.socket_path)
self.server = await asyncio.start_unix_server(...)
os.chmod(self.config.socket_path, 0o600)
```

The socket is created with the process umask and only then narrowed to `0600`,
leaving a brief window in which another local user may connect. The
`exists → unlink → create` sequence is itself a race in a world-writable
directory.

The final state is correct (`0600`), and TCP mode binds hard-coded to
`127.0.0.1` regardless of the configured `host`, so exposure is minimal.

**Remediation.** Create the socket inside a `0700` directory owned by the
service (e.g. under `$XDG_RUNTIME_DIR`), or set the umask before binding.

---

### PKG-1 — `@pearl/pearl-address-validation` accepts Bitcoin base58 addresses as valid Pearl addresses  ·  **Medium** (latent — no in-repo consumer)

**Location:** `apps/packages/pearl-address-validation/src/index.ts`

The package validates bech32m/taproot addresses correctly: it requires a
`prl1p` / `tprl1p` / `rprl1p` prefix, decodes with bech32m, enforces witness
version 1, and requires a 32-byte witness program. That path is sound.

The **base58 fallback path** is not. It accepts any base58check string whose
version byte is in:

```js
const addressTypes = {
  0x00: { type: p2pkh, network: mainnet },   // Bitcoin P2PKH
  0x6f: { type: p2pkh, network: testnet },
  0x05: { type: p2sh,  network: mainnet },   // Bitcoin P2SH
  0xc4: { type: p2sh,  network: testnet },
};
```

These are **Bitcoin's** version bytes. Pearl's `node/chaincfg/params.go`
defines no `PubKeyHashAddrID` and no `ScriptHashAddrID` at all — the chain is
taproot-only (`txauthor/author.go:110-116` accepts only P2TR and P2MR
scripts). There is no such thing as a legacy base58 Pearl address.

**Impact.** `validate("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa")` returns `true`
with `network: mainnet`. Any consumer using this library to gate a
withdrawal or send flow would accept a Bitcoin address as a valid Pearl
mainnet destination. Funds sent there would pay a script no Pearl key can
redeem — permanent loss.

**Why this is Medium, not High.** The package has **zero importers in the
repository**, and Pearl Desktop Wallet does not use it — the desktop wallet
validates via the daemon's `validateaddress` RPC
(`preload/index.ts` → `wallet-validate-address`), which is the correct path.
The hazard is latent: this is a versioned, published-style package
(`@pearl/pearl-address-validation` v3.0.0, `main: dist/index.js`) evidently
intended for external consumers such as exchanges or integrators.

**Remediation.** Delete the base58 branch and the `addressTypes` table
entirely; on a taproot-only chain any non-bech32m input should be rejected
outright.

**Minor, same file.** `mapPrefixToNetwork` maps `rprl → simnet`, but Pearl
uses the `rprl` HRP for **both** regtest (`params.go:476`) and simnet
(`params.go:754`), so the two are indistinguishable and regtest is
mislabelled. Low impact — both are test networks.

---

### DEP-1 — `google.golang.org/grpc v1.82.0` is affected by GHSA-hrxh-6v49-42gf  ·  **Low** for default deployments (High CVSS, but the affected server is opt-in)

**Location:** `go.mod` — `google.golang.org/grpc v1.82.0`

[GHSA-hrxh-6v49-42gf](https://github.com/advisories/GHSA-hrxh-6v49-42gf)
(CVSS 8.8, High) affects all `google.golang.org/grpc` versions **< 1.82.1**.
Pearl pins v1.82.0, one patch release behind the fix. It bundles three
server-role issues:

| Sub-issue | Applies to Pearl? |
|---|---|
| xDS RBAC authorization bypass (`Metadata` / `RequestedServerName` matchers silently ignored) | **No** — Pearl does not use xDS |
| **HTTP/2 Rapid Reset DoS** — rapid stream create/terminate bypasses reader blocking, high CPU | **Yes**, for any gRPC server |
| xDS RBAC engine panic on `NOT`-wrapped unsupported field | **No** — no xDS |

**Impact.** Only the Rapid Reset DoS is reachable. Oyster's gRPC surface is
the experimental `WalletLoaderService`, which is **not started by default**
(`ExperimentalRPCListeners` is empty unless configured — see OYS-3). An
operator who enables it exposes a remote CPU-exhaustion vector. No key
material is at risk.

**Remediation.** `go get google.golang.org/grpc@v1.82.1`.

**Checked and NOT affected:**

| Dependency | Pinned | Advisory status |
|---|---|---|
| `golang.org/x/crypto` | v0.53.0 | The GO-2026-5005/5006/5013/5017/5018/5019/5020/5021/5023/5033 cluster was fixed in **v0.52.0** — ahead of it |
| `golang.org/x/net` | v0.56.0 | GO-2026-5026 fixed in **v0.55.0**; CVE-2026-33814 fixed earlier — ahead of both |
| `go.etcd.io/bbolt` | v1.5.0 | Current release; no advisory found |
| `google.golang.org/grpc` | v1.82.0 | CVE-2026-33186 (CVSS 9.1 authz bypass) fixed in 1.79.3 — **not** affected by that one |

**Method caveat — this is not a substitute for `govulncheck`.** The review
environment's egress policy blocks `vuln.go.dev` and `api.osv.dev` (403 on
CONNECT), so this is *version matching against published advisories*, not
`govulncheck`'s call-graph reachability analysis. It can miss advisories it
did not search for, and it cannot tell whether a vulnerable code path is
actually reachable from Pearl's code. Maintainers should still run
`govulncheck ./...` in CI.

**Ancillary observation.** Two transitive btcsuite dependencies are pinned to
pseudo-versions roughly a decade old — `github.com/btcsuite/websocket`
(2015-01-19) and `github.com/btcsuite/go-socks` (2017-01-05). Both are
inherited from btcd upstream and neither has a published advisory, but both
are effectively unmaintained forks and worth tracking.

---

### ZKP-2 — plonky2 fork modifies soundness-critical code that the bundled audits do not cover  ·  **Informational** (assurance gap, not a defect)

**Location:** `plonky2/` (Pearl fork), `plonky2/audits/`

`plonky2/README.md` states this is the Pearl fork of Polygon Zero's plonky2,
maintained by the Pearl team after upstream deprecated it in favour of
Plonky3. Upstream attribution is properly retained here.

`plonky2/audits/` contains two Least Authority reports — "Polygon Zero
Plonky2" and "Polygon Zero Starky & zkEVM Kernel". **Both audit upstream's
unmodified library.** Neither covers Pearl's fork, and neither covers Pearl's
own circuits in `zk-pow/src/circuit/`.

Diffing the fork against upstream `0xPolygonZero/plonky2` HEAD shows
modification concentrated in exactly the soundness-critical files:

| File | Changed lines |
|---|---|
| `plonky2/src/fri/verifier.rs` | 73 |
| `plonky2/src/plonk/verifier.rs` | 38 |
| `plonky2/src/plonk/get_challenges.rs` | 36 |
| `starky/src/get_challenges.rs` | 28 |
| `starky/src/config.rs` | 23 |
| `plonky2/src/fri/validate_shape.rs` | 12 |
| `plonky2/src/plonk/validate_shape.rs` | 6 |

plus a new `starky/src/pair_stark` module. (Some fraction is upstream drift
since the fork point rather than Pearl's work; upstream is deprecated, so
drift should be limited, but the split was not separated.)

The most consequential change is a new `oracles_to_skip` parameter on the FRI
verifier:

```rust
/// Verifies a FRI proof, skipping Merkle proof verification for oracle indices in
/// `oracles_to_skip`. Use when the verifier has computed those oracle evaluations itself.
/// Pass `&[]` to skip nothing.
```

Skipping Merkle verification for an oracle is sound **only if** the verifier
genuinely recomputes those evaluations and binds them to the transcript. That
is the evident intent — it pairs with the new `preprocessed_columns` concept,
which the verifier computes itself (`zk-pow/src/api/verify.rs` supplies
`preprocessed_columns` as a public input). But an incorrect `oracles_to_skip`
at any call site would let a prover supply arbitrary values for a skipped
oracle, which is a total soundness break.

**This is not a reported vulnerability.** No misuse was found, and the spot
checks performed were positive — see below. It is recorded as an **assurance
gap**: the audited artifact and the shipped artifact are not the same code,
and the delta lands precisely in the verifier.

**Positive signal.** The `starky/src/config.rs` change observes the new
`preprocessed_columns` into the Fiat-Shamir challenger with a length prefix
for state separation, and mirrors it correctly in *both* the native and
in-circuit recursive verifiers:

```rust
// Include length as first element for state separation
let mut prep_cols = vec![F::from_canonical_usize(self.preprocessed_columns.len())];
prep_cols.extend(self.preprocessed_columns.iter().map(|&i| F::from_canonical_usize(i)));
challenger.observe_elements(&prep_cols);
```

Binding new public parameters into the transcript, length-prefixed, in both
verifier forms is what a soundness-aware author does. This raises confidence
in the fork's quality without substituting for an audit.

**Recommendation.** Commission a cryptographic audit scoped to the fork delta
and to `zk-pow/src/circuit/`, and state plainly in `plonky2/README.md` that
the bundled audits apply to upstream and not to this fork.

---

### RS-1 — Unsound `unsafe` aliasing in BLAKE3 trace generation  ·  **Low** (prover-side only)

**Location:** `zk-pow/src/circuit/chip/blake3/trace.rs:111` and the identical
`zk-pow/src/v1/circuit/chip/blake3/trace.rs:113`

```rust
blocks.par_iter().for_each(|&(first, pivot, params)| {
    ...
    for row_idx in first..=pivot {
        let row = unsafe { &mut *(trace.as_ptr() as *mut [F; pearl_columns::TOTAL]).add(row_idx) };
```

Inside a rayon parallel iterator, `trace.as_ptr()` (a `*const` derived from a
shared reference) is cast to `*mut` and dereferenced as `&mut`. Writing
through a pointer derived from a shared reference violates Rust's aliasing
model, and shared reads of the same buffer (`&trace[row_idx]`) occur
concurrently with those writes. There is no `// SAFETY:` comment.

**In practice** the row ranges appear disjoint per block, so there is likely
no data race today. But this is undefined behaviour by the language rules —
Miri would reject it — and a future compiler is entitled to miscompile it.

**Severity is Low and bounded:** this is **trace generation in the prover**,
not the verifier. Miscompilation would yield invalid proofs (a miner wasting
work), not acceptance of invalid proofs. It is not a consensus-security
issue.

**Remediation.** Use `par_chunks_mut` over disjoint row ranges, or wrap the
buffer in a `SyncUnsafeCell`-style type with an explicit `// SAFETY:` comment
justifying disjointness. Note there are **two copies** of this code (`v1/` and
current); fix both.

`pearl-blake3` and `miner/` contain **zero** `unsafe` blocks.

---

### ZKP-1 — `extract_difficulty_bound` fails open on overflow  ·  **Informational**

**Location:** `zk-pow/src/api/sanity_checks.rs`, `extract_difficulty_bound`

```rust
if target_difficulty > U256::MAX / difficulty_adjustment_factor {
    info!("Difficulty is too easy: hardness={} h*w*k={}", ...);
    U256::MAX          // every hash satisfies the bound
} else {
    target_difficulty * difficulty_adjustment_factor
}
```

When the difficulty-adjustment factor would overflow, the function returns
`U256::MAX`, so `check_jackpot_against_nbits` accepts **any** jackpot hash.
The condition is logged at `info` level, not treated as an error.

**Not currently exploitable.** `checkProofOfWork` rejects
`target.Sign() <= 0` and `target > powLimit` (`blockchain/validate.go:311-323`)
*before* `VerifyCertificate` is reached, so `nbits` on mainnet cannot reach the
overflow range.

The concern is stylistic but consensus-adjacent: a fail-open default inside a
proof-of-work check depends on an invariant enforced in a different language,
in a different module, by a different call. If that ordering is ever changed
or a new caller invokes verification directly, the branch silently accepts
invalid work.

**Remediation.** Return an error rather than `U256::MAX`, and log at `warn`
or `error`.

---

### OYS-10 — Seed and passphrase not zeroed in the setup path  ·  **Informational**

**Location:** `wallet/walletsetup.go:157-193`

`waddrmgr` zeroes secrets thoroughly, but `seed` and `privPass` in
`createWalletFromJSON` are left to the garbage collector. Go makes reliable
wiping difficult, so this is best-effort — but `zero.Bytes` is already
available and used elsewhere in the tree.

---

## 4. Summary

| ID | Component | Severity | Finding |
|---|---|---|---|
| PDW-1 | Desktop | **High** | Plaintext seed/passphrase in world-readable file |
| PDW-2 | Desktop | Medium | RPC credentials on argv |
| PDW-3 | Desktop | Medium | Unsigned / un-notarized releases |
| PDW-4 | Desktop | Low | Electron `sandbox: false`, unvalidated `openExternal` |
| OYS-11 | Oyster | **High**\* | Recovery lookahead derivation convention mismatches PQ address creation — funds not rediscovered on seed-only restore |
| OYS-12 | Oyster | Medium | XMSS commitment silently omitted when wallet locked/watch-only |
| OYS-1 | Oyster | Medium | `--createfromfile` does not check file permissions |
| OYS-2 | Oyster | Medium | Seed printed to stdout → captured by system logs |
| OYS-3 | Oyster | Medium | Experimental gRPC server unauthenticated |
| OYS-4 | Oyster | Low | scrypt `N=2^14` |
| OYS-5 | Oyster | Low | `InsecurePubPassphrase` default |
| OYS-6 | Oyster | Low | `walletpassphrase timeout=0` never relocks |
| OYS-7 | Oyster | Info | BIP-322 spec deviation |
| OYS-8 | Oyster | Info | Dead `rand.Seed` |
| OYS-9 | Oyster | Info | Stale seed-length comment |
| OYS-10 | Oyster | Info | Secrets not zeroed in setup path |
| GW-1 | miner/gateway | Medium | Pearl node RPC password logged at INFO level |
| GW-2 | miner/gateway | Low | Weak default RPC credentials (`user`/`pass`) |
| GW-3 | miner/gateway | Info | UDS created in world-writable `/tmp` before chmod 0600 |
| PKG-1 | apps/packages | Medium\*\* | `pearl-address-validation` accepts Bitcoin base58 addresses as valid Pearl addresses |
| DEP-1 | go.mod | Low\*\*\* | `grpc v1.82.0` affected by GHSA-hrxh-6v49-42gf (HTTP/2 Rapid Reset DoS); fix is v1.82.1 |
| RS-1 | zk-pow | Low | Unsound `unsafe` aliasing in BLAKE3 trace gen (prover-side only; two copies) |
| ZKP-2 | plonky2 | Info | Fork modifies soundness-critical verifier code; bundled audits cover upstream only |
| ZKP-1 | zk-pow | Info | `extract_difficulty_bound` fails open on overflow (guarded upstream) |

\* OYS-11 is conditional: it affects only users who explicitly opted into PQ
addresses (`pq=true`), which is not the default and which the desktop wallet
never requests.

\*\*\* DEP-1 carries a High CVSS (8.8) upstream, but the only sub-issue that
applies to Pearl is a DoS against the gRPC server, which is not started in
default deployments.

\*\* PKG-1 is latent: the package has no importer in this repository and the
desktop wallet does not use it. It is rated Medium because it is a versioned,
distributable library evidently intended for external integrators.

**Revised bottom line.** Oyster's key generation, signing, authentication, and
on-disk permissions are sound, and the SPV and consensus validation paths hold
up. However OYS-11 means the daemon *can* fail to rediscover funds on a
seed-only restore for users of the opt-in post-quantum address type — so the
earlier statement that Oyster carried no fund-loss finding no longer holds
without that qualification. PDW-1 remains the only issue that discloses key
material outright.

---

## 5. Scope and limitations

Reviewed: `wallet/` (Oyster), `apps/apps/pearl-desktop-wallet`,
`node/btcutil/hdkeychain`, `node/btcec`, `node/txscript` (XMSS opcode),
`wallet/snacl`, `wallet/waddrmgr` (address & tapscript derivation),
`wallet/wallet/txauthor` (fee/change arithmetic), `spv/` (header, block, and
filter validation), `node/blockchain/validate.go` (certificate verification),
`node/zkpow` + `xmss/` build-tag stubs, `xmss/`, `.github/workflows/`,
`install.sh`.

Also reviewed in a later pass: difficulty retarget and timestamp rules
(`node/blockchain/difficulty.go`, `validate.go`), `wtxmgr` reorg and
double-spend handling, `node/wire` message bounds, and the ZK-PoW
**verifier API** — parameter validation (`zk-pow/src/circuit/pearl_circuit.rs`)
and difficulty binding (`zk-pow/src/api/verify.rs`, `sanity_checks.rs`).

**Not** reviewed: **ZK circuit soundness** — whether the AIR constraints in
`zk-pow/src/circuit/` actually enforce the claimed matrix-multiplication
computation, and the `plonky2/` fork itself. This is the single largest
remaining unknown and is **not assessable by source reading**; it requires a
specialist cryptographic audit with formal analysis of the constraint system.
A flaw there would let an attacker forge proofs of work, which is a
chain-integrity failure rather than a wallet failure, but it would make
confirmations meaningless.

Also not reviewed: `miner/`, `dnsseeder/`, `spv/` internals beyond validation,
and the PearlBridge browser wallet (separate repository).

Not performed:
- **Dependency CVE scan — partially closed.** `govulncheck` was rebuilt
  against Go 1.26.5, but the review environment's egress policy blocks
  `vuln.go.dev` and `api.osv.dev` (403 on CONNECT), so the reachability scan
  could not run. A manual advisory cross-check was performed instead and found
  **DEP-1** (`grpc v1.82.0`); `x/crypto` and `x/net` are ahead of their fix
  versions. This is version matching, **not** call-graph reachability
  analysis — it can miss advisories and cannot say whether vulnerable paths
  are reachable. **Maintainers should still run `govulncheck ./...` in CI.**
- Verification that released binaries correspond to this source; no
  reproducible-build or provenance attestation exists.
- Dynamic testing, fuzzing, or any exploitation attempt.

### Note on `install.sh`

`install.sh` verifies SHA-256 against `checksums.txt`, but `checksums.txt` is
fetched from the same GitHub release as the archive, and `checksums.txt` is
itself unsigned (`blockchain_release.yml:` "Generate checksums" step). This
defends against a corrupted download, **not** against a compromised release
pipeline or maintainer account. Consider cosign/sigstore or GPG signing, plus
SLSA provenance.

---

## 5b. Non-security observation — upstream copyright attribution

Not a vulnerability, and not legal advice — flagged because it is likely
unintentional and cheap to fix.

`node/` and `wallet/` are derived from btcd, btcwallet, and neutrino, all
ISC-licensed by Conformal Systems / the btcsuite developers. In the current
tree:

- 460 of 532 `.go` files under `node/` carry a `Pearl Research Labs`
  copyright header;
- **0** retain a btcsuite or Conformal notice;
- the root `LICENSE` credits `Pearl Research Labs` and `The Decred
  developers`, but not btcsuite.

The ISC license requires that "the above copyright notice and this permission
notice appear in all copies." The README does credit btcd/btcwallet/neutrino
at a project level, so this reads as an oversight during relicensing rather
than intent. Restoring the original notices alongside Pearl's — as was
evidently done for Decred — would resolve it.

## 5c. Consensus design observations (not findings)

Noted so a future reviewer does not re-derive them:

- **The script-flag system is vestigial.** `StandardVerifyFlags ScriptFlags = 0`
  (`txscript/standard.go:26`) and none of btcd's ~21 `ScriptVerify*` flags
  survive. Behaviour is hardcoded for a taproot-only chain. The checks those
  flags gated are still enforced unconditionally — `ErrCleanStack`,
  `ErrNullFail`, `ErrMinimalIf`, `ErrMinimalData`,
  `ErrDiscourageUpgradeableTaprootVersion`, and witness-program length
  (`engine.go:430,440`) all remain live. This is a simplification, not a
  weakening.
- **`OP_SUCCESS` is not implemented** — no active handling, only comments.
  This forecloses Bitcoin's taproot soft-fork upgrade mechanism. A design
  choice, not a defect.
- **`pearl-blake3`** wraps the audited `blake3` crate v1.8 but builds its
  Merkle tree via the crate's `hazmat` API (`merge_subtrees_root`,
  `merge_subtrees_non_root`) with hand-defined domain-separation flags. The
  primitive is sound; the custom tree construction is the part that warrants
  dedicated test vectors.

## 6. Unrelated ecosystem hazard (not a code defect)

"Oyster" and "PRL" collide with [Oyster Protocol / Oyster Pearl
(PRL)](https://www.ccn.com/oyster-protocol-founder-exit-scams-steals-300000-from-ico-smart-contract/),
a 2018 ERC-20 whose founder retained a `transferDirector` trapdoor, reopened
the ICO, minted ~3M PRL, sold them, and disappeared; he was
[later arrested](https://decrypt.co/50951/feds-arrest-crypto-founder-behind-multimillion-dollar-exit-scam).
Etherscan still carries a contract warning on that token.

This is a phishing and user-confusion hazard for Pearl Research Labs, not a
vulnerability. Prominent disambiguation in the README and on the project site
would help users avoid buying the defunct scam token or an impersonator.
