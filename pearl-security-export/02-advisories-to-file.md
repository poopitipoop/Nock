# Paste-ready disclosure submissions — pearl-research-labs/pearl

Submit the four advisories at:
**https://github.com/pearl-research-labs/pearl/security/advisories/new**

File them **separately** — different components and different CWEs, so each
maps to its own CVE. File Advisory 1 first; it is the only one that discloses
key material outright. Advisory 4 is the other fund-affecting one, but it is
conditional on an opt-in feature.

The final section is ordinary hardening and belongs in a public issue, not an
advisory.

---
---

# ADVISORY 1 of 4  —  HIGH

## Form fields

| Field | Value |
|---|---|
| Title | Plaintext wallet seed and passphrase written to a world-readable file during wallet import |
| Ecosystem | Other |
| Package name | `pearl-desktop-wallet` |
| Affected versions | `<= 2.0.4` |
| Patched versions | none |
| Severity | High |
| CVSS vector | `CVSS:3.1/AV:L/AC:L/PR:L/UI:R/S:U/C:H/I:H/A:N` (7.3) |
| CWE | CWE-732: Incorrect Permission Assignment for Critical Resource |
| Secondary CWE | CWE-522: Insufficiently Protected Credentials |

## Description (paste below this line)

### Summary

During wallet creation and import, Pearl Desktop Wallet writes the user's
BIP-39 mnemonic and wallet passphrase in cleartext to
`wallet-setup.json` inside the persistent wallet data directory, using
default file permissions (`0644`) inside a default-permission directory
(`0755`). Any other local user account, or any process running as a
different user, can read the file while it exists.

On the **import** path the file contains the complete mnemonic, so
disclosure results in total, irreversible loss of funds.

### Details

`src/main/services/wallet-process.ts:140-147`:

```js
walletConfigFile = path.join(this.config.dataDir, 'wallet-setup.json');
const walletConfig = {
  seed,                          // full BIP39 mnemonic on import
  privatepassphrase: passphrase,
  bday: isImport ? '1724644369' : undefined,
};
fs.writeFileSync(walletConfigFile, JSON.stringify(walletConfig, null, 2));
```

`fs.writeFileSync` is called with no `mode` argument, so Node uses
`0o666 & ~umask` — `0644` under the usual `umask 022`.

The containing directory is created at
`src/main/services/manager-service.ts:66`:

```js
fs.mkdirSync(walletDataDir, { recursive: true });
```

also with no `mode`, yielding `0755` — world-traversable. The resulting
path `~/.pearl-wallet/wallet-data/<name>/wallet-setup.json` is therefore
world-readable.

### Proof of concept

1. In Pearl Desktop Wallet, choose "Import wallet" and enter an existing
   12-word mnemonic.
2. While the import runs, from a **different** local user account:

   ```sh
   cat /home/<victim>/.pearl-wallet/wallet-data/*/wallet-setup.json
   ```

3. The mnemonic and passphrase are returned in cleartext:

   ```json
   {
     "seed": "<12-word BIP39 mnemonic>",
     "privatepassphrase": "<wallet passphrase>",
     "bday": "1724644369"
   }
   ```

### Impact

Local disclosure of complete key material. An attacker who reads this file
can reconstruct the wallet independently and spend all funds; no further
interaction with the victim's machine is required.

Aggravating factors:

- **Cleanup is not crash-safe.** `fs.unlinkSync` is wired only to the child
  process `close` (`wallet-process.ts:194`) and `error` (`:223`) handlers.
  A SIGKILL of the Electron app, a power loss, or an OS crash mid-import
  leaves the file on disk indefinitely.
- **`unlink` does not wipe.** The plaintext remains recoverable from freed
  disk blocks after deletion.
- **Persistent location.** The file is written to the wallet data directory,
  not a temp directory or tmpfs.
- **Backup amplification.** At `0644` the file is readable by user-level
  backup and sync agents (Time Machine, Dropbox, cloud backup), which may
  replicate the mnemonic off-machine.

### Related — Oyster does not validate permissions on this file

`--createfromfile` is an Oyster daemon flag whose documented contents include
`PrivatePassphrase` and `Seed`. Oyster reads it with a bare `os.ReadFile`
(`wallet/config.go:542-546`) and performs no permission check.

A single check at that boundary would have turned this from a silent key
disclosure into a startup error. Suggested:

```go
if fi, err := os.Stat(filePath); err == nil && fi.Mode()&0o077 != 0 {
    return nil, nil, fmt.Errorf(
        "refusing to read wallet setup file %s: mode %04o is group/world-accessible",
        filePath, fi.Mode().Perm())
}
```

### Remediation

Oyster's own CLI already implements this correctly at
`wallet/cmd/oystercli/createwallet.go:126-146` — a `0700` temp directory, an
explicit `0o600` file mode, and `defer os.RemoveAll`. The desktop app should
adopt the same pattern:

