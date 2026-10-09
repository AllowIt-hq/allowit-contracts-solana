//! PaySH executes the pinned, unmodified Orca Whirlpool SBF program and real SPL Token CPIs.
//! Build `whirlpool.so` using scripts/build-whirlpool-test.sh before running this suite.
use allowit_paysh_interface::*;
use borsh::BorshDeserialize;
use ed25519_dalek::{Signer, SigningKey};
use mollusk_svm::{
    Mollusk,
    result::types::{TransactionProgramResult, TransactionResult},
};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction as Ix};
use solana_program::{
    hash::hash, program_option::COption, program_pack::Pack, pubkey::Pubkey as OldKey,
};
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use spl_token::state::{Account as Token, AccountState, Mint};
use std::collections::BTreeMap;

const Q64: u128 = 1 << 64;
const INPUT: u64 = 1_000_000;
const PRIOR_WSOL: u64 = 123_456;
const RESERVE: u64 = 2_039_280;
const WHIRLPOOL: Pubkey = Pubkey::from_str_const("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");
fn key(v: u8) -> Pubkey {
    Pubkey::new_from_array([v; 32])
}
fn old(k: Pubkey) -> OldKey {
    OldKey::new_from_array(k.to_bytes())
}
fn token_program() -> Pubkey {
    Pubkey::new_from_array(spl_token::id().to_bytes())
}
fn native_mint() -> Pubkey {
    Pubkey::new_from_array(spl_token::native_mint::id().to_bytes())
}
fn account(owner: Pubkey, data: Vec<u8>) -> Account {
    Account {
        lamports: 100_000_000,
        owner,
        data,
        executable: false,
        rent_epoch: 0,
    }
}
fn token(owner: Pubkey, mint: Pubkey, amount: u64, native: bool) -> Account {
    let mut data = vec![0; Token::LEN];
    Token::pack(
        Token {
            mint: old(mint),
            owner: old(owner),
            amount,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: if native {
                COption::Some(RESERVE)
            } else {
                COption::None
            },
            delegated_amount: 0,
            close_authority: COption::None,
        },
        &mut data,
    )
    .unwrap();
    let mut a = account(token_program(), data);
    if native {
        a.lamports = RESERVE + amount;
    }
    a
}
fn rw(k: Pubkey, signer: bool) -> AccountMeta {
    AccountMeta::new(k, signer)
}
fn ro(k: Pubkey, signer: bool) -> AccountMeta {
    AccountMeta::new_readonly(k, signer)
}

