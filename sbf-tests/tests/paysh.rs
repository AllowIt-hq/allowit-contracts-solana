//! Compiled-SBF policy, Ed25519 precompile, and real SPL Token CPI tests.
use allowit_paysh_interface::*;
use borsh::BorshDeserialize;
use ed25519_dalek::{Signer, SigningKey};
use mollusk_svm::{result::types::TransactionResult, Mollusk};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction as Ix};
use solana_program::{program_option::COption, program_pack::Pack, pubkey::Pubkey as OldKey};
use solana_pubkey::Pubkey;
use spl_token::state::{Account as Token, AccountState, Mint};
use std::collections::BTreeMap;
fn key(v: u8) -> Pubkey {
    Pubkey::new_from_array([v; 32])
}
fn old(k: Pubkey) -> OldKey {
    OldKey::new_from_array(k.to_bytes())
}
fn a(owner: Pubkey, data: Vec<u8>) -> Account {
    Account {
        lamports: 100_000_000,
        owner,
        data,
        executable: false,
        rent_epoch: 0,
    }
}
fn rw(k: Pubkey, s: bool) -> AccountMeta {
    AccountMeta::new(k, s)
}
fn ro(k: Pubkey, s: bool) -> AccountMeta {
    AccountMeta::new_readonly(k, s)
}
struct F {
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
    r: Request,
}
impl F {
    fn new() -> Self {
        let program = key(41);
        let owner = key(42);
        let payer = key(43);
        let instance = [44; 32];
        let (policy, bump) =
            Pubkey::find_program_address(&[POLICY_SEED, owner.as_ref(), &instance], &program);
        let treasury = key(45);
        let mint = key(46);
        let usdc = key(47);
        let wsol = key(48);
        let vendor = key(49);
        let token_program = Pubkey::new_from_array(spl_token::id().to_bytes());
        let mut svm = Mollusk::new(&program, "allowit_paysh");
        mollusk_svm_programs_token::token::add_program(&mut svm);
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
                program: [11; 32],
                state: [12; 32],
                wsol: [13; 32],
                usdc: [14; 32],
                oracle: [15; 32],
            },
            period_seconds: 3600,
            max_usdc_per_period: 10_000_000,
            max_swap_lamports_per_period: 10_000_000,
            max_fee_lamports_per_period: 10000,
            max_sol_debits_per_period: 20_000_000,
            max_swap_lamports_per_call: 5_000_000,
            max_total_swap_lamports: 10_000_000,
            allocation_lamports: 20_000_000,
            service_fee_lamports: 1000,
            min_usdc_per_sol: 100_000_000,
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
        let native = Pubkey::find_program_address(&[SOL_SEED, policy.as_ref()], &program).0;
        accounts.insert(native, a(key(0), vec![]));
        accounts.insert(policy, a(program, data));
        for k in [payer, treasury, vendor, mint] {
            accounts.insert(k, a(key(0), vec![]));
        }
        let mut mint_data = vec![0; Mint::LEN];
        Mint::pack(
            Mint {
                mint_authority: COption::None,
                supply: 20_000_000,
                decimals: 6,
                is_initialized: true,
                freeze_authority: COption::None,
            },
            &mut mint_data,
        )
        .unwrap();
        accounts.insert(mint, a(token_program, mint_data));
        for (k, owner, m, amount, native) in [
            (usdc, policy, mint, 20_000_000, COption::None),
            (vendor, key(51), mint, 0, COption::None),
            (
                wsol,
                policy,
                Pubkey::new_from_array(spl_token::native_mint::id().to_bytes()),
                0,
                COption::Some(2039280),
            ),
        ] {
            let mut data = vec![0; Token::LEN];
            Token::pack(
                Token {
                    mint: old(m),
                    owner: old(owner),
                    amount,
                    delegate: COption::None,
                    state: AccountState::Initialized,
                    is_native: native,
                    delegated_amount: 0,
                    close_authority: COption::None,
                },
                &mut data,
            )
            .unwrap();
            accounts.insert(k, a(token_program, data));
        }
        for (k, account) in [
            mollusk_svm::program::keyed_account_for_system_program(),
            mollusk_svm_programs_token::token::keyed_account(),
        ] {
            accounts.insert(k, account);
        }
        accounts.insert(
            program,
            mollusk_svm::program::create_program_account_loader_v3(&program),
        );
        let r = Request {
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
            action: Action::PayUsdc { amount: 1_000_000 },
        };
        Self {
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
            r,
        }
    }
    fn run(&mut self, mutate_signed: bool) -> TransactionResult {
        let (rk, _) = Pubkey::find_program_address(
            &[RECEIPT_SEED, self.policy.as_ref(), &self.r.nonce],
            &self.program,
        );
        let period = (self.r.signing_timestamp as u64 / 3600).to_le_bytes();
        let (bk, _) = Pubkey::find_program_address(
            &[BUDGET_SEED, self.policy.as_ref(), &period],
            &self.program,
        );
        self.accounts.entry(rk).or_insert(a(key(0), vec![]));
        self.accounts.entry(bk).or_insert(a(key(0), vec![]));
        let tp = Pubkey::new_from_array(spl_token::id().to_bytes());
        let instructions =
            Pubkey::new_from_array(solana_program::sysvar::instructions::id().to_bytes());
        let ix = Ix {
            program_id: self.program,
            accounts: vec![
                rw(self.policy, false),
                rw(self.payer, true),
                rw(rk, false),
                rw(bk, false),
                rw(self.treasury, false),
                rw(self.usdc, false),
                rw(self.wsol, false),
                rw(self.vendor, false),
                ro(self.mint, false),
                ro(tp, false),
                ro(key(0), false),
                ro(instructions, false),
                rw(
                    Pubkey::find_program_address(&[SOL_SEED, self.policy.as_ref()], &self.program)
                        .0,
                    false,
                ),
            ],
            data: borsh::to_vec(&Instruction::Execute(self.r.clone())).unwrap(),
        };
        let message = self.r.signed_message();
        let mut ed = vec![1, 0];
        for v in [
            48u16,
            u16::MAX,
            16,
            u16::MAX,
            112,
            message.len() as u16,
            u16::MAX,
        ] {
            ed.extend(v.to_le_bytes())
        }
        ed.extend(SigningKey::from_bytes(&[3; 32]).verifying_key().to_bytes());
        ed.extend(SigningKey::from_bytes(&[3; 32]).sign(&message).to_bytes());
        ed.extend(message);
        if mutate_signed {
            *ed.last_mut().unwrap() ^= 1;
        }
        let edix = Ix {
            program_id: Pubkey::new_from_array(solana_program::ed25519_program::id().to_bytes()),
            accounts: vec![],
            data: ed,
        };
        let before = self
            .accounts
            .iter()
            .map(|(k, a)| (*k, a.clone()))
            .collect::<Vec<_>>();
        let result =
            self.svm
                .process_transaction_instructions(&[edix, ix], &before, Some(&self.payer));
        if result.program_result.is_ok() {
            for (k, a) in &result.resulting_accounts {
                self.accounts.insert(*k, a.clone());
            }
        }
        result
    }
    fn amount(&self, k: Pubkey) -> u64 {
        Token::unpack(&self.accounts[&k].data).unwrap().amount
    }
}
#[test]
fn pays_fee_then_pays_vendor_and_consumes_nonce() {
    let mut f = F::new();
    let before = f.accounts[&f.treasury].lamports;
    let r = f.run(false);
    assert!(r.program_result.is_ok(), "{:?}", r.program_result);
    assert_eq!(f.amount(f.vendor), 1_000_000);
    assert_eq!(f.accounts[&f.treasury].lamports, before + 1000);
    assert!(f.run(false).program_result.is_err());
}
#[test]
fn signed_body_tampering_rejected() {
    let mut f = F::new();
    assert!(f.run(true).program_result.is_err());
    assert_eq!(f.amount(f.vendor), 0);
}
#[test]
fn stale_request_and_budget_overrun_rejected() {
    let mut f = F::new();
    f.svm.sysvars.clock.unix_timestamp = 3662;
    assert!(f.run(false).program_result.is_err());
    let mut f = F::new();
    f.r.action = Action::PayUsdc { amount: 10_000_001 };
    assert!(f.run(false).program_result.is_err());
}
#[test]
fn late_execution_charges_original_bucket() {
    let mut f = F::new();
    f.r.signing_timestamp = 3599;
    f.r.expires_timestamp = 3659;
    assert!(f.run(false).program_result.is_ok());
    let (bk, _) = Pubkey::find_program_address(
        &[BUDGET_SEED, f.policy.as_ref(), &0u64.to_le_bytes()],
        &f.program,
    );
    assert_eq!(
        Budget::deserialize(&mut &f.accounts[&bk].data[..])
            .unwrap()
            .usdc,
        1_000_000
    );
}
#[test]
fn token_failure_rolls_back_fee_and_receipt() {
    let mut f = F::new();
    Token::pack(
        Token {
            amount: 0,
            ..Token::unpack(&f.accounts[&f.usdc].data).unwrap()
        },
        &mut f.accounts.get_mut(&f.usdc).unwrap().data,
    )
    .unwrap();
    let before = f.accounts[&f.treasury].lamports;
    assert!(f.run(false).program_result.is_err());
    assert_eq!(f.accounts[&f.treasury].lamports, before);
    assert_eq!(f.amount(f.vendor), 0);
}

