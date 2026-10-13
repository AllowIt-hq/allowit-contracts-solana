# AllowIt native Solana contracts

Shared Solana custody and Rust policy programs enforce owner-approved spending through per-vault PDA state and classic SPL Token accounts. The separate [PaySH program](docs/paysh.md) supports bounded payments and swaps.

The owner funds, approves, tunes, withdraws and closes a vault. Payments require the executor and server authority to sign. Custody checks balance, asset, recipient, action/day limits, nonce, revision, instance and expiry. The server evaluates semantic rules before signing.

Funding preserves spending counters. Limits use UTC days and six-decimal units: `1_000_000` units = one token. The `daily_limit` ceiling is `50_000_000`. Zero pauses spending. Changing policy clears approval.

## Build and test

Use Rust 1.98.0, Agave 4.3.0 and platform-tools 1.57.

```sh
python3 scripts/check-policy.py
cargo test --workspace --locked
cargo build-sbf --manifest-path programs/policy/Cargo.toml --arch v3 --optimize-size --tools-version v1.57 -- --locked
cargo build-sbf --manifest-path programs/vault/Cargo.toml --arch v3 --optimize-size --tools-version v1.57 -- --locked
cargo build-sbf --manifest-path programs/paysh/Cargo.toml --arch v3 --optimize-size --tools-version v1.57 -- --locked
bash scripts/build-whirlpool-test.sh
SBF_OUT_DIR="$PWD/target/deploy" cargo test --manifest-path sbf-tests/Cargo.toml --locked
python3 scripts/artifacts.py
```

Mollusk tests execute compiled programs and SPL Token transfers. The Whirlpool fixture uses pinned Orca source. See [runtime tests](sbf-tests/tests) and [fixture build](scripts/build-whirlpool-test.sh).

## Deploy to Solana Testnet

Build first. Use fresh test keys outside the checkout. Check that the selected cluster supports sBPFv3. See [Solana deployment instructions](https://solana.com/docs/programs/deploying).

```sh
cluster=testnet
key_dir="$HOME/.local/share/allowit-solana-testnet"
mkdir -p "$key_dir"
chmod 700 "$key_dir"
solana-keygen new --no-bip39-passphrase --silent --outfile "$key_dir/deployer.json"
solana-keygen new --no-bip39-passphrase --silent --outfile "$key_dir/policy.json"
solana-keygen new --no-bip39-passphrase --silent --outfile "$key_dir/vault.json"
solana-keygen new --no-bip39-passphrase --silent --outfile "$key_dir/paysh.json"
solana --url "$cluster" genesis-hash
solana --url "$cluster" --keypair "$key_dir/deployer.json" airdrop 5
solana --url "$cluster" --keypair "$key_dir/deployer.json" balance

policy_size=$(wc -c < target/deploy/allowit_policy.so | tr -d ' ')
solana --url "$cluster" --keypair "$key_dir/deployer.json" program deploy \
  target/deploy/allowit_policy.so --program-id "$key_dir/policy.json" \
  --max-len "$policy_size" --final
solana --url "$cluster" --keypair "$key_dir/deployer.json" program deploy \
  target/deploy/allowit_vault.so --program-id "$key_dir/vault.json"
solana --url "$cluster" --keypair "$key_dir/deployer.json" program deploy \
  target/deploy/allowit_paysh.so --program-id "$key_dir/paysh.json"

policy_id=$(solana-keygen pubkey "$key_dir/policy.json")
solana --url "$cluster" program show "$policy_id"
solana --url "$cluster" program dump "$policy_id" /tmp/allowit-policy-deployed.so
cmp target/deploy/allowit_policy.so /tmp/allowit-policy-deployed.so
```

`cluster=devnet` selects Solana Devnet with the same procedure. Faucet limits and storage rent can require more test SOL. Policy deployment is permanent: `--final` removes its upgrade authority. Exact allocation makes its deployed artifact hash match the built ELF.

Vault and PaySH retain the deployer's upgrade authority in this example. That authority can replace custody code. Record program/ProgramData IDs, genesis hash, artifact hashes and authorities. Check each deployment before configuring the SDK or initializing vaults. PaySH also needs the selected cluster's token accounts, treasury and Whirlpool pool described in [its deployment requirements](docs/paysh.md).

No verified AllowIt mainnet deployment address is recorded.

## ABI and policy

[ABI 2](crates/interface/src/lib.rs) uses Borsh, a 352-byte state allocation and PDA seeds `[b"allowit-vault-v2", owner, vault_id]`. ABI 1 state is rejected. Requests bind the exact instance and server commitment. Expiry cannot exceed chain time by 300 seconds.

Account order (`w` writable, `s` signer):

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

Custody checks immutable policy ProgramData and deployed artifact identity before invocation. See [vault implementation](programs/vault/src/lib.rs) for instruction layouts and checks.

[Policy source](policy/policy.rs) calls `require_approval` and `enforce_daily_limit` from [policy_api.rs](policy/policy_api.rs). Custody commits spending and token movement atomically. The shared source bundle and executable hash are separate identities. Pin both with chain, ABI, program and authorities. Check sibling source copies with `python3 scripts/check-policy.py --peer ../OTHER_REPO`.

Save signed transaction bytes before broadcast. Check the original transaction after an uncertain result before signing a replacement. `python3 scripts/artifacts.py --output PATH` saves a build manifest.
