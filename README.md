# AllowIt native Solana contracts

Shared Solana custody and Rust policy programs enforce owner-approved spending through per-vault PDA state and classic SPL Token accounts. The separate [PaySH program](docs/paysh.md) supports bounded payments and swaps.

The [Solana vendor crate](crates/solana-vendor/README.md) provides host adapters for the SDK client, including retained PaySH operation reconciliation.

The owner funds, approves, tunes, withdraws and closes a vault. Payments require the executor and server authority to sign. Custody checks balance, asset, recipient, action/day limits, nonce, revision, instance and expiry. The server evaluates semantic rules before signing.

Funding preserves spending counters. Limits use UTC days and six-decimal units: `1_000_000` units = one token. The `daily_limit` ceiling is `50_000_000`. Zero pauses spending. Changing policy clears approval.

[Wallet signing and contract custody](docs/architecture.md) describes policy, signing and owner-control boundaries.

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

## Deploy native vaults to Solana Devnet

Build the exact contract revision and artifacts pinned by the consuming SDK first; the current branch is not necessarily that release. Use fresh test keys outside the checkout. Check that the selected cluster supports sBPFv3. See [Solana deployment instructions](https://solana.com/docs/programs/deploying).

The current `native-vault-v2` SDK verifies the source bundle, exact deployed artifact bytes and absence of an upgrade authority for **both** programs. Verify the pinned hashes before using `--final`: it permanently removes the ability to upgrade that program. A deployment with another artifact or a remaining upgrade authority is not compatible with this profile.

```sh
cluster=devnet
key_dir="$HOME/.local/share/allowit-solana-devnet"
mkdir -p "$key_dir"
chmod 700 "$key_dir"
solana-keygen new --no-bip39-passphrase --silent --outfile "$key_dir/deployer.json"
solana-keygen new --no-bip39-passphrase --silent --outfile "$key_dir/policy.json"
solana-keygen new --no-bip39-passphrase --silent --outfile "$key_dir/vault.json"
solana --url "$cluster" genesis-hash
solana --url "$cluster" --keypair "$key_dir/deployer.json" airdrop 5
solana --url "$cluster" --keypair "$key_dir/deployer.json" balance

policy_size=$(wc -c < target/deploy/allowit_policy.so | tr -d ' ')
vault_size=$(wc -c < target/deploy/allowit_vault.so | tr -d ' ')
solana --url "$cluster" --keypair "$key_dir/deployer.json" program deploy \
  target/deploy/allowit_policy.so --program-id "$key_dir/policy.json" \
  --max-len "$policy_size" --final
solana --url "$cluster" --keypair "$key_dir/deployer.json" program deploy \
  target/deploy/allowit_vault.so --program-id "$key_dir/vault.json" \
  --max-len "$vault_size" --final

policy_id=$(solana-keygen pubkey "$key_dir/policy.json")
vault_id=$(solana-keygen pubkey "$key_dir/vault.json")
dump_dir=$(mktemp -d)
solana --url "$cluster" program show "$policy_id"
solana --url "$cluster" program show "$vault_id"
solana --url "$cluster" program dump "$policy_id" "$dump_dir/policy.so"
solana --url "$cluster" program dump "$vault_id" "$dump_dir/vault.so"
cmp target/deploy/allowit_policy.so "$dump_dir/policy.so"
cmp target/deploy/allowit_vault.so "$dump_dir/vault.so"
```

`cluster=testnet` selects Solana Testnet with the same procedure and a separate key directory. Moving between clusters means a new deployment, new vault state and a new owner-approved setup; it does not migrate balances or existing approvals. Faucet limits and storage rent can require more test SOL. Exact allocation avoids trailing capacity bytes changing the deployed artifact hash.

Record both program/ProgramData IDs, genesis hash, artifact hashes and absent upgrade authorities. Configure the SDK/backend with that cluster's mint, executor and server authority, and verify the deployment before initializing vaults. Native owner setup can combine creation, initial funding and approval into one owner-signed transaction; subsequent execution uses executor and server-authority signatures. Revocation is a separate owner-signed transaction.

The separate PaySH profile is not provisioned by these native-vault commands. It needs its own compatible program and the selected cluster's token accounts, treasury and Whirlpool pool described in [its deployment requirements](docs/paysh.md). A successful native token transfer alone is not proof of a paid API response.

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
