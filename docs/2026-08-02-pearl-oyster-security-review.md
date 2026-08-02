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

**The Oyster daemon has no finding that directly causes loss of funds.** Its
key generation, signing, authentication, and on-disk permissions are sound.
The one High-severity issue is confined to the Electron desktop wallet, on the
wallet-import path.

---

## 5. Scope and limitations

Reviewed: `wallet/` (Oyster), `apps/apps/pearl-desktop-wallet`,
`node/btcutil/hdkeychain`, `node/btcec`, `node/txscript` (XMSS opcode),
`wallet/snacl`, `wallet/waddrmgr`, `xmss/`, `.github/workflows/`, `install.sh`.

**Not** reviewed: consensus rules, `zk-pow/`, `plonky2/`, `miner/`, `spv/`
internals, `dnsseeder/`, the PearlBridge browser wallet (separate repository).

Not performed:
- **Dependency CVE scan.** `govulncheck` was rebuilt against Go 1.26.5 but the
  review environment's network policy blocked `vuln.go.dev` and `api.osv.dev`
  (403). Dependency versions look current (`x/crypto v0.53.0`,
  `x/net v0.56.0`, `grpc v1.82.0`) but were not machine-checked. **Maintainers
  should run `govulncheck ./...` — this is the largest unclosed gap.**
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
