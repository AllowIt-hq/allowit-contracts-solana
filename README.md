# AllowIt native Solana contracts

Hackathon MVP: one shared custody program, a native Rust policy program per version, and a PDA state account plus classic SPL Token account per vault. The policy uses a minimal Pinocchio 0.11.2 adapter; custody uses the native SDK. No executable is deployed per wallet.

Funding changes only the token balance; it never recompiles policy or resets spending. Daily limits use UTC calendar days and six-decimal policy units (one token = `1_000_000`). The owner can tune the explicitly declared `daily_limit` within `0..=50_000_000`; zero pauses spending. Custody independently enforces that compiled ceiling. No generic setter exists for other parameters.

Owner approval is standing approval. Each payment requires the designated executor and trusted server authority to sign the same transaction. Custody checks balance, per-action and UTC daily limits, nonce, revision, instance slot, expiry, mint and recipient accounts. Owner configuration preserves counters and invalidates old approvals by advancing revision. Funding changes only the available token balance, with no lifetime spending cap. Funding, withdrawal and closure require the owner. The server evaluates any approved semantic rules before signing. Its signature is trusted authorization, not a cryptographic proof of semantic correctness.

Setup combines vault ATA creation, Initialize, Deposit and Approve in one transaction, with the owner as its only signer. Shared programs must exist beforehand. A failed instruction rolls back the entire setup. Wallet sign-in and connection remain separate authentication operations. Owner withdrawal or closure requires a later transaction.

The separate bounded PaySH program has its own custody and request ABI. See [Bounded PaySH execution profile](docs/paysh.md).


The sibling repositories compile byte-identical `policy/policy.rs` and `policy/policy_api.rs`. Their bundle identity is SHA-256 of `allowit-policy-source-v1` followed by NUL, then each file in that order: filename, NUL, eight-byte little-endian length and contents. The build fails on a stale generated identity. This source identity differs from the executable artifact hash. Clients must pin the approved artifact, chain, ABI, vault/program/factory, asset, owner, executor, server authority, revision and `instance_slot`; a reported source hash alone is not proof of deployed code. Check copies using `python3 scripts/check-policy.py --peer ../OTHER_REPO`.

Persist signed transaction bytes and hash/signature before broadcast. Query that transaction after uncertain results; do not automatically re-sign deposits. Refresh state before retrying a finalized failed or expired request. Finalized token movement proves settlement, separately from service delivery.

## Build and test

Pinned versions: Rust 1.98.0, Agave 4.3.0, platform-tools 1.57.

```sh
python3 scripts/check-policy.py
cargo test --workspace --locked
cargo build-sbf --manifest-path programs/policy/Cargo.toml --arch v3 --optimize-size --tools-version v1.57 -- --locked
cargo build-sbf --manifest-path programs/vault/Cargo.toml --arch v3 --optimize-size --tools-version v1.57 -- --locked
SBF_OUT_DIR="$PWD/target/deploy" cargo test --manifest-path sbf-tests/Cargo.toml --locked
python3 scripts/artifacts.py
```

Mollusk tests execute compiled policy/custody, System Program initialization and real SPL Token CPIs. The minimal adapter is checked against Borsh encoding and kernel decisions, including malformed lengths/bools and unexpected accounts.

## ABI version 2

`crates/interface/src/lib.rs` owns the Borsh layout: one-byte enum tags, little-endian integers and raw 32-byte keys/hashes. State has a 347-byte struct prefix in a 352-byte allocation. ABI 1 and its 320-byte state are rejected. PDA seeds: `[b"allowit-vault-v2", owner, vault_id]`. The state appends `authority: [u8;32]`, `action_limit: u64` and `instance_slot: u64` after `approved`. Initialization records the current chain slot. Close requires a later slot. Any subsequent recreation therefore has a newer instance identity, and cannot accept old execution approvals.

Account order (`w` writable, `s` signer, others readonly):

