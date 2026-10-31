# Native correspondence check

This folder holds the native policy correspondence check for the Solana code. The Allowit verification space, with the obligation ledger, the core Lean model, refinement, adapter, trace, refusal and isolation evidence, stays in [allowit-sdk `verification/`](https://github.com/allowit-hq/allowit-sdk/tree/main/verification).

## Contents

- [`check_native.py`](check_native.py) runs the frozen native `policy.rs` and `policy_api.rs` with host `rustc` and compares every result with the Lean executable model in [`lean/NativeDaily.lean`](lean/NativeDaily.lean). It also checks that nine policy mutations change at least one result.
- [`native-source-lock.json`](native-source-lock.json) binds the check to the Solana and Stellar policy sources by SHA-256 and names their checkout paths.
- [`native-correspondence.json`](native-correspondence.json) is the recorded receipt: 2,907 cases, the source and model digests, the proof log digest and the digests of every verification dependency.
- [`lean/`](lean/) holds the Lean inputs that the receipt binds. `check.py` audits the theorems in `AllowIt.lean` and `NativeDaily.lean`. `proof-check.txt` is the recorded proof log.

All of these files are hash-bound and byte-identical to allowit-sdk `ede4e9f`. Never re-hash a lock or receipt to make a check pass.

## Scope

The receipt covers the frozen `policy.rs` SHA-256 `eceb1d4f55c93ef7921f47f3ca0d35bc589c3c6a1b0f29ae63f5fb9f9ff37da8` with `policy_api.rs` SHA-256 `70b783aac1a7d7016ea967cc6d873a91917f6ce3f38b030551f4658923b7793d`. The current `policy/` sources have other digests, so a full run against them reports a stale source binding. Source correspondence does not prove the adapter, the compiled program or a deployment.

The lock names the targets `../AllowIt-contracts-solana/policy` and `../AllowIt-contracts-stellar/policy`, relative to the repository root. A full run needs both repositories checked out beside this one under those names.

## Run

The unit tests need only Python 3:

```sh
python3 -m unittest discover -s verification/tests
```

The full check needs Lean 4.11.0 (`lean/lean-toolchain`) and Rust:

```sh
LEAN_BIN=/absolute/path/to/lean python3 verification/check_native.py
```