struct Fixture {
    svm: Mollusk,
    accounts: BTreeMap<Pubkey, Account>,
    program: Pubkey,
    policy: Pubkey,
    payer: Pubkey,
    treasury: Pubkey,
    usdc: Pubkey,
    wsol: Pubkey,
    vendor: Pubkey,
    mint: Pubkey,
    native: Pubkey,
    pool: Pubkey,
    pool_wsol: Pubkey,
    pool_usdc: Pubkey,
    oracle: Pubkey,
    ticks: [Pubkey; 3],
    request: Request,
}
impl Fixture {
    fn new(price_limit: u128, min_out: u64) -> Self {
        let program = key(41);
        let owner = key(42);
        let payer = key(43);
        let instance = [44; 32];
        let (policy, bump) =
            Pubkey::find_program_address(&[POLICY_SEED, owner.as_ref(), &instance], &program);
        let native = Pubkey::find_program_address(&[SOL_SEED, policy.as_ref()], &program).0;
        let treasury = key(45);
        let mint = key(46);
        let usdc = key(47);
        let wsol = key(48);
        let vendor = key(49);
        let config_key = key(60);
        let spacing = 64u16.to_le_bytes();
        let (pool, pool_bump) = Pubkey::find_program_address(
            &[
                b"whirlpool",
                config_key.as_ref(),
                native_mint().as_ref(),
                mint.as_ref(),
                &spacing,
            ],
            &WHIRLPOOL,
        );
        let pool_wsol = key(61);
        let pool_usdc = key(62);
        let oracle = Pubkey::find_program_address(&[b"oracle", pool.as_ref()], &WHIRLPOOL).0;
        let starts = [0i32, -5632, -11264];
        let ticks = starts.map(|start| {
            Pubkey::find_program_address(
                &[b"tick_array", pool.as_ref(), start.to_string().as_bytes()],
                &WHIRLPOOL,
            )
            .0
        });
        // Resolve PaySH explicitly: Mollusk's default search prefers tests/fixtures
        // and BPF_OUT_DIR over SBF_OUT_DIR, which could mask an old/new ELF comparison.
        let paysh_path = std::env::var_os("PAYSH_SBF_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("SBF_OUT_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/deploy")
                    })
                    .join("allowit_paysh.so")
            });
        let paysh_elf = std::fs::read(&paysh_path).expect("build the selected PaySH ELF");
        let paysh_sha256 = hash(&paysh_elf)
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        eprintln!(
            "PaySH fixture ELF: {} SHA-256 {paysh_sha256}",
            paysh_path.display()
        );
        let mut svm = Mollusk::default();
        svm.add_program_with_loader_and_elf(
            &program,
            &mollusk_svm::program::loader_keys::LOADER_V3,
            &paysh_elf,
        );
        let whirlpool_path = std::env::var_os("WHIRLPOOL_SBF_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../target/whirlpool-fixture/target/deploy/whirlpool.so")
            });
        let whirlpool_elf = std::fs::read(&whirlpool_path)
            .expect("build the pinned Whirlpool fixture with scripts/build-whirlpool-test.sh");
        svm.add_program_with_loader_and_elf(
            &WHIRLPOOL,
            &mollusk_svm::program::loader_keys::LOADER_V3,
            &whirlpool_elf,
        );
        mollusk_svm_programs_token::token::add_program(&mut svm);
        svm.compute_budget.compute_unit_limit = 1_400_000;
        svm.sysvars.clock.slot = 1000;
        svm.sysvars.clock.unix_timestamp = 3601;
        let config = Config {
            instance_id: instance,
            network: [1; 32],
            module_digest: [2; 32],
            evaluator: SigningKey::from_bytes(&[3; 32]).verifying_key().to_bytes(),
            treasury: treasury.to_bytes(),
            usdc_mint: mint.to_bytes(),
            vault_usdc: usdc.to_bytes(),
            vault_wsol: wsol.to_bytes(),
            vendor_usdc: vendor.to_bytes(),
            pool: Pool {
                program: WHIRLPOOL.to_bytes(),
                state: pool.to_bytes(),
                wsol: pool_wsol.to_bytes(),
                usdc: pool_usdc.to_bytes(),
                oracle: oracle.to_bytes(),
            },
            period_seconds: 3600,
            max_usdc_per_period: 10_000_000,
            max_swap_lamports_per_period: 10_000_000,
            max_fee_lamports_per_period: 10_000,
            max_sol_debits_per_period: 20_000_000,
            max_swap_lamports_per_call: 5_000_000,
            max_total_swap_lamports: 10_000_000,
            allocation_lamports: 20_000_000,
            service_fee_lamports: 1000,
            min_usdc_per_sol: 1,
            max_pool_fee_bps: 100,
            max_age_slots: 180,
            max_age_seconds: 60,
            policy_expires_timestamp: 10000,
            owner_only: false,
        };
        let p = Policy {
            version: 1,
            bump,
            owner: owner.to_bytes(),
            paused: false,
            total_swap_lamports: 0,
            total_sol_debits: 0,
            config,
        };
        let mut data = borsh::to_vec(&p).unwrap();
        data.resize(POLICY_BYTES, 0);
        let mut accounts = BTreeMap::new();
        accounts.insert(policy, account(program, data));
        for k in [owner, payer, treasury, native, oracle] {
            accounts.insert(k, account(key(0), vec![]));
        }
        let mut mint_data = vec![0; Mint::LEN];
        Mint::pack(
            Mint {
                mint_authority: COption::None,
                supply: 2_000_000_000_000,
                decimals: 6,
                is_initialized: true,
                freeze_authority: COption::None,
            },
            &mut mint_data,
        )
        .unwrap();
        accounts.insert(mint, account(token_program(), mint_data));
        accounts.insert(usdc, token(policy, mint, 0, false));
        accounts.insert(wsol, token(policy, native_mint(), PRIOR_WSOL, true));
        accounts.insert(vendor, token(key(51), mint, 0, false));
        accounts.insert(
            pool_wsol,
            token(pool, native_mint(), 1_000_000_000_000, true),
        );
        accounts.insert(pool_usdc, token(pool, mint, 1_000_000_000_000, false));
        // Fixed-layout Whirlpool: static 0.3% fee, 100 billion active liquidity,
        // price 1, current tick 0. No initialized ticks are crossed by these swaps.
        let mut pool_data = vec![0; 653];
        pool_data[..8].copy_from_slice(&hash(b"account:Whirlpool").to_bytes()[..8]);
        pool_data[8..40].copy_from_slice(config_key.as_ref());
        pool_data[40] = pool_bump;
        pool_data[41..43].copy_from_slice(&spacing);
        pool_data[43..45].copy_from_slice(&spacing);
        pool_data[45..47].copy_from_slice(&3000u16.to_le_bytes());
        pool_data[49..65].copy_from_slice(&100_000_000_000u128.to_le_bytes());
        pool_data[65..81].copy_from_slice(&Q64.to_le_bytes());
        pool_data[101..133].copy_from_slice(native_mint().as_ref());
        pool_data[133..165].copy_from_slice(pool_wsol.as_ref());
        pool_data[181..213].copy_from_slice(mint.as_ref());
        pool_data[213..245].copy_from_slice(pool_usdc.as_ref());
        accounts.insert(pool, account(WHIRLPOOL, pool_data));
        for (tick, start) in ticks.into_iter().zip(starts) {
            let mut data = vec![0; 9988];
            data[..8].copy_from_slice(&hash(b"account:TickArray").to_bytes()[..8]);
            data[8..12].copy_from_slice(&start.to_le_bytes());
            data[9956..9988].copy_from_slice(pool.as_ref());
            accounts.insert(tick, account(WHIRLPOOL, data));
        }
        for (k, a) in [
            mollusk_svm::program::keyed_account_for_system_program(),
            mollusk_svm_programs_token::token::keyed_account(),
        ] {
            accounts.insert(k, a);
        }
        for k in [program, WHIRLPOOL] {
            accounts.insert(
                k,
                mollusk_svm::program::create_program_account_loader_v3(&k),
            );
        }
        let request = Request {
            network: [1; 32],
            program: program.to_bytes(),
            policy: policy.to_bytes(),
            owner: owner.to_bytes(),
            module_digest: [2; 32],
            operation_id: [4; 32],
            nonce: [5; 32],
            challenge_hash: [6; 32],
            evidence_hash: [7; 32],
            signing_slot: 1000,
            signing_timestamp: 3601,
            expires_slot: 1180,
            expires_timestamp: 3661,
            service_fee_lamports: 1000,
            action: Action::SwapSolToUsdc {
                amount_in_lamports: INPUT,
                min_out_usdc: min_out,
                sqrt_price_limit: price_limit,
                tick_arrays: ticks.map(|k| k.to_bytes()),
            },
        };
        let mut f = Self {
            svm,
            accounts,
            program,
            policy,
            payer,
            treasury,
            usdc,
            wsol,
            vendor,
            mint,
            native,
            pool,
            pool_wsol,
            pool_usdc,
            oracle,
            ticks,
            request,
        };
        for k in [f.receipt(), f.budget()] {
            f.accounts.insert(k, Account::default());
        }
        f
    }
    fn receipt(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[RECEIPT_SEED, self.policy.as_ref(), &self.request.nonce],
            &self.program,
        )
        .0
    }
    fn budget(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[BUDGET_SEED, self.policy.as_ref(), &1u64.to_le_bytes()],
            &self.program,
        )
        .0
    }
    fn snapshot(&self) -> Vec<(Pubkey, Account)> {
        self.accounts.iter().map(|(k, a)| (*k, a.clone())).collect()
    }
    fn amount_in(result: &TransactionResult, key: Pubkey) -> u64 {
        Token::unpack(
            &result
                .resulting_accounts
                .iter()
                .find(|(k, _)| *k == key)
                .unwrap()
                .1
                .data,
        )
        .unwrap()
        .amount
    }
    fn execute(&self) -> TransactionResult {
        let instructions =
            Pubkey::new_from_array(solana_program::sysvar::instructions::id().to_bytes());
        let mut accounts = vec![
            rw(self.policy, false),
            rw(self.payer, true),
            rw(self.receipt(), false),
            rw(self.budget(), false),
            rw(self.treasury, false),
            rw(self.usdc, false),
            rw(self.wsol, false),
            rw(self.vendor, false),
            ro(self.mint, false),
            ro(token_program(), false),
            ro(key(0), false),
            ro(instructions, false),
            rw(self.native, false),
            ro(WHIRLPOOL, false),
            rw(self.pool, false),
            rw(self.pool_wsol, false),
            rw(self.pool_usdc, false),
            ro(self.oracle, false),
        ];
        accounts.extend(self.ticks.map(|k| rw(k, false)));
        let ix = Ix {
            program_id: self.program,
            accounts,
            data: borsh::to_vec(&Instruction::Execute(self.request.clone())).unwrap(),
        };
        let message = self.request.signed_message();
        let mut ed = vec![1, 0];
        for n in [
            48u16,
            u16::MAX,
            16,
            u16::MAX,
            112,
            message.len() as u16,
            u16::MAX,
        ] {
            ed.extend(n.to_le_bytes());
        }
        let signing_key = SigningKey::from_bytes(&[3; 32]);
        ed.extend(signing_key.verifying_key().to_bytes());
        ed.extend(signing_key.sign(&message).to_bytes());
        ed.extend(message);
        let edix = Ix {
            program_id: Pubkey::new_from_array(solana_program::ed25519_program::id().to_bytes()),
            accounts: vec![],
            data: ed,
        };
        self.svm
            .process_transaction_instructions(&[edix, ix], &self.snapshot(), Some(&self.payer))
    }
    fn assert_rolled_back(&self, result: &TransactionResult) {
        // Inspect runtime output itself, including account creation and every CPI-mutated account.
        for k in [
            self.policy,
            self.payer,
            self.native,
            self.treasury,
            self.usdc,
            self.wsol,
            self.pool,
            self.pool_wsol,
            self.pool_usdc,
            self.receipt(),
            self.budget(),
            self.ticks[0],
            self.ticks[1],
            self.ticks[2],
        ] {
            assert_eq!(
                &result
                    .resulting_accounts
                    .iter()
                    .find(|(found, _)| *found == k)
                    .unwrap()
                    .1,
                &self.accounts[&k],
                "rollback account {k}"
            );
        }
    }
    fn direct_whirlpool_swap(mut self) -> TransactionResult {
        // Independently establish that the same real pool permits a partial fill meeting minOut.
        for k in [self.wsol, self.usdc] {
            let a = self.accounts.get_mut(&k).unwrap();
            let mut t = Token::unpack(&a.data).unwrap();
            t.owner = old(self.payer);
            if k == self.wsol {
                t.amount += INPUT;
                a.lamports += INPUT;
            }
            Token::pack(t, &mut a.data).unwrap();
        }
        let Action::SwapSolToUsdc {
            min_out_usdc,
            sqrt_price_limit,
            ..
        } = self.request.action
        else {
            unreachable!()
        };
        let mut data = hash(b"global:swap").to_bytes()[..8].to_vec();
        data.extend(INPUT.to_le_bytes());
        data.extend(min_out_usdc.to_le_bytes());
        data.extend(sqrt_price_limit.to_le_bytes());
        data.extend([1, 1]);
        let ix = Ix {
            program_id: WHIRLPOOL,
            accounts: vec![
                ro(token_program(), false),
                ro(self.payer, true),
                rw(self.pool, false),
                rw(self.wsol, false),
                rw(self.pool_wsol, false),
                rw(self.usdc, false),
                rw(self.pool_usdc, false),
                rw(self.ticks[0], false),
                rw(self.ticks[1], false),
                rw(self.ticks[2], false),
                ro(self.oracle, false),
            ],
            data,
        };
        self.svm
            .process_transaction_instructions(&[ix], &self.snapshot(), Some(&self.payer))
    }
}

