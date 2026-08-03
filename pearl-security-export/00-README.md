# Pearl / Oyster security review — export bundle

Generated 2026-08-03. Subject: `github.com/pearl-research-labs/pearl` at
commit `83cf4cb` (master), plus full history (6,778 commits) and all 8 release
tags.

**Method:** source review and static analysis only. Nothing was built, executed,
fuzzed, or tested against any live system.

---

## What's in here

| File | What it is | Who it's for |
|---|---|---|
| `01-security-review.md` | The full review — 23 findings, ~30 verified-clean areas, scope and limitations | You; the Pearl maintainers |
| `02-advisories-to-file.md` | 4 paste-ready GitHub private-advisory bodies (form fields + verbatim text) and 1 public hardening issue | You, to submit |
| `03-HANDOFF-for-claude.md` | Continuation brief: exact commands run, techniques that worked, what wasn't covered, limitations, prioritised next steps | An AI assistant picking this up on another machine |

---

## The short version

**The question you asked — is Oyster affected by the Coldcard weak-RNG
vulnerability? — is answered: no, and it never was.** Seed generation has used
`crypto/rand` since the very first commit, byte-identical across all 5 commits
that ever touched the file and all 8 release tags. Signing nonces are RFC6979
deterministic, so the other classic key-recovery class is absent too.

The review then widened across the whole repository and found 23 items.

### What actually changes your exposure

1. **PDW-1 (High)** — Pearl Desktop Wallet writes your **plaintext mnemonic and
   passphrase** to a world-readable file (`0644` in a `0755` directory) during
   wallet **import**. Any other local user can read it. Cleanup is not
   crash-safe, and `unlink` doesn't wipe.
   → **Don't import a wallet into the desktop app on a shared machine until
   this is fixed.** Creating a new wallet writes only the passphrase, not the
   seed. Oyster's own CLI does this correctly.

2. **The PRL name collision** — "Oyster Pearl (PRL)" was a **2018 ERC-20 exit
   scam** whose founder minted ~3M tokens, dumped them, and was later arrested.
   Etherscan still flags that contract. It is unrelated to Pearl Research Labs.
   → **Verify what chain and what token you're actually holding.** A separate
   `PearlBridgeXYZ/pearlwallet` browser wallet also exists and was not reviewed.

3. **OYS-11 (High, conditional)** — if you opted into post-quantum addresses
   (`pq=true`, not the default, and the desktop wallet never sets it), a
   seed-only restore may not rediscover those funds. Analysis-only; not
   reproduced.

Everything else is either lower severity, latent, or affects mining
infrastructure rather than wallet users.

---

## If you want to report this

Pearl's policy is **private disclosure**, not public issues:
<https://github.com/pearl-research-labs/pearl/security/advisories/new>

`02-advisories-to-file.md` has four separate submissions ready to paste, with
form-field values (affected versions, CVSS vectors, CWE IDs). File Advisory 1
first — it's the only one that discloses key material outright.

**Nothing has been reported to Pearl Research Labs.** That's your call to make
under your own identity.

---

## Two things this review could not do

1. **No dependency reachability scan.** The review environment's egress policy
   blocked `vuln.go.dev` and `api.osv.dev`. A manual advisory cross-check was
   done instead and found DEP-1 (`grpc v1.82.0`), but that's version matching,
   not `govulncheck`'s reachability analysis. Running `govulncheck ./...`
   anywhere with network access is the cheapest remaining win.

2. **ZK circuit soundness was not assessed** and can't be by reading source.
   Note that the two audit reports bundled in `plonky2/audits/` cover
   **upstream's unmodified library** — not Pearl's fork, which modifies the FRI
   and PLONK verifiers, and not Pearl's own circuits. That gap needs a
   specialist cryptographic audit.

Absence of findings in an area is not proof that none exist.