```js
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pearl-setup-')); // 0700
const walletConfigFile = path.join(dir, 'wallet-setup.json');
fs.writeFileSync(walletConfigFile, JSON.stringify(walletConfig), { mode: 0o600 });
// remove the whole directory in a finally block
```

Additionally:

- Register cleanup on `process.on('exit')` and `uncaughtException`, not only
  on child-process events.
- Overwrite the file contents before unlinking.
- Add the `os.Stat` permission check in Oyster as defense in depth.

### Recommendation for users

Anyone who has **imported** a wallet into Pearl Desktop Wallet on a
multi-user or shared machine should treat that mnemonic as potentially
disclosed and migrate funds to a newly generated wallet.

---
---

# ADVISORY 2 of 4  —  MEDIUM

## Form fields

| Field | Value |
|---|---|
| Title | Wallet seed printed to stdout in `--createfromfile` mode is captured by system logs |
| Ecosystem | Go |
| Package name | `github.com/pearl-research-labs/pearl/wallet` |
| Affected versions | `<= 1.2.1` |
| Patched versions | none |
| Severity | Medium |
| CVSS vector | `CVSS:3.1/AV:L/AC:L/PR:L/UI:N/S:U/C:H/I:N/A:N` (6.2) |
| CWE | CWE-532: Insertion of Sensitive Information into Log File |

## Description (paste below this line)

### Summary

When Oyster creates a wallet via `--createfromfile`, it writes the wallet
seed to standard output unconditionally. Because this code path is
machine-driven rather than interactive, stdout is routinely captured by a
supervisor, which writes the mnemonic into system logs.

### Details

`wallet/config.go:552-554`:

```go
// Print the seed so the operator can back it up.
fmt.Println(seedHex)
os.Exit(0)
```

There is no terminal check, no flag gating the output, and no alternative
delivery channel.

### Impact

Under systemd (journald), Docker, Kubernetes, a process supervisor, or any
shell redirection, the mnemonic is written to log storage that is typically:

- readable by users in an administrative or logging group,
- retained and rotated rather than ephemeral,
- shipped to centralized or third-party log aggregation.

This converts a one-time console display into durable plaintext key material
outside the wallet's protection boundary. Any operator or downstream log
consumer can recover the seed and spend the wallet's funds.

The interactive path at `wallet/internal/prompt/prompt.go:258` is
appropriate — it is an explicit, human-facing backup ceremony with warnings
and a confirmation step. The `--createfromfile` path should not assume a
human terminal is attached.

### Remediation

Any of the following:

- Gate the output behind an explicit opt-in flag (e.g. `--show-seed`).
- Write the seed to a caller-specified path created with mode `0600`.
- Emit to stdout only when it is a terminal:

  ```go
  if term.IsTerminal(int(os.Stdout.Fd())) {
      fmt.Println(seedHex)
  } else {
      fmt.Fprintln(os.Stderr,
          "wallet created; re-run with --show-seed on a terminal to display the seed")
  }
  ```

---
---

# ADVISORY 3 of 4  —  MEDIUM

## Form fields

| Field | Value |
|---|---|
| Title | Experimental gRPC wallet-loader service accepts unauthenticated connections |
| Ecosystem | Go |
| Package name | `github.com/pearl-research-labs/pearl/wallet` |
| Affected versions | `<= 1.2.1` |
| Patched versions | none |
| Severity | Medium |
| CVSS vector | `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:L/I:H/A:L` (8.2 if exposed; opt-in configuration) |
| CWE | CWE-306: Missing Authentication for Critical Function |

## Description (paste below this line)

### Summary

The experimental gRPC server exposes `WalletLoaderService` protected by
server-side TLS only. No client authentication of any kind is configured —
no mutual TLS, no credential interceptor. This contrasts with the legacy
RPC server, which refuses to start unless a username and password are set.

### Details

`wallet/rpcserver.go:112-118`:

```go
creds := credentials.NewServerTLSFromCert(&keyPair)
server = grpc.NewServer(grpc.Creds(creds))
rpcserver.StartVersionService(server)
rpcserver.StartWalletLoaderService(server, walletLoader, activeNet)
```

`credentials.NewServerTLSFromCert` supplies server-side TLS. `ClientAuth` is
never set on the `tls.Config` used for the gRPC listener, so client
certificates are not requested or verified, and no
`grpc.UnaryInterceptor` performs credential checking.

Compare the legacy RPC server at `wallet/rpcserver.go:129-130`, which fails
closed:

```go
if cfg.Username == "" || cfg.Password == "" {
    log.Info("Legacy RPC server disabled (requires username and password)")
}
```

### Impact

An operator who sets `--experimentalrpclisten` to a non-loopback address
exposes wallet create/open/close operations to unauthenticated network
callers.

