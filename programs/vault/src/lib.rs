use allowit_interface::{
    Evaluation, PolicyDecision, PolicyInstruction, VaultInstruction, VaultState, ABI_VERSION,
    STATE_BYTES, VAULT_SEED,
};
use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    entrypoint::ProgramResult,
    hash::hashv,
    instruction::Instruction,
    program::{get_return_data, invoke, invoke_signed, set_return_data},
    program_error::ProgramError,
    program_option::COption,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction, system_program,
    sysvar::Sysvar,
};
use spl_token::state::{Account as TokenAccount, AccountState, Mint};
#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

#[repr(u32)]
pub enum Error {
    UnsupportedMethod = 100,
    StaleRequest = 101,
    Overflow = 102,
    UnsupportedAsset = 103,
    NotApproved = 104,
    SourceChanged = 105,
    InsufficientFunds = 106,
    ZeroAmount = 107,
    BadRecipient = 108,
    InvalidDecision = 109,
    Unauthorized = 110,
    BadAccount = 111,
}
impl From<Error> for ProgramError {
    fn from(value: Error) -> Self {
        Self::Custom(value as u32)
    }
}
fn fail<T>(value: Error) -> Result<T, ProgramError> {
    Err(value.into())
}
fn require_signer(a: &AccountInfo) -> ProgramResult {
    if !a.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    Ok(())
}
fn load(program: &Pubkey, account: &AccountInfo) -> Result<VaultState, ProgramError> {
    if account.owner != program || !account.is_writable || account.data_len() != STATE_BYTES {
        return fail(Error::BadAccount);
    }
    let data = account.try_borrow_data()?;
    let state =
        VaultState::deserialize(&mut &data[..]).map_err(|_| ProgramError::InvalidAccountData)?;
    let (key, bump) =
        Pubkey::find_program_address(&[VAULT_SEED, &state.owner, &state.vault_id], program);
    if state.abi != ABI_VERSION || key != *account.key || bump != state.bump {
        return fail(Error::BadAccount);
    }
    Ok(state)
}
fn store(account: &AccountInfo, state: &VaultState) -> ProgramResult {
    let mut data = account.try_borrow_mut_data()?;
    data.fill(0);
    state
        .serialize(&mut &mut data[..])
        .map_err(|_| ProgramError::AccountDataTooSmall)
}
fn owner(state: &VaultState, actor: &AccountInfo) -> ProgramResult {
    require_signer(actor)?;
    if state.owner != actor.key.to_bytes() {
        return fail(Error::Unauthorized);
    }
    Ok(())
}
fn revision(state: &VaultState, expected: u64) -> ProgramResult {
    if state.revision != expected {
        return fail(Error::StaleRequest);
    }
    Ok(())
}
fn changed(state: &mut VaultState) -> ProgramResult {
    state.revision = state.revision.checked_add(1).ok_or(Error::Overflow)?;
    Ok(())
}
fn mint(account: &AccountInfo) -> Result<Mint, ProgramError> {
    if account.owner != &spl_token::id() {
        return fail(Error::UnsupportedAsset);
    }
    let value = Mint::unpack(&account.try_borrow_data()?)?;
    if !value.is_initialized || value.decimals != 6 {
        return fail(Error::UnsupportedAsset);
    }
    Ok(value)
}
fn token(account: &AccountInfo, asset: &Pubkey) -> Result<TokenAccount, ProgramError> {
    if account.owner != &spl_token::id() {
        return fail(Error::BadAccount);
    }
    let value = TokenAccount::unpack(&account.try_borrow_data()?)?;
    if value.mint != *asset || value.state != AccountState::Initialized {
        return fail(Error::UnsupportedAsset);
    }
    Ok(value)
}
fn vault_token(
    state: &VaultState,
    vault: &AccountInfo,
    account: &AccountInfo,
) -> Result<TokenAccount, ProgramError> {
    if state.token_account != account.key.to_bytes() {
        return fail(Error::BadAccount);
    }
    let value = token(account, &Pubkey::new_from_array(state.mint))?;
    if value.owner != *vault.key
        || value.delegate != COption::None
        || value.close_authority != COption::None
    {
        return fail(Error::BadAccount);
    }
    Ok(value)
}
fn verify_artifact(
    policy: &AccountInfo,
    data_account: &AccountInfo,
    expected: &[u8; 32],
) -> ProgramResult {
    let loader = solana_program::bpf_loader_upgradeable::id();
    if !policy.executable || policy.owner != &loader || data_account.owner != &loader {
        return fail(Error::BadAccount);
    }
    let program = policy.try_borrow_data()?;
    let data = data_account.try_borrow_data()?;
    // Loader-v3: Program tag=2, ProgramData tag=3; immutable authority Option at byte 12.
    if program.len() != 36
        || program[..4] != 2u32.to_le_bytes()
        || program[4..36] != data_account.key.to_bytes()
        || data.len() <= 45
        || data[..4] != 3u32.to_le_bytes()
        || data[12] != 0
    {
        return fail(Error::BadAccount);
    }
    let (address, _) = Pubkey::find_program_address(&[policy.key.as_ref()], &loader);
    if address != *data_account.key || hashv(&[&data[45..]]).to_bytes() != *expected {
        return fail(Error::SourceChanged);
    }
    Ok(())
}
fn evaluate(
    policy: &AccountInfo,
    instruction: PolicyInstruction,
    source: &[u8; 32],
    binding: &[u8; 32],
) -> Result<u64, ProgramError> {
    if !policy.executable {
        return fail(Error::BadAccount);
    }
    set_return_data(&[]);
    invoke(
        &Instruction {
            program_id: *policy.key,
            accounts: vec![],
            data: borsh::to_vec(&instruction).map_err(|_| ProgramError::InvalidInstructionData)?,
        },
        &[policy.clone()],
    )?;
    let (producer, data) = get_return_data().ok_or(Error::InvalidDecision)?;
    if producer != *policy.key {
        return fail(Error::InvalidDecision);
    }
    let result =
        PolicyDecision::try_from_slice(&data).map_err(|_| ProgramError::InvalidInstructionData)?;
    if result.abi != ABI_VERSION || &result.binding != binding {
        return fail(Error::InvalidDecision);
    }
    if &result.source != source {
        return fail(Error::SourceChanged);
    }
    if result.error != 0 {
        return Err(ProgramError::Custom(1000 + result.error.min(999)));
    }
    Ok(result.next_spent)
}
fn bound_limit(
    vault: &AccountInfo,
    policy: &AccountInfo,
    data: &AccountInfo,
    artifact: &[u8; 32],
    source: &[u8; 32],
    value: u64,
) -> ProgramResult {
    allowit_interface::policy::validate_daily_limit(value)
        .map_err(|e| ProgramError::Custom(1000 + e as u32))?;
    verify_artifact(policy, data, artifact)?;
    let binding = hashv(&[b"allowit-tune-v1", vault.key.as_ref(), &value.to_le_bytes()]).to_bytes();
    evaluate(
        policy,
        PolicyInstruction::ValidateDailyLimit { value, binding },
        source,
        &binding,
    )?;
    Ok(())
}
fn clock() -> Result<u64, ProgramError> {
    u64::try_from(Clock::get()?.unix_timestamp).map_err(|_| Error::InvalidDecision.into())
}