#[test]
fn real_whirlpool_full_input_preserves_prior_wsol_and_records_budget() {
    let f = Fixture::new(Q64 - Q64 / 10_000, 990_000);
    let result = f.execute();
    assert_eq!(result.program_result, TransactionProgramResult::Success);
    assert_eq!(Fixture::amount_in(&result, f.wsol), PRIOR_WSOL);
    let output = Fixture::amount_in(&result, f.usdc);
    assert!(output >= 990_000 && output < INPUT);
    assert_eq!(
        Fixture::amount_in(&result, f.pool_wsol),
        1_000_000_000_000 + INPUT
    );
    assert_eq!(
        Fixture::amount_in(&result, f.pool_usdc),
        1_000_000_000_000 - output
    );
    let state = |key| {
        &result
            .resulting_accounts
            .iter()
            .find(|(k, _)| *k == key)
            .unwrap()
            .1
    };
    assert_eq!(
        state(f.treasury).lamports,
        f.accounts[&f.treasury].lamports + 1000
    );
    assert_eq!(
        state(f.native).lamports,
        f.accounts[&f.native].lamports - INPUT - 1000
    );
    let budget = Budget::deserialize(&mut &state(f.budget()).data[..]).unwrap();
    assert_eq!(budget.swap_lamports, INPUT);
    assert_eq!(budget.fee_lamports, 1000);
    assert_eq!(budget.sol_debits, INPUT + 1000);
    let receipt = Receipt::deserialize(&mut &state(f.receipt()).data[..]).unwrap();
    assert_eq!(receipt.operation_id, f.request.operation_id);
    assert_eq!(
        receipt.request_hash,
        hash(&f.request.signed_message()).to_bytes()
    );
}