The hazard is the **asymmetry**: an operator who has configured
`--username`/`--password` reasonably assumes that credential requirement
applies to all RPC surfaces. It does not apply to gRPC, and nothing warns
them.

**Mitigating factors.** `ExperimentalRPCListeners` is empty by default, so
the gRPC server is not started in default deployments; reaching this state
requires explicit operator configuration. The design is inherited from
upstream btcwallet, where the service is likewise marked experimental.

### Remediation

Preferred: apply the same credential gate as the legacy server and fail
closed when it is unset.

Alternatively require client certificates:

```go
tlsCfg := &tls.Config{
    Certificates: []tls.Certificate{keyPair},
    ClientAuth:   tls.RequireAndVerifyClientCert,
    ClientCAs:    clientCAPool,
    MinVersion:   tls.VersionTLS12,
}
server = grpc.NewServer(grpc.Creds(credentials.NewTLS(tlsCfg)))
```

At minimum, refuse to bind a non-loopback address when no authentication is
configured, and log a prominent warning.

---
---

# ADVISORY 4 of 4  —  HIGH (conditional)

## Form fields

| Field | Value |
|---|---|
| Title | Seed-only wallet recovery fails to rediscover funds held at opt-in post-quantum (PQ) addresses |
| Ecosystem | Go |
| Package name | `github.com/pearl-research-labs/pearl/wallet` |
| Affected versions | `<= 1.2.1` |
| Patched versions | none |
| Severity | High (conditional — affects only users who opted into PQ addresses) |
| CVSS vector | `CVSS:3.1/AV:L/AC:H/PR:L/UI:R/S:U/C:N/I:N/A:H` (4.4) — availability of funds, not disclosure |
| CWE | CWE-460: Improper Cleanup on Thrown Exception / more precisely CWE-754: Improper Check for Unusual or Exceptional Conditions |

## Description (paste below this line)

### Summary

Address derivation applies the XMSS tapscript commitment based on three
inputs that disagree with one another. As a result, a wallet restored from
its mnemonic alone derives its recovery lookahead window using the **non-PQ**
convention, and therefore does not rediscover funds received at addresses
created with the opt-in PQ convention.

### Details

A BIP-86 taproot output key is
`internalKey + H_TapTweak(internalKey ‖ merkleRoot)·G`. Supplying a tapscript
merkle root yields a different address than the key-only tweak. This is
BIP-341 arithmetic, and `newManagedAddress` implements exactly that
(`waddrmgr/address.go:521-523`):

```go
if tapscriptRoot != nil && len(tapscriptRoot) == 32 {
    tapKey = txscript.ComputeTaprootOutputKey(pubKey, tapscriptRoot)
}
```

Three independent inputs decide whether the root is applied:

1. **Per-address opt-in at creation** — `usePQ := cmd.PQ != nil && *cmd.PQ`
   (`legacyrpc/methods.go:606,635`), default **false**.
2. **The recovery flag** — `expandScopeHorizons` (`wallet/wallet.go:960,990`),
   which builds the lookahead window that discovers funds at not-yet-known
   indexes, passes `includePQTapscript=false`. `recovery.go:87,107`, which
   re-derives only *already known* indexes, passes `true`.
3. **Lock / watch-only state** — `maybeDeriveTapscriptRoot`
   (`waddrmgr/scoped_manager.go:547`) additionally requires
   `key.IsPrivate()`, and `nextAddresses` selects `acctKeyPub` whenever the
   manager is locked or watch-only (`scoped_manager.go:1239-1243`).

The tapscript root **is** persisted per address
(`waddrmgr/db.go:1408-1420`, encrypted), so a wallet with an intact database
spends correctly. That persistence is exactly what is absent during a
seed-only restore.

### Impact

On a fresh restore the wallet database is empty, so `ExternalKeyCount` is 0
and the `recovery.go` loops are no-ops. **Every** address is therefore derived
through `expandScopeHorizons`, using the non-PQ convention. A user who created
PQ addresses and later restores from the mnemonic alone will have the rescan
search for the wrong scripts, and funds at those addresses are not
discovered.

The funds are not cryptographically lost — the seed still controls them — but
the shipped recovery path does not find them, which for most users is
indistinguishable from loss.

### Scope limiter

PQ addresses are opt-in and default to false, and Pearl Desktop Wallet never
sets the flag. Users who never passed `pq=true` are unaffected, and for them
recovery is self-consistent.

### Reproduction (suggested — not dynamically confirmed by the reporter)

1. `getnewaddress "" "bech32" true` — note the PQ address.
2. Send funds to it and let them confirm.
3. Delete the wallet database.
4. Recover the wallet from the same mnemonic and rescan.
5. Expected: funds rediscovered. Observed (predicted from the code path): not
   rediscovered, because the lookahead derived key-only-tweaked addresses.

