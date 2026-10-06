# AllowIt native Solana contracts

Private hackathon MVP: one shared custody program, a native Rust policy program per version, and a PDA state account plus classic SPL Token account per vault. The policy uses a minimal Pinocchio 0.11.2 adapter; custody uses the native SDK. No executable is deployed per wallet.

Funding changes only the token balance; it never recompiles policy or resets spending. Daily limits use UTC calendar days and six-decimal policy units (one token = `1_000_000`). The owner can tune the explicitly declared `daily_limit` within `0..=50_000_000`; zero pauses spending. Custody independently enforces that compiled ceiling. No generic setter exists for other parameters.

Owner approval is standing approval, while each transfer requires the designated executor's signature, native Rust evaluation, asset checks, daily accounting, nonce and current revision. Tuning preserves standing approval; switching policy clears it. Funding, tuning, approval/revocation and policy switches preserve counters. Owner withdrawal and revocation work independently of policy execution. The cap is per vault. Delegation, semantic evaluation, per-action permits, optimizer authority and PaySH are not implemented. The explicit Unsupported method returns error 100; unknown selectors use the chain's invalid-instruction/host errors.

The sibling repositories compile byte-identical `policy/policy.rs` and `policy/policy_api.rs`. Their bundle identity is SHA-256 of `allowit-policy-source-v1` followed by NUL, then each file in that order: filename, NUL, eight-byte little-endian length and contents. The build fails on a stale generated identity. This source identity differs from the executable artifact hash. Clients must pin the approved artifact, chain, ABI, vault/program/factory, asset, owner, executor and revision; a reported source hash alone is not proof of deployed code. Check copies using `python3 scripts/check-policy.py --peer ../OTHER_REPO`.

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

Mollusk tests execute compiled policy/custody, System Program initialization and real SPL Token CPIs. The minimal adapter is checked against Borsh encoding and kernel decisions, including malformed lengths/bools and unexpected accounts. Deployment and acceptance evidence is maintained in the main project wiki and applies only to the recorded release. VM tests do not establish frontend/native-wallet acceptance or a production security audit.

## ABI version 1

`crates/interface/src/lib.rs` owns the Borsh layout: one-byte enum tags, little-endian integers and raw 32-byte keys/hashes. State occupies 320 bytes; decode its struct prefix and ignore trailing zero padding. PDA seeds: `[b"allowit-vault-v1", owner, vault_id]`.

Account order (`w` writable, `s` signer, others readonly):

| Instruction | Accounts |
| --- | --- |
| Initialize | vault(w), owner(w,s), mint, vault token, policy, executor, System Program, policy ProgramData |
| Deposit | vault(w), funder(s), source token(w), vault token(w), mint, SPL Token Program |
| Approve | vault(w), owner(s), policy, policy ProgramData (only needed for true) |
| SetDailyLimit / SetPolicy | vault(w), owner(s), policy, policy ProgramData |
| Transfer | vault(w), executor(s), vault token(w), recipient token(w), mint, SPL Token Program, policy, policy ProgramData |
| Withdraw | vault(w), owner(s), vault token(w), destination token(w), mint, SPL Token Program |
| Unsupported | no accounts required |

Initialize and SetPolicy carry `policy_source` and `policy_artifact`. Artifact identity hashes the complete deployed ELF allocation after the 45-byte loader header. Exact-size allocation makes this equal the `.so` hash; padded deployments require hashing their actual deployed bytes. Custody checks loader-v3 ownership, ProgramData linkage/canonical address, absent upgrade authority and matching artifact hash before invocation. Policy receives no accounts or vault signer. Its response is 77 bytes: ABI, u32 error, binding, source bundle, next spend. Custody offsets policy errors by 1000. Binding echoes the request digest; it is not proof of evaluation.

## Deployment, authorities and costs

Use a fresh test key and an explicitly selected test cluster. Deploy policy with `--final` and exact-size `--max-len`, then custody. Record program/ProgramData IDs, hashes, source bundle, toolchains, genesis hash, authority, parameters and initialization transaction. Use `solana program dump` to verify deployed bytes. Validate current cluster features for sBPFv3.

Immutable policy code cannot be upgraded, closed or refunded. Deploy new policy versions under new IDs and switch with owner-authorized SetPolicy, which clears approval. Custody's upgrade authority is a separate trust root: if retained, its holder can replace custody logic and control funds. Production requires an explicit authority/governance choice. Do not close shared custody while vaults hold funds. A vault/token-account closure instruction is not implemented.

Query live `getMinimumBalanceForRentExemption` for storage; upload buffers can temporarily require additional capital. Loader-v3 closure refunds ProgramData, leaves the tiny program-address account and consumes fees. Shared program storage is paid once per deployment, while each vault pays for small state/token accounts.

## Policy entrypoint and system functions

`policy/policy.rs` contains one generated function, `execute`. It calls `require_approval` and `enforce_daily_limit` from `policy/policy_api.rs`. Approval, daily rollover, clock checks, overflow checks and the compiled parameter ceiling belong to this standard library. Custody commits the returned next daily spend atomically with token movement. Both files are included in the pinned source bundle; clients display the literal compiled policy source and can show the exact system-function implementation separately. Chain adapters retain their version-1 ABI.

Review reports, deployment evidence and retained artifact manifests are in the [main project artifacts](https://github.com/ackrate/ackrate-project/tree/main/instance/artifacts/109-contract-and-branch-audit/AllowIt-contracts-solana). Write new task outputs there under the numbered source resource. `python3 scripts/artifacts.py` prints a build manifest to stdout; pass `--output` with an explicit main-project artifact path to retain it.
