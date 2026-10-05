use allowit_interface::{ABI_VERSION, STATE_BYTES, VAULT_SEED, VaultInstruction as V, VaultState};
use borsh::BorshDeserialize;
use mollusk_svm::{Mollusk, result::InstructionResult};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_program::{program_option::COption, program_pack::Pack, pubkey::Pubkey as OldKey};
use solana_pubkey::Pubkey;
use spl_token::state::{Account as Token, AccountState, Mint};
use std::collections::BTreeMap;
#[path = "../../policy/source_hash.rs"]
mod source_hash;
fn key(b: u8) -> Pubkey {
    Pubkey::new_from_array([b; 32])
}
fn old(key: Pubkey) -> OldKey {
    OldKey::new_from_array(key.to_bytes())
}
fn ro(key: Pubkey, signer: bool) -> AccountMeta {
    AccountMeta::new_readonly(key, signer)
}
fn rw(key: Pubkey, signer: bool) -> AccountMeta {
    AccountMeta::new(key, signer)
}
fn account(owner: Pubkey, data: Vec<u8>) -> Account {
    Account {
        owner,
        data,
        lamports: 10_000_000_000,
        executable: false,
        rent_epoch: 0,
    }
}
struct F {
    svm: Mollusk,
    accounts: BTreeMap<Pubkey, Account>,
    program: Pubkey,
    vault: Pubkey,
    owner: Pubkey,
    executor: Pubkey,
    mint: Pubkey,
    source: Pubkey,
    tokens: Pubkey,
    recipient: Pubkey,
    policy: Pubkey,
    policy_data: Pubkey,
    artifact: [u8; 32],
}
impl F {
    fn new() -> Self {
        assert!(
            std::env::var_os("SBF_OUT_DIR").is_some(),
            "SBF_OUT_DIR must point at actual compiled programs"
        );
        let program = key(10);
        let policy = key(11);
        let owner = key(12);
        let executor = key(13);
        let mint = key(14);
        let source = key(15);
        let tokens = key(16);
        let recipient = key(17);
        let loader =
            Pubkey::new_from_array(solana_program::bpf_loader_upgradeable::id().to_bytes());
        let (policy_data, _) = Pubkey::find_program_address(&[policy.as_ref()], &loader);
        let elf = std::fs::read(
            std::path::Path::new(&std::env::var("SBF_OUT_DIR").unwrap()).join("allowit_policy.so"),
        )
        .unwrap();
        let artifact = solana_program::hash::hash(&elf).to_bytes();
        let (vault, bump) =
            Pubkey::find_program_address(&[VAULT_SEED, owner.as_ref(), &[18; 32]], &program);
        let mut svm = Mollusk::new(&program, "allowit_vault");
        svm.add_program(&policy, "allowit_policy");
        mollusk_svm_programs_token::token::add_program(&mut svm);
        svm.sysvars.clock.unix_timestamp = 86_401;
        let mut accounts = BTreeMap::new();
        let state = VaultState {
            abi: ABI_VERSION,
            bump,
            owner: owner.to_bytes(),
            executor: executor.to_bytes(),
            mint: mint.to_bytes(),
            token_account: tokens.to_bytes(),
            policy: policy.to_bytes(),
            policy_source: source_hash::SOURCE_HASH,
            policy_artifact: artifact,
            vault_id: [18; 32],
            daily_limit: 25_000_000,
            spent: 0,
            spent_day: 1,
            nonce: 0,
            revision: 0,
            approved: false,
        };
        let mut data = borsh::to_vec(&state).unwrap();
        data.resize(STATE_BYTES, 0);
        accounts.insert(vault, account(program, data));
        for k in [owner, executor] {
            accounts.insert(k, account(key(0), vec![]));
        }
        let token_owner = Pubkey::new_from_array(spl_token::id().to_bytes());
        let mut data = vec![0; Mint::LEN];
        Mint::pack(
            Mint {
                mint_authority: COption::None,
                supply: 200_000_000,
                decimals: 6,
                is_initialized: true,
                freeze_authority: COption::None,
            },
            &mut data,
        )
        .unwrap();
        accounts.insert(mint, account(token_owner, data));
        for (k, authority, amount) in [
            (source, owner, 100_000_000),
            (tokens, vault, 0),
            (recipient, key(19), 0),
        ] {
            let mut data = vec![0; Token::LEN];
            Token::pack(
                Token {
                    mint: old(mint),
                    owner: old(authority),
                    amount,
                    delegate: COption::None,
                    state: AccountState::Initialized,
                    is_native: COption::None,
                    delegated_amount: 0,
                    close_authority: COption::None,
                },
                &mut data,
            )
            .unwrap();
            accounts.insert(k, account(token_owner, data));
        }
        for (key, account) in [
            mollusk_svm_programs_token::token::keyed_account(),
            mollusk_svm::program::keyed_account_for_system_program(),
        ] {
            accounts.insert(key, account);
        }
        accounts.insert(
            policy,
            mollusk_svm::program::create_program_account_loader_v3(&policy),
        );
        accounts.insert(
            program,
            mollusk_svm::program::create_program_account_loader_v3(&program),
        );
        let mut policy_bytes = vec![0u8; 45];
        policy_bytes[..4].copy_from_slice(&3u32.to_le_bytes());
        policy_bytes.extend_from_slice(&elf);
        accounts.insert(policy_data, account(loader, policy_bytes));
        Self {
            svm,
            accounts,
            program,
            vault,
            owner,
            executor,
            mint,
            source,
            tokens,
            recipient,
            policy,
            policy_data,
            artifact,
        }
    }
    fn state(&self) -> VaultState {
        VaultState::deserialize(&mut &self.accounts[&self.vault].data[..]).unwrap()
    }
    fn amount(&self, k: Pubkey) -> u64 {
        Token::unpack(&self.accounts[&k].data).unwrap().amount
    }
    fn run(&mut self, instruction: V, metas: Vec<AccountMeta>) -> InstructionResult {
        let ix = Instruction {
            program_id: self.program,
            accounts: metas,
            data: borsh::to_vec(&instruction).unwrap(),
        };
        let before: Vec<_> = self.accounts.iter().map(|(k, a)| (*k, a.clone())).collect();
        let result = self.svm.process_instruction(&ix, &before);
        if result.program_result.is_ok() {
            for (k, a) in &result.resulting_accounts {
                self.accounts.insert(*k, a.clone());
            }
        } else {
            for k in [self.vault, self.source, self.tokens, self.recipient] {
                assert_eq!(
                    result.get_account(&k).unwrap(),
                    self.accounts.get(&k).unwrap(),
                    "rejection mutated state"
                );
            }
        }
        result
    }
    fn control(&mut self, i: V) -> InstructionResult {
        self.run(
            i,
            vec![
                rw(self.vault, false),
                ro(self.owner, true),
                ro(self.policy, false),
                ro(self.policy_data, false),
            ],
        )
    }
    fn deposit(&mut self, amount: u64) -> InstructionResult {
        self.run(
            V::Deposit { amount },
            vec![
                rw(self.vault, false),
                ro(self.owner, true),
                rw(self.source, false),
                rw(self.tokens, false),
                ro(self.mint, false),
                ro(Pubkey::new_from_array(spl_token::id().to_bytes()), false),
            ],
        )
    }
    fn transfer(&mut self, amount: u64, nonce: u64, revision: u64) -> InstructionResult {
        self.run(
            V::Transfer {
                amount,
                nonce,
                expected_revision: revision,
            },
            vec![
                rw(self.vault, false),
                ro(self.executor, true),
                rw(self.tokens, false),
                rw(self.recipient, false),
                ro(self.mint, false),
                ro(Pubkey::new_from_array(spl_token::id().to_bytes()), false),
                ro(self.policy, false),
                ro(self.policy_data, false),
            ],
        )
    }
}
#[test]
fn real_cpi_transfer_daily_limits_nonce_and_day_rollover() {
    let mut f = F::new();
    assert!(f.deposit(100_000_000).program_result.is_ok());
    assert!(f.transfer(1, 0, 0).program_result.is_err());
    assert!(
        f.control(V::Approve {
            approved: true,
            expected_revision: 0
        })
        .program_result
        .is_ok()
    );
    assert!(f.transfer(20_000_000, 0, 1).program_result.is_ok());
    assert_eq!(f.amount(f.recipient), 20_000_000);
    assert!(f.transfer(1, 0, 1).program_result.is_err());
    assert!(f.transfer(6_000_000, 1, 1).program_result.is_err());
    assert!(f.transfer(5_000_000, 1, 1).program_result.is_ok());
    f.svm.sysvars.clock.unix_timestamp = 172_800;
    assert!(f.transfer(25_000_000, 2, 1).program_result.is_ok());
    assert_eq!(f.state().spent, 25_000_000);
    assert_eq!(f.state().nonce, 3);
}
#[test]
fn tuning_revocation_recovery_and_placeholders() {
    let mut f = F::new();
    assert!(f.deposit(100_000_000).program_result.is_ok());
    assert!(
        f.control(V::Approve {
            approved: true,
            expected_revision: 0
        })
        .program_result
        .is_ok()
    );
    assert!(f.transfer(20_000_000, 0, 1).program_result.is_ok());
    assert!(
        f.control(V::SetDailyLimit {
            value: 50_000_001,
            expected_revision: 1
        })
        .program_result
        .is_err()
    );
    assert!(
        f.control(V::SetDailyLimit {
            value: 40_000_000,
            expected_revision: 1
        })
        .program_result
        .is_ok()
    );
    assert_eq!(f.state().spent, 20_000_000);
    assert!(f.transfer(1, 1, 1).program_result.is_err());
    assert!(f.transfer(20_000_000, 1, 2).program_result.is_ok());
    assert!(
        f.control(V::Approve {
            approved: false,
            expected_revision: 2
        })
        .program_result
        .is_ok()
    );
    assert!(f.transfer(1, 2, 3).program_result.is_err());
    let r = f.run(
        V::Withdraw { amount: 60_000_000 },
        vec![
            rw(f.vault, false),
            ro(f.owner, true),
            rw(f.tokens, false),
            rw(f.source, false),
            ro(f.mint, false),
            ro(Pubkey::new_from_array(spl_token::id().to_bytes()), false),
        ],
    );
    assert!(r.program_result.is_ok());
    assert_eq!(f.amount(f.tokens), 0);
    assert_eq!(f.state().spent, 40_000_000);
    assert!(
        f.run(V::Unsupported { method: 1 }, vec![])
            .program_result
            .is_err()
    );
}
#[test]
fn forged_actor_and_policy_cannot_spend() {
    let mut f = F::new();
    assert!(f.deposit(100_000_000).program_result.is_ok());
    assert!(
        f.control(V::Approve {
            approved: true,
            expected_revision: 0,
        })
        .program_result
        .is_ok()
    );
    let metas = vec![
        rw(f.vault, false),
        ro(f.owner, true),
        rw(f.tokens, false),
        rw(f.recipient, false),
        ro(f.mint, false),
        ro(Pubkey::new_from_array(spl_token::id().to_bytes()), false),
        ro(f.policy, false),
    ];
    assert!(
        f.run(
            V::Transfer {
                amount: 1,
                nonce: 0,
                expected_revision: 1
            },
            metas
        )
        .program_result
        .is_err()
    );
    let metas = vec![
        rw(f.vault, false),
        ro(f.executor, false),
        rw(f.tokens, false),
        rw(f.recipient, false),
        ro(f.mint, false),
        ro(Pubkey::new_from_array(spl_token::id().to_bytes()), false),
        ro(f.policy, false),
    ];
    assert!(
        f.run(
            V::Transfer {
                amount: 1,
                nonce: 0,
                expected_revision: 1
            },
            metas
        )
        .program_result
        .is_err()
    );
    let metas = vec![
        rw(f.vault, false),
        ro(f.executor, true),
        rw(f.tokens, false),
        rw(f.recipient, false),
        ro(f.mint, false),
        ro(Pubkey::new_from_array(spl_token::id().to_bytes()), false),
        ro(f.program, false),
    ];
    assert!(
        f.run(
            V::Transfer {
                amount: 1,
                nonce: 0,
                expected_revision: 1
            },
            metas
        )
        .program_result
        .is_err()
    );
}
#[test]
fn initialize_creates_factory_pda_and_policy_change_keeps_counters() {
    let mut f = F::new();
    f.accounts.insert(
        f.vault,
        Account {
            lamports: 0,
            data: vec![],
            owner: Pubkey::default(),
            executable: false,
            rent_epoch: 0,
        },
    );
    let ix = V::Initialize {
        vault_id: [18; 32],
        policy_source: source_hash::SOURCE_HASH,
        policy_artifact: f.artifact,
        daily_limit: 25_000_000,
    };
    assert!(
        f.run(
            ix,
            vec![
                rw(f.vault, false),
                rw(f.owner, true),
                ro(f.mint, false),
                ro(f.tokens, false),
                ro(f.policy, false),
                ro(f.executor, false),
                ro(Pubkey::default(), false),
                ro(f.policy_data, false)
            ]
        )
        .program_result
        .is_ok()
    );
    assert!(f.deposit(100_000_000).program_result.is_ok());
    assert!(
        f.control(V::Approve {
            approved: true,
            expected_revision: 0,
        })
        .program_result
        .is_ok()
    );
    assert!(f.transfer(20_000_000, 0, 1).program_result.is_ok());
    assert!(
        f.control(V::SetPolicy {
            policy_source: source_hash::SOURCE_HASH,
            policy_artifact: f.artifact,
            expected_revision: 1
        })
        .program_result
        .is_ok()
    );
    assert_eq!(f.state().spent, 20_000_000);
    assert_eq!(f.state().nonce, 1);
    assert!(!f.state().approved);
}