The reporter analysed control flow only; building the tree requires the cgo
XMSS and Rust ZK-PoW static libraries. The BIP-341 arithmetic is certain, the
code path should be confirmed empirically.

### Related — silent degradation

`maybeDeriveTapscriptRoot` returns `nil, nil` when `includePQTapscript` is
requested but `key.IsPrivate()` is false. A caller asking for a PQ address
while the wallet is locked or watch-only silently receives an ordinary BIP-86
address, with no error and no log line. A watch-only companion wallet
therefore also derives a different address set than the spending wallet.

### Remediation

- Persist the derivation convention per account (or per index) somewhere that
  survives into recovery, and re-read it before the lookahead runs.
- Failing that, have `expandScopeHorizons` derive **both** variants into the
  recovery window. The cost is a doubled window; the alternative is
  undiscoverable funds.
- Make `maybeDeriveTapscriptRoot` return an explicit error rather than `nil`
  when PQ is requested but cannot be honoured — fail closed.
- Add a round-trip regression test: create a PQ address, discard the database,
  recover from seed, assert rediscovery.

---
---

# PUBLIC ISSUE (not an advisory)

Title: **Wallet hardening: signing, KDF parameters, Electron sandbox, unlock timeout**

These are defense-in-depth items with no direct exploit path. They do not
warrant coordinated disclosure and are better tracked in the open.

- **Releases are unsigned and un-notarized.** `electron-builder.json` sets
  `"notarize": false`, there is no Windows signing configuration, and
  `.github/workflows/pearl-desktop-wallet.yml` contains no signing step —
  `SIGNING_GUIDE.md` documents signing as a manual local procedure. Users are
  conditioned to click through Gatekeeper/SmartScreen warnings, which is the
  exact conditioning a malicious lookalike build relies on. Move signing and
  notarization into the release workflow with `release`-scoped secrets.

- **`checksums.txt` is unsigned.** `install.sh` verifies SHA-256, but the
  checksums are fetched from the same GitHub release as the archive and are
  themselves unsigned. This defends against a corrupted download, not against
  a compromised release pipeline. Consider cosign/sigstore or GPG, plus SLSA
  provenance.

- **Electron hardening** (`create-window.ts:18,29`): `sandbox: false`;
  `setWindowOpenHandler` passes any URL to `shell.openExternal` with no scheme
  allowlist; no `will-navigate` guard. (`contextIsolation` and
  `nodeIntegration` are correctly set.)

- **RPC credentials passed as argv** (`wallet-process.ts:90-91`):
  `--username=`/`--password=` are visible to other local users via `ps` and
  `/proc/<pid>/cmdline`. Credentials are well generated (ephemeral
  `randomBytes(16)`/`randomBytes(32)`); only the transport is at fault. Use an
  environment variable, stdin, or a `0600` config file.

- **scrypt parameters** (`wallet/snacl/snacl.go:38-40`): `N=16384 (2^14)`,
  `r=8`, `p=1` — the original 2009 interactive parameters, soft against modern
  offline GPU brute-force of a stolen `wallet.db`. Raise `N` for new wallets
  (parameters are already stored per-wallet, so existing wallets still open),
  or migrate to Argon2id.

- **`walletpassphrase` with `timeout=0` never relocks**
  (`legacyrpc/methods.go:1837-1842`): `unlockAfter` stays nil, so the wallet
  remains unlocked indefinitely. Enforce a maximum and reject `0`.

- **BIP-322 deviates from the specification** (`legacyrpc/bip322.go:48-59`):
  the message is placed in an `OP_RETURN` output of `to_sign` rather than in
  `to_spend`'s scriptSig as the BIP-340 tagged hash. Not a security defect —
  the sighash still commits to the message and the null-outpoint construction
  preserves replay safety — but signatures will not verify against conforming
  BIP-322 implementations, and `NullDataScript` caps messages at 80 bytes.
  Either conform or rename the API.

- **Dead deprecated PRNG seeding** (`wallet/wallet/rand.go`):
  `rand.Seed(time.Now().Unix())` in `init()`. Deprecated since Go 1.20; the
  module targets Go 1.26. Only consumer of the global source is the
  coin-selection shuffle. Delete the file.

- **Stale doc comment** (`node/btcutil/hdkeychain/extendedkey.go:734`): says
  "recommended length is 32 (256 bits)" while the constant is `16`. Code is
  correct; comment is not.

- **Secrets not zeroed in the setup path** (`wallet/walletsetup.go:157-193`):
  `seed` and `privPass` are left to the GC. `zero.Bytes` is already used
  elsewhere in the tree.

- **Suggested:** run `govulncheck ./...` in CI. Dependencies look current
  (`x/crypto v0.53.0`, `x/net v0.56.0`, `grpc v1.82.0`) but were not
  machine-checked during this review.