#[test]
fn real_whirlpool_partial_input_meets_minout_but_paysh_rolls_back() {
    let limit = Q64 - Q64 / 200_000;
    let direct = Fixture::new(limit, 450_000);
    let source = direct.wsol;
    let destination = direct.usdc;
    let result = direct.direct_whirlpool_swap();
    assert_eq!(result.program_result, TransactionProgramResult::Success);
    let remaining = Fixture::amount_in(&result, source);
    assert!(remaining > PRIOR_WSOL && remaining < PRIOR_WSOL + INPUT);
    assert!(Fixture::amount_in(&result, destination) >= 450_000);
    let f = Fixture::new(limit, 450_000);
    let result = f.execute();
    assert_eq!(
        result.program_result,
        TransactionProgramResult::Failure(1, ProgramError::Custom(210))
    );
    f.assert_rolled_back(&result);
}

#[test]
fn real_whirlpool_full_input_preserves_unsynced_native_donation() {
    let mut f = Fixture::new(Q64 - Q64 / 10_000, 990_000);
    // Anyone can transfer native SOL to the WSOL account without calling SyncNative.
    // It remains owner property and must not make an otherwise valid swap fail.
    f.accounts.get_mut(&f.wsol).unwrap().lamports += 777;
    let result = f.execute();
    assert_eq!(result.program_result, TransactionProgramResult::Success);
    assert_eq!(Fixture::amount_in(&result, f.wsol), PRIOR_WSOL + 777);
    let wsol = &result
        .resulting_accounts
        .iter()
        .find(|(key, _)| *key == f.wsol)
        .unwrap()
        .1;
    assert_eq!(wsol.lamports, RESERVE + PRIOR_WSOL + 777);
    assert!(Fixture::amount_in(&result, f.usdc) >= 990_000);
}

#[test]
fn real_whirlpool_partial_input_with_native_donation_still_rolls_back() {
    let mut f = Fixture::new(Q64 - Q64 / 200_000, 450_000);
    f.accounts.get_mut(&f.wsol).unwrap().lamports += 777;
    let result = f.execute();
    assert_eq!(
        result.program_result,
        TransactionProgramResult::Failure(1, ProgramError::Custom(210))
    );
    f.assert_rolled_back(&result);
}

#[test]
fn real_whirlpool_minout_failure_rolls_back_fee_receipt_and_budget() {
    let f = Fixture::new(Q64 - Q64 / 10_000, INPUT);
    let result = f.execute();
    // Orca AmountOutBelowMinimum occurs after PaySH's initial native fee transfer.
    assert_eq!(
        result.program_result,
        TransactionProgramResult::Failure(1, ProgramError::Custom(6036))
    );
    f.assert_rolled_back(&result);
}
