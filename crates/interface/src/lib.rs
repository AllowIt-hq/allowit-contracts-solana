use borsh::{BorshDeserialize, BorshSerialize};
#[path = "../../../policy/policy.rs"]
pub mod policy;
#[path = "../../../policy/policy_api.rs"]
pub mod policy_api;

pub const ABI_VERSION: u8 = 1;
pub const VAULT_SEED: &[u8] = b"allowit-vault-v1";
pub const STATE_BYTES: usize = 320;
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct VaultState {
    pub abi: u8,
    pub bump: u8,
    pub owner: [u8; 32],
    pub executor: [u8; 32],
    pub mint: [u8; 32],
    pub token_account: [u8; 32],
    pub policy: [u8; 32],
    pub policy_source: [u8; 32],
    pub policy_artifact: [u8; 32],
    pub vault_id: [u8; 32],
    pub daily_limit: u64,
    pub spent: u64,
    pub spent_day: u64,
    pub nonce: u64,
    pub revision: u64,
    pub approved: bool,
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize)]
pub enum VaultInstruction {
    Initialize {
        vault_id: [u8; 32],
        policy_source: [u8; 32],
        policy_artifact: [u8; 32],
        daily_limit: u64,
    },
    Deposit {
        amount: u64,
    },
    Approve {
        approved: bool,
        expected_revision: u64,
    },
    SetDailyLimit {
        value: u64,
        expected_revision: u64,
    },
    Transfer {
        amount: u64,
        nonce: u64,
        expected_revision: u64,
    },
    Withdraw {
        amount: u64,
    },
    SetPolicy {
        policy_source: [u8; 32],
        policy_artifact: [u8; 32],
        expected_revision: u64,
    },
    Unsupported {
        method: u32,
    },
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct Evaluation {
    pub approved: bool,
    pub amount: u64,
    pub daily_limit: u64,
    pub spent: u64,
    pub spent_day: u64,
    pub now: u64,
    pub binding: [u8; 32],
}
impl Evaluation {
    pub fn context(&self) -> policy_api::Context {
        policy_api::Context {
            approved: self.approved,
            amount: self.amount,
            daily_limit: self.daily_limit,
            spent: self.spent,
            spent_day: self.spent_day,
            now: self.now,
        }
    }
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize)]
pub enum PolicyInstruction {
    Evaluate(Evaluation),
    ValidateDailyLimit { value: u64, binding: [u8; 32] },
}
#[derive(Clone, Debug, BorshDeserialize, BorshSerialize)]
pub struct PolicyDecision {
    pub abi: u8,
    pub error: u32,
    pub binding: [u8; 32],
    pub source: [u8; 32],
    pub next_spent: u64,
}