| Instruction | Accounts |
| --- | --- |
| Initialize | vault(w), owner(w,s), mint, vault token, policy, executor, System Program, policy ProgramData, authority |
| Deposit | vault(w), owner(s), owner source token(w), vault token(w), mint, SPL Token Program |
| Approve | vault(w), owner(s), policy, policy ProgramData (only needed for true) |
| SetDailyLimit / SetPolicy | vault(w), owner(s), policy, policy ProgramData |
| Transfer | vault(w), executor(s), vault token(w), recipient token(w), mint, SPL Token Program, policy, policy ProgramData, authority(s) |
| Withdraw | vault(w), owner(s), vault token(w), owner destination token(w), mint, SPL Token Program |
| SetActionLimit | vault(w), owner(s) |
| Close | vault(w), owner(w,s), vault token(w), owner destination token(w), mint, SPL Token Program |
| Unsupported | no accounts required |

Initialize and SetPolicy carry `policy_source` and `policy_artifact`. Artifact identity hashes the complete deployed ELF allocation after the 45-byte loader header. Exact-size allocation makes this equal the `.so` hash; padded deployments require hashing their actual deployed bytes. Custody checks loader-v3 ownership, ProgramData linkage/canonical address, absent upgrade authority and matching artifact hash before invocation. Policy receives no accounts or vault signer. Its response is 77 bytes: ABI, u32 error, binding, source bundle, next spend. Custody offsets policy errors by 1000. Unsupported instructions return custody error 100 (`UnsupportedMethod`). Binding echoes the request digest; it is not proof of evaluation.

Existing enum tags 0–7 retain their operation names. Initialize appends `action_limit: u64`. Transfer appends `expires_at: u64`, `commitment: [u8;32]` and `expected_instance_slot: u64` after amount, nonce and revision. The nonzero commitment identifies the server's exact assessed operation and request. Both transaction signatures bind this commitment and every instruction/account byte. Expiry uses Unix seconds and cannot exceed the observed chain time by 300 seconds. Tag 8 is SetActionLimit(value, expected_revision). Tag 9 is Close(expected_revision). Action and daily limits use six-decimal base units: one token equals `1_000_000` units. The `0..=50_000_000` ceiling therefore permits at most 50 tokens. Zero pauses execution.

Close transfers the entire balance to an owner-controlled token account, closes the empty vault token account, and drains/deallocates the vault state. Both accounts refund rent to the signing owner. It requires neither the executor nor the policy program. Clients retain operation history outside the closed accounts. Close includes `expected_instance_slot` after its revision and checks the exact instance. A state account can close only after its creation slot.

## Deployment, authorities and costs

Use a fresh test key and an explicitly selected test cluster. Deploy policy with `--final` and exact-size `--max-len`, then custody. Record program/ProgramData IDs, hashes, source bundle, toolchains, genesis hash, authority, parameters and initialization transaction. Use `solana program dump` to verify deployed bytes. Validate current cluster features for sBPFv3.

Immutable policy code cannot be upgraded, closed or refunded. ABI 2 requires fresh program IDs and matching SDK artifact pins. Deploy new policy versions under new IDs and switch with owner-authorized SetPolicy, which clears approval. Custody's upgrade authority is a separate trust root: if retained, its holder can replace custody logic and control funds. Production requires an explicit authority/governance choice. Close affects only a mandate's state and token account. Shared executable programs remain separate.

Query live `getMinimumBalanceForRentExemption` for storage; upload buffers can temporarily require additional capital. Loader-v3 closure refunds ProgramData, leaves the tiny program-address account and consumes fees. Shared program storage is paid once per deployment, while each vault pays for small state/token accounts.

## Policy entrypoint and system functions

`policy/policy.rs` contains one generated function, `execute`. It calls `require_approval` and `enforce_daily_limit` from `policy/policy_api.rs`. Approval, daily rollover, clock checks, overflow checks and the compiled parameter ceiling belong to this standard library. Custody commits the returned next daily spend atomically with token movement. Both files remain byte-identical to the existing portable source bundle. Solana-specific authority, instance, custody and per-action checks reside in the vault. The Solana adapter returns ABI 2 with unchanged numeric request encoding. This change does not implement a Stellar vault or change its adapter ABI.

`python3 scripts/artifacts.py` prints a build manifest. Use `--output PATH` to save it.
