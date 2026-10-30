# PaySH: the bounded execution profile

PaySH is how an Allowit agent pays for a service. The owner sets up a policy with hard budgets and an allowlist of services. An evaluator signs each request. The [`allowit-paysh` program](../programs/paysh/src/lib.rs) then re-checks everything the owner configured before it moves a single lamport.

It is a separate, upgradeable program. It changes nothing about the immutable policy helper or the native vault ABI. Because its upgrade authority can replace the program, that authority controls the safety of every PaySH custody account; keep it under explicitly chosen owner governance and record it for every production deployment.

## What the owner controls

- **Initialize** needs the owner's signature and binds an immutable `Config` to the policy instance: evaluator key, treasury, USDC mint, vendor account, pool, every budget and the `service_allowlist_root`.
- **Pause** and **Withdraw** need the owner's signature and ignore the evaluator entirely.
- **Execute** can be submitted by anyone carrying a valid evaluator signature, unless `owner_only` is set, in which case only the owner may submit.
- **Funding** is ordinary: a System Program transfer into the SOL custody PDA, plus classic SPL Token transfers into the policy's token accounts.

## The request protocol

[`crates/paysh-interface`](../crates/paysh-interface/src/lib.rs) is the canonical Borsh protocol, version 2. A client must carry an identical copy, and a release must check schema equality before it ships.

The evaluator signs `allowit-paysh-request-v2` followed by the SHA-256 of the Borsh-encoded `Request`. The transaction places a standard, self-contained, single-signature Ed25519 precompile instruction immediately before `Execute`, with the public key at offset 16, the signature at offset 48, the message at offset 112 and every instruction index set to `u16::MAX`. The program reads the real instructions sysvar, checks the previous program ID, the exact layout, the evaluator key and the whole message. `Execute` cannot be reached through an unrelated CPI wrapper.

A request binds: genesis hash, program, policy, owner, module digest, operation ID, a random replay nonce, the gateway challenge commitment, the semantic-evidence commitment, signing slot and time, expiry slot and time, the exact service fee and the exact action. The digests authenticate no evidence by themselves; the evaluator's signature attests that the off-chain checks ran. Budgets, the caller rule, expiry, pause and the exact effects are enforced on-chain regardless.

Solana exposes no genesis-hash sysvar, so the genesis hash is part of the immutable owner configuration. Clients verify it through their RPC before initializing or signing.

### Service allowlist

At initialization the owner commits a `service_allowlist_root`. Each request carries a service hash and up to three sibling hashes that prove membership in a list of at most eight services, scoped to the configured genesis hash, mint and vendor. An all-zero root denies every service. Membership is verified before any fee, receipt, budget or token effect. See [`allowlist.rs`](../crates/paysh-interface/src/allowlist.rs).

## What a successful call does

1. Transfers the configured SOL service fee from the policy's SOL custody to the treasury.
2. Either pays exact USDC to the single approved vendor, or swaps exact SOL input into policy-owned USDC.
3. Writes a receipt and updates the period budget.

Receipt and budget account rent is paid by the transaction submitter, never from policy custody. If anything fails, every effect rolls back; the submitter still pays the network fee.

### Budgets

Service fees and swap input count against a gross period SOL budget and a lifetime allocation. USDC, swap input and service fees each have their own period cap on top. The period is derived from the signed original timestamp, which may be at most 60 seconds and 180 slots old. A request executed late is charged to its original bucket. Nothing is reserved ahead of time.

### Replay protection and nonces

Receipts are never closed. On success, `(policy, nonce)` is consumed forever. On failure it stays reusable, and the evaluator's signature remains public and executable until it expires.

For one logical operation the host persists one nonce before signing and reuses it for every replacement quote or request. A fresh nonce authorizes a fresh debit, even with the same operation ID. Never mint a new nonce just because a transaction failed; let the old and new requests compete for the same permanent receipt, and recover any unknown settlement before issuing a replacement. The receipt stores the SHA-256 of the signed message, the operation ID and the original signing timestamp. Gateway verification matches the exact successful `Execute` and its inner payment CPI, and consumes the payment once.

## The swap adapter

The direct swap path admits classic SPL Token, static-fee Orca Whirlpools only, pinned to the owner-configured program, pool, vaults, oracle, WSOL mint A and USDC mint B. It validates the pool discriminator, canonical PDA, token ownership and the configured LP/protocol fee cap, and requires three fixed `TickArray` accounts that belong to the pool, have canonical PDAs, and cover the current array plus two contiguous decreasing arrays. The request binds those keys and an A-to-B sqrt price limit.