fn reject(result: &InstructionResult, code: u32) {
    assert_eq!(
        result.program_result,
        mollusk_svm::result::ProgramResult::Failure(solana_program_error::ProgramError::Custom(
            code
        ))
    );
}
#[test]
fn policy_artifact_and_immutability_are_enforced_and_recovery_is_independent() {
    let mut f = F::new();
    assert!(f.deposit(100_000_000).program_result.is_ok());
    f.accounts.get_mut(&f.policy_data).unwrap().data[12] = 1;
    reject(
        &f.control(V::Approve {
            approved: true,
            expected_revision: 0,
        }),
        111,
    );
    f.accounts.get_mut(&f.policy_data).unwrap().data[12] = 0;
    let mut s = f.state();
    s.policy_artifact[0] ^= 1;
    let mut bytes = borsh::to_vec(&s).unwrap();
    bytes.resize(STATE_BYTES, 0);
    f.accounts.get_mut(&f.vault).unwrap().data = bytes;
    reject(
        &f.control(V::Approve {
            approved: true,
            expected_revision: 0,
        }),
        105,
    );
    let mut s = f.state();
    s.policy_artifact = f.artifact;
    s.policy_source[0] ^= 1;
    let mut bytes = borsh::to_vec(&s).unwrap();
    bytes.resize(STATE_BYTES, 0);
    f.accounts.get_mut(&f.vault).unwrap().data = bytes;
    reject(
        &f.control(V::Approve {
            approved: true,
            expected_revision: 0,
        }),
        105,
    );
    f.accounts.get_mut(&f.policy).unwrap().executable = false;
    assert!(
        f.control(V::Approve {
            approved: false,
            expected_revision: 0
        })
        .program_result
        .is_ok()
    );
    let r = f.run(
        V::Withdraw {
            amount: 100_000_000,
        },
        vec![
            rw(f.vault, false),
            ro(f.owner, true),
            rw(f.tokens, false),
            rw(f.source, false),
            ro(f.mint, false),
            ro(Pubkey::new_from_array(spl_token::id().to_bytes()), false),
        ],
    );
    assert!(r.program_result.is_ok());
    assert_eq!(f.amount(f.tokens), 0);
}
#[test]
fn wrong_mint_and_vault_owned_destinations_are_rejected() {
    let mut f = F::new();
    assert!(f.deposit(100_000_000).program_result.is_ok());
    assert!(
        f.control(V::Approve {
            approved: true,
            expected_revision: 0
        })
        .program_result
        .is_ok()
    );
    let mut destination = Token::unpack(&f.accounts[&f.recipient].data).unwrap();
    destination.owner = old(f.vault);
    Token::pack(
        destination,
        &mut f.accounts.get_mut(&f.recipient).unwrap().data,
    )
    .unwrap();
    reject(&f.transfer(1, 0, 1), 108);
    let mut m = Mint::unpack(&f.accounts[&f.mint].data).unwrap();
    m.decimals = 7;
    Mint::pack(m, &mut f.accounts.get_mut(&f.mint).unwrap().data).unwrap();
    reject(&f.deposit(1), 103);
    m.decimals = 6;
    Mint::pack(m, &mut f.accounts.get_mut(&f.mint).unwrap().data).unwrap();
    f.accounts.get_mut(&f.mint).unwrap().owner = key(90);
    reject(&f.deposit(1), 103);
}
#[test]
fn minimal_native_adapter_matches_borsh_requests_and_every_kernel_decision() {
    use allowit_interface::{Evaluation, PolicyDecision, PolicyInstruction, policy};
    let f = F::new();
    let accounts: Vec<_> = f.accounts.iter().map(|(k, a)| (*k, a.clone())).collect();
    for amount in [0, 1, 25_000_000, 25_000_001, u64::MAX] {
        for spent in [0, 10, 25_000_000, u64::MAX] {
            for now in [0, 86_401, 172_800] {
                for approved in [false, true] {
                    let value = Evaluation {
                        approved,
                        amount,
                        daily_limit: 25_000_000,
                        spent,
                        spent_day: 1,
                        now,
                        binding: [42; 32],
                    };
                    let expected = policy::execute(&value.context());
                    let ix = Instruction {
                        program_id: f.policy,
                        accounts: vec![],
                        data: borsh::to_vec(&PolicyInstruction::Evaluate(value)).unwrap(),
                    };
                    let r = f.svm.process_instruction(&ix, &accounts);
                    assert!(r.program_result.is_ok());
                    let d = PolicyDecision::try_from_slice(&r.return_data).unwrap();
                    assert_eq!(d.abi, ABI_VERSION);
                    assert_eq!(d.source, source_hash::SOURCE_HASH);
                    assert_eq!(d.binding, [42; 32]);
                    match expected {
                        Ok(next) => {
                            assert_eq!(d.error, 0);
                            assert_eq!(d.next_spent, next)
                        }
                        Err(error) => {
                            assert_eq!(d.error, error as u32);
                            assert_eq!(d.next_spent, 0)
                        }
                    }
                }
            }
        }
    }
    for limit in [0, 1, 50_000_000, 50_000_001, u64::MAX] {
        let ix = Instruction {
            program_id: f.policy,
            accounts: vec![],
            data: borsh::to_vec(&PolicyInstruction::ValidateDailyLimit {
                value: limit,
                binding: [7; 32],
            })
            .unwrap(),
        };
        let r = f.svm.process_instruction(&ix, &accounts);
        assert!(r.program_result.is_ok());
        let d = PolicyDecision::try_from_slice(&r.return_data).unwrap();
        assert_eq!(d.binding, [7; 32]);
        assert_eq!(
            d.error,
            policy_api::validate_daily_limit(limit)
                .err()
                .map(|e| e as u32)
                .unwrap_or(0)
        );
    }
    let mut valid = vec![0; 74];
    valid[1] = 2;
    for data in [
        vec![],
        vec![2],
        vec![0; 73],
        vec![0; 75],
        valid,
        vec![1; 40],
        vec![1; 42],
    ] {
        let r = f.svm.process_instruction(
            &Instruction {
                program_id: f.policy,
                accounts: vec![],
                data,
            },
            &accounts,
        );
        assert_eq!(
            r.program_result,
            mollusk_svm::result::ProgramResult::Failure(
                solana_program_error::ProgramError::InvalidInstructionData
            )
        );
    }
    for metas in [
        vec![ro(f.owner, false)],
        vec![ro(f.owner, false), ro(f.executor, false)],
    ] {
        let r = f.svm.process_instruction(
            &Instruction {
                program_id: f.policy,
                accounts: metas,
                data: borsh::to_vec(&PolicyInstruction::ValidateDailyLimit {
                    value: 1,
                    binding: [0; 32],
                })
                .unwrap(),
            },
            &accounts,
        );
        assert_eq!(
            r.program_result,
            mollusk_svm::result::ProgramResult::Failure(
                solana_program_error::ProgramError::InvalidArgument
            )
        );
    }
}