pub fn process_instruction(
    program: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let instruction =
        VaultInstruction::try_from_slice(data).map_err(|_| ProgramError::InvalidInstructionData)?;
    if let VaultInstruction::Unsupported { .. } = instruction {
        return fail(Error::UnsupportedMethod);
    }
    let iter = &mut accounts.iter();
    let vault = next_account_info(iter)?;
    let actor = next_account_info(iter)?;
    if let VaultInstruction::Initialize {
        vault_id,
        policy_source,
        policy_artifact,
        daily_limit,
    } = instruction
    {
        let asset = next_account_info(iter)?;
        let tokens = next_account_info(iter)?;
        let policy = next_account_info(iter)?;
        let executor = next_account_info(iter)?;
        let system = next_account_info(iter)?;
        let policy_data = next_account_info(iter)?;
        require_signer(actor)?;
        if !actor.is_writable || !vault.is_writable || system.key != &system_program::id() {
            return fail(Error::BadAccount);
        }
        let (address, bump) =
            Pubkey::find_program_address(&[VAULT_SEED, actor.key.as_ref(), &vault_id], program);
        if *vault.key != address || !vault.data_is_empty() || vault.owner != &system_program::id() {
            return fail(Error::BadAccount);
        }
        mint(asset)?;
        let value = token(tokens, asset.key)?;
        if value.owner != address
            || value.delegate != COption::None
            || value.close_authority != COption::None
        {
            return fail(Error::BadAccount);
        }
        bound_limit(
            vault,
            policy,
            policy_data,
            &policy_artifact,
            &policy_source,
            daily_limit,
        )?;
        let required = Rent::get()?.minimum_balance(STATE_BYTES).max(1);
        let bump_seed = [bump];
        let seeds = &[VAULT_SEED, actor.key.as_ref(), &vault_id, &bump_seed];
        if vault.lamports() == 0 {
            invoke_signed(
                &system_instruction::create_account(
                    actor.key,
                    vault.key,
                    required,
                    STATE_BYTES as u64,
                    program,
                ),
                &[actor.clone(), vault.clone(), system.clone()],
                &[seeds],
            )?;
        } else {
            let top_up = required.saturating_sub(vault.lamports());
            if top_up > 0 {
                invoke(
                    &system_instruction::transfer(actor.key, vault.key, top_up),
                    &[actor.clone(), vault.clone(), system.clone()],
                )?;
            }
            invoke_signed(
                &system_instruction::allocate(vault.key, STATE_BYTES as u64),
                &[vault.clone(), system.clone()],
                &[seeds],
            )?;
            invoke_signed(
                &system_instruction::assign(vault.key, program),
                &[vault.clone(), system.clone()],
                &[seeds],
            )?;
        }
        return store(
            vault,
            &VaultState {
                abi: ABI_VERSION,
                bump,
                owner: actor.key.to_bytes(),
                executor: executor.key.to_bytes(),
                mint: asset.key.to_bytes(),
                token_account: tokens.key.to_bytes(),
                policy: policy.key.to_bytes(),
                policy_source,
                policy_artifact,
                vault_id,
                daily_limit,
                spent: 0,
                spent_day: clock()? / 86_400,
                nonce: 0,
                revision: 0,
                approved: false,
            },
        );
    }
    let mut state = load(program, vault)?;
    match instruction {
        VaultInstruction::Approve {
            approved,
            expected_revision,
        } => {
            owner(&state, actor)?;
            revision(&state, expected_revision)?;
            let policy = next_account_info(iter)?;
            if state.policy != policy.key.to_bytes() {
                return fail(Error::BadAccount);
            }
            if approved {
                let policy_data = next_account_info(iter)?;
                bound_limit(
                    vault,
                    policy,
                    policy_data,
                    &state.policy_artifact,
                    &state.policy_source,
                    state.daily_limit,
                )?;
            }
            state.approved = approved;
            changed(&mut state)?;
            store(vault, &state)
        }
        VaultInstruction::SetDailyLimit {
            value,
            expected_revision,
        } => {
            owner(&state, actor)?;
            revision(&state, expected_revision)?;
            let policy = next_account_info(iter)?;
            if state.policy != policy.key.to_bytes() {
                return fail(Error::BadAccount);
            }
            let policy_data = next_account_info(iter)?;
            bound_limit(
                vault,
                policy,
                policy_data,
                &state.policy_artifact,
                &state.policy_source,
                value,
            )?;
            state.daily_limit = value;
            changed(&mut state)?;
            store(vault, &state)
        }
        VaultInstruction::SetPolicy {
            policy_source,
            policy_artifact,
            expected_revision,
        } => {
            owner(&state, actor)?;
            revision(&state, expected_revision)?;
            let policy = next_account_info(iter)?;
            let policy_data = next_account_info(iter)?;
            bound_limit(
                vault,
                policy,
                policy_data,
                &policy_artifact,
                &policy_source,
                state.daily_limit,
            )?;
            state.policy = policy.key.to_bytes();
            state.policy_source = policy_source;
            state.policy_artifact = policy_artifact;
            state.approved = false;
            changed(&mut state)?;
            store(vault, &state)
        }
        VaultInstruction::Deposit { amount }
        | VaultInstruction::Withdraw { amount }
        | VaultInstruction::Transfer { amount, .. } => {
            if amount == 0 {
                return fail(Error::ZeroAmount);
            }
            let source = next_account_info(iter)?;
            let destination = next_account_info(iter)?;
            let asset = next_account_info(iter)?;
            let token_program = next_account_info(iter)?;
            if asset.key.to_bytes() != state.mint
                || token_program.key != &spl_token::id()
                || source.key == destination.key
            {
                return fail(Error::BadAccount);
            }
            mint(asset)?;
            if let VaultInstruction::Deposit { .. } = instruction {
                require_signer(actor)?;
                let source_data = token(source, asset.key)?;
                if source_data.owner != *actor.key {
                    return fail(Error::Unauthorized);
                }
                vault_token(&state, vault, destination)?;
                return invoke(
                    &spl_token::instruction::transfer_checked(
                        token_program.key,
                        source.key,
                        asset.key,
                        destination.key,
                        actor.key,
                        &[],
                        amount,
                        6,
                    )?,
                    &[
                        source.clone(),
                        asset.clone(),
                        destination.clone(),
                        actor.clone(),
                        token_program.clone(),
                    ],
                );
            }
            let balance = vault_token(&state, vault, source)?.amount;
            if token(destination, asset.key)?.owner == *vault.key {
                return fail(Error::BadRecipient);
            }
            if balance < amount {
                return fail(Error::InsufficientFunds);
            }
            if let VaultInstruction::Transfer {
                nonce,
                expected_revision,
                ..
            } = instruction
            {
                require_signer(actor)?;
                if state.executor != actor.key.to_bytes() {
                    return fail(Error::Unauthorized);
                }
                if !state.approved {
                    return fail(Error::NotApproved);
                }
                revision(&state, expected_revision)?;
                if state.nonce != nonce {
                    return fail(Error::StaleRequest);
                }
                let policy = next_account_info(iter)?;
                if state.policy != policy.key.to_bytes() {
                    return fail(Error::BadAccount);
                }
                let now = clock()?;
                let day = now / 86_400;
                if day < state.spent_day {
                    return fail(Error::InvalidDecision);
                }
                let expected = (if day == state.spent_day {
                    state.spent
                } else {
                    0
                })
                .checked_add(amount)
                .ok_or(Error::Overflow)?;
                let binding = hashv(&[
                    b"allowit-transfer-v1",
                    program.as_ref(),
                    vault.key.as_ref(),
                    actor.key.as_ref(),
                    source.key.as_ref(),
                    destination.key.as_ref(),
                    asset.key.as_ref(),
                    policy.key.as_ref(),
                    &state.policy_source,
                    &amount.to_le_bytes(),
                    &nonce.to_le_bytes(),
                    &expected_revision.to_le_bytes(),
                    &now.to_le_bytes(),
                    &state.daily_limit.to_le_bytes(),
                    &state.spent.to_le_bytes(),
                    &state.spent_day.to_le_bytes(),
                ])
                .to_bytes();
                let value = Evaluation {
                    approved: state.approved,
                    amount,
                    daily_limit: state.daily_limit,
                    spent: state.spent,
                    spent_day: state.spent_day,
                    now,
                    binding,
                };
                let policy_data = next_account_info(iter)?;
                verify_artifact(policy, policy_data, &state.policy_artifact)?;
                allowit_interface::policy::validate_daily_limit(state.daily_limit)
                    .map_err(|e| ProgramError::Custom(1000 + e as u32))?;
                let next = evaluate(
                    policy,
                    PolicyInstruction::Evaluate(value),
                    &state.policy_source,
                    &binding,
                )?;
                if next != expected || next > state.daily_limit {
                    return fail(Error::InvalidDecision);
                }
                state.spent = next;
                state.spent_day = day;
                state.nonce = state.nonce.checked_add(1).ok_or(Error::Overflow)?;
                store(vault, &state)?;
            } else {
                owner(&state, actor)?;
            }
            let bump = [state.bump];
            let seeds = &[VAULT_SEED, &state.owner, &state.vault_id, &bump];
            invoke_signed(
                &spl_token::instruction::transfer_checked(
                    token_program.key,
                    source.key,
                    asset.key,
                    destination.key,
                    vault.key,
                    &[],
                    amount,
                    6,
                )?,
                &[
                    source.clone(),
                    asset.clone(),
                    destination.clone(),
                    vault.clone(),
                    token_program.clone(),
                ],
                &[seeds],
            )?;
            // Transaction finality plus exact token CPI is settlement evidence; return state is informational.
            set_return_data(&borsh::to_vec(&state).map_err(|_| ProgramError::InvalidAccountData)?);
            Ok(())
        }
        _ => fail(Error::UnsupportedMethod),
    }
}