Hard loss bounds come from the native input and allocation limits, a configured minimum USDC per SOL and the exact minimum output. A successful swap must consume all newly wrapped SOL and leave the WSOL balance that existed before `Execute` untouched, including earlier donations recognized by `SyncNative`. A partial fill at the price limit fails with `Slippage` even when minimum output is met, and the whole transaction rolls back. After such a failure the backend fetches a fresh quote and keeps the original operation and nonce. Pool trading fees apply; the adapter adds no host, aggregator or referral fee.

Adaptive fees, dynamic tick arrays, Token-2022, B-to-A swaps and intermediary aggregators need a different profile.

## Owner recovery

`Withdraw` is owner-only. It returns all remaining SOL custody, unwraps WSOL to the owner and transfers accessible USDC to the owner's canonical associated token account. USDC custody closes and refunds its rent when empty; WSOL custody closes independently. If the USDC vault is frozen, or the canonical destination is missing or frozen, USDC custody is preserved for a later withdrawal while SOL and WSOL still return. Existing destination accounts must have the exact mint and owner.

Withdrawal works while paused or expired. It permanently marks the policy as a paused version-2 tombstone that cannot be resumed or reinitialized. Policy, receipt and budget accounts remain so replay protection and history survive. The owner may withdraw again after thawing USDC or creating the canonical destination, and later donations to closed custody accounts are still recoverable.

## Accounts and seeds

| Instruction | Ordered accounts |
| --- | --- |
| Initialize | policy(w), owner(w,s), vault USDC, vault WSOL, USDC mint, vendor USDC, treasury, System Program, SOL custody(w), pool program, pool state, pool WSOL vault, pool USDC vault, oracle |
| Execute PayUsdc | policy(w), payer(w,s), receipt(w), budget(w), treasury(w), vault USDC(w), vault WSOL(w), vendor USDC(w), USDC mint, SPL Token, System Program, Instructions sysvar, SOL custody(w) |
| Execute SwapSolToUsdc | the PayUsdc prefix, then pool program, pool state(w), pool WSOL vault(w), pool USDC vault(w), oracle, tick array 0(w), tick array 1(w), tick array 2(w) |
| Pause | policy(w), owner(s) |
| Withdraw (tag 3) | policy(w), owner(w,s), SOL custody(w), vault USDC(w), vault WSOL(w), owner canonical USDC ATA(w), SPL Token, System Program |

| Account | Seeds | Size |
| --- | --- | --- |
| Policy | `[b"paysh-policy-v2", owner, instance_id]` | 1024 bytes |
| SOL custody | `[b"paysh-sol-v2", policy]` | zero-data System account |
| Receipt | `[b"paysh-receipt-v2", policy, nonce]` | 73 bytes |
| Budget | `[b"paysh-budget-v2", policy, u64LE(signing_timestamp / period_seconds)]` | 40 bytes |

SOL custody is a System Program account that the policy program signs for with PDA seeds, which lets it make native transfers before token CPIs. Where accounts carry padding, decode the struct prefix.

## Build and test

```sh
cargo build-sbf --manifest-path programs/paysh/Cargo.toml --arch v3 --optimize-size --tools-version v1.57 -- --locked
SBF_OUT_DIR="$PWD/target/deploy" cargo test --manifest-path sbf-tests/Cargo.toml --test paysh --locked
```

The Whirlpool tests need the pinned Orca fixture from `bash scripts/build-whirlpool-test.sh`, then `--test paysh_whirlpool`.

## Deployment

A PaySH deployment needs a funded fee payer, a separately recorded upgrade authority, classic SPL token accounts and a verified Whirlpool pool on the selected cluster. Public Testnet USDC fixtures are test assets, not mainnet USDC.

Rent matters:

- SOL custody must hold the rent-exempt reserve on top of its intended allocation. Every transfer must leave it at zero or rent-exempt; a leftover below the live minimum fails the transaction atomically.
- The treasury must already be rent-exempt before small service fees arrive.
- Query the cluster for current rent minima. Network fees and receipt/budget rent are the submitter's liabilities.

Version 2 uses its own request domain and PDA seeds, so it is always a fresh program deployment. Version-1 deployments and their pending-operation journals stay separate; keep the v1 client and deployment available to recover its pending operations. Release tooling hashes every built `allowit_*.so`, including `allowit_paysh.so`. Before deploying, compare the client's and the program's interface source, domain and seed constants; a mismatch is a release blocker.