#[test]
fn owner_domain_and_exact_service_fee_are_authenticated() {
    for change in 0..4 {
        let mut f = F::new();
        match change {
            0 => f.r.owner = [0; 32],
            1 => f.r.network = [0; 32],
            2 => f.r.service_fee_lamports = 1,
            _ => f.r.module_digest = [0; 32],
        }
        let before = f.accounts[&f.treasury].lamports;
        assert!(f.run(false).program_result.is_err());
        assert_eq!(f.accounts[&f.treasury].lamports, before);
    }
}
#[test]
fn service_fees_cannot_exceed_lifetime_allocation_or_gross_period_cap() {
    for lifetime in [false, true] {
        let mut f = F::new();
        let mut p = Policy::deserialize(&mut &f.accounts[&f.policy].data[..]).unwrap();
        if lifetime {
            p.total_sol_debits = p.config.allocation_lamports;
        } else {
            p.config.max_sol_debits_per_period = 999;
        }
        let mut data = borsh::to_vec(&p).unwrap();
        data.resize(POLICY_BYTES, 0);
        f.accounts.get_mut(&f.policy).unwrap().data = data;
        let before = f.accounts[&f.treasury].lamports;
        assert!(f.run(false).program_result.is_err());
        assert_eq!(f.accounts[&f.treasury].lamports, before);
    }
}
#[test]
fn owner_only_and_pause_block_public_submitters() {
    for paused in [false, true] {
        let mut f = F::new();
        let mut p = Policy::deserialize(&mut &f.accounts[&f.policy].data[..]).unwrap();
        if paused {
            p.paused = true;
        } else {
            p.config.owner_only = true;
        }
        let mut data = borsh::to_vec(&p).unwrap();
        data.resize(POLICY_BYTES, 0);
        f.accounts.get_mut(&f.policy).unwrap().data = data;
        assert!(f.run(false).program_result.is_err());
        assert_eq!(f.amount(f.vendor), 0);
    }
}
