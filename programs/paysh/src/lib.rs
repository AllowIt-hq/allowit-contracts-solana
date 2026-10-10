//! Separate upgradeable bounded execution profile; legacy immutable policy ABI is unchanged.
use allowit_paysh_interface::*;
use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    entrypoint::ProgramResult,
    hash::{hash, hashv},
    instruction::{AccountMeta, Instruction as SolInstruction},
    program::{invoke, invoke_signed, set_return_data},
    program_error::ProgramError,
    program_option::COption,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction, system_program,
    sysvar::{
        instructions::{load_current_index_checked, load_instruction_at_checked},
        Sysvar,
    },
};
use spl_token::state::{Account as Token, AccountState, Mint};
#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum Error {
    BadAccount = 200,
    Unauthorized = 201,
    Expired = 202,
    Replay = 203,
    Budget = 204,
    Overflow = 205,
    BadSignature = 206,
    BadPool = 207,
    InvalidRequest = 208,
    Paused = 209,
    Slippage = 210,
    InvalidTreasury = 211,
}
impl From<Error> for ProgramError {
    fn from(e: Error) -> Self {
        Self::Custom(e as u32)
    }
}
fn ensure(v: bool, e: Error) -> ProgramResult {
    if v {
        Ok(())
    } else {
        Err(e.into())
    }
}
fn save<T: BorshSerialize>(a: &AccountInfo, v: &T) -> ProgramResult {
    let mut b = a.try_borrow_mut_data()?;
    b.fill(0);
    v.serialize(&mut &mut b[..])
        .map_err(|_| ProgramError::AccountDataTooSmall)
}
fn read<T: BorshDeserialize>(a: &AccountInfo) -> Result<T, ProgramError> {
    T::deserialize(&mut &a.try_borrow_data()?[..]).map_err(|_| ProgramError::InvalidAccountData)
}
// Keep the immutable config on the heap rather than sharing the execute frame
// with the request, CPI account vectors, and serialized instruction buffers.
#[inline(never)]
fn policy_state(a: &AccountInfo) -> Result<Box<Policy>, ProgramError> {
    Ok(Box::new(read(a)?))
}
fn address(a: &AccountInfo, k: &[u8; 32]) -> ProgramResult {
    ensure(a.key.to_bytes() == *k, Error::BadAccount)
}
fn token(a: &AccountInfo, m: &Pubkey) -> Result<Token, ProgramError> {
    ensure(a.owner == &spl_token::id(), Error::BadAccount)?;
    let t = Token::unpack(&a.try_borrow_data()?)?;
    ensure(
        t.mint == *m && t.state == AccountState::Initialized,
        Error::BadAccount,
    )?;
    Ok(t)
}
fn vault_token(a: &AccountInfo, m: &Pubkey, p: &Pubkey) -> Result<Token, ProgramError> {
    let t = token(a, m)?;
    ensure(
        t.owner == *p && t.delegate == COption::None && t.close_authority == COption::None,
        Error::BadAccount,
    )?;
    Ok(t)
}
// Owner recovery may leave frozen USDC in custody while returning native SOL.
// Frozen accounts still require the exact mint, authority and valid SPL state.
fn recovery_token(
    a: &AccountInfo,
    mint: &Pubkey,
    owner: &Pubkey,
) -> Result<Option<Token>, ProgramError> {
    if a.owner == &system_program::id() && a.data_is_empty() {
        return Ok(None);
    }
    ensure(a.owner == &spl_token::id(), Error::BadAccount)?;
    let t = Token::unpack(&a.try_borrow_data()?)?;
    ensure(
        t.mint == *mint
            && t.owner == *owner
            && matches!(t.state, AccountState::Initialized | AccountState::Frozen),
        Error::BadAccount,
    )?;
    Ok(Some(t))
}
fn add(v: u64, n: u64, limit: u64) -> Result<u64, ProgramError> {
    let x = v.checked_add(n).ok_or(Error::Overflow)?;
    ensure(x <= limit, Error::Budget)?;
    Ok(x)
}
fn create<'a>(
    program: &Pubkey,
    a: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    size: usize,
    seeds: &[&[u8]],
) -> ProgramResult {
    ensure(
        a.is_writable
            && payer.is_signer
            && payer.is_writable
            && system.key == &system_program::id()
            && a.owner == &system_program::id()
            && a.data_is_empty(),
        Error::BadAccount,
    )?;
    let rent = Rent::get()?.minimum_balance(size).max(1);
    if a.lamports() == 0 {
        invoke_signed(
            &system_instruction::create_account(payer.key, a.key, rent, size as u64, program),
            &[payer.clone(), a.clone(), system.clone()],
            &[seeds],
        )
    } else {
        let top = rent.saturating_sub(a.lamports());
        if top > 0 {
            invoke(
                &system_instruction::transfer(payer.key, a.key, top),
                &[payer.clone(), a.clone(), system.clone()],
            )?;
        }
        invoke_signed(
            &system_instruction::allocate(a.key, size as u64),
            &[a.clone(), system.clone()],
            &[seeds],
        )?;
        invoke_signed(
            &system_instruction::assign(a.key, program),
            &[a.clone(), system.clone()],
            &[seeds],
        )
    }
}
// Keep cryptographic work in the native SHA-256 syscall onchain. The pure SDK
// helper uses software SHA-256 and produces byte-identical signed messages.
fn signed_message(request: &Request) -> Result<Vec<u8>, ProgramError> {
    let bytes = borsh::to_vec(request).map_err(|_| ProgramError::InvalidInstructionData)?;
    let mut message = DOMAIN.to_vec();
    message.extend(hash(&bytes).to_bytes());
    Ok(message)
}
// Match the SDK's v2 membership encoding while keeping SHA-256 in the native
// syscall. The owner commits the root at initialization; every signed request
// proves one service scoped to the exact network, mint and vendor.
fn service_allowed(config: &Config, request: &Request) -> ProgramResult {
    ensure(
        config.service_allowlist_root != [0; 32]
            && request.service_hash != [0; 32]
            && request.service_proof.len() <= allowlist::MAX_PROOF_DEPTH,
        Error::Unauthorized,
    )?;
    let mut value = hashv(&[
        b"allowit-paysh-leaf-v2\0",
        &config.network,
        &config.usdc_mint,
        &config.vendor_usdc,
        &request.service_hash,
    ])
    .to_bytes();
    for sibling in &request.service_proof {
        let (left, right) = if value <= *sibling {
            (&value, sibling)
        } else {
            (sibling, &value)
        };
        value = hashv(&[b"allowit-paysh-node-v2\0", left, right]).to_bytes();
    }
    ensure(value == config.service_allowlist_root, Error::Unauthorized)
}
fn approval(
    program: &Pubkey,
    sys: &AccountInfo,
    evaluator: &[u8; 32],
    request: &Request,
) -> ProgramResult {
    let index = load_current_index_checked(sys)? as usize;
    ensure(index > 0, Error::BadSignature)?;
    // Top-level only. A CPI wrapper must not borrow another instruction's approval.
    ensure(
        load_instruction_at_checked(index, sys)?.program_id == *program,
        Error::BadSignature,
    )?;
    let ix = load_instruction_at_checked(index - 1, sys)?;
    ensure(
        ix.program_id == solana_program::ed25519_program::id() && ix.accounts.is_empty(),
        Error::BadSignature,
    )?;
    let d = &ix.data;
    ensure(d.len() >= 16 && d[0] == 1 && d[1] == 0, Error::BadSignature)?;
    let u = |p: usize| -> u16 { u16::from_le_bytes([d[p], d[p + 1]]) };
    // Accept only the canonical self-contained layout (key32,signature64,message).
    let message = signed_message(request)?;
    ensure(
        u(2) == 48
            && u(4) == u16::MAX
            && u(6) == 16
            && u(8) == u16::MAX
            && u(10) == 112
            && u(12) as usize == message.len()
            && u(14) == u16::MAX
            && d.len() == 112 + message.len(),
        Error::BadSignature,
    )?;
    ensure(
        &d[16..48] == evaluator && d[112..] == message,
        Error::BadSignature,
    )
}
fn native_transfer<'a>(
    from: &AccountInfo<'a>,
    to: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    amount: u64,
    seeds: &[&[u8]],
) -> ProgramResult {
    ensure(
        from.owner == &system_program::id()
            && from.data_is_empty()
            && from.is_writable
            && to.is_writable
            && from.key != to.key,
        Error::BadAccount,
    )?;
    invoke_signed(
        &system_instruction::transfer(from.key, to.key, amount),
        &[from.clone(), to.clone(), system.clone()],
        &[seeds],
    )
}
fn sol_vault(program: &Pubkey, policy: &AccountInfo, a: &AccountInfo) -> Result<u8, ProgramError> {
    let (k, bump) = Pubkey::find_program_address(&[SOL_SEED, policy.key.as_ref()], program);
    ensure(
        *a.key == k && a.owner == &system_program::id() && a.data_is_empty(),
        Error::BadAccount,
    )?;
    Ok(bump)
}
fn pool(c: &Config, accounts: &[AccountInfo]) -> ProgramResult {
    ensure(accounts.len() == 5, Error::BadPool)?;
    let expected = [
        c.pool.program,
        c.pool.state,
        c.pool.wsol,
        c.pool.usdc,
        c.pool.oracle,
    ];
    for (a, k) in accounts.iter().zip(expected) {
        address(a, &k)?;
    }
    let [program, state, a, b, oracle] = accounts else {
        return Err(Error::BadPool.into());
    };
    ensure(
        program.executable && state.owner == program.key,
        Error::BadPool,
    )?;
    let d = state.try_borrow_data()?;
    let discriminator = hash(b"account:Whirlpool").to_bytes();
    ensure(
        d.len() == 653 && d[..8] == discriminator[..8],
        Error::BadPool,
    )?;
    ensure(
        d[101..133] == spl_token::native_mint::id().to_bytes()
            && d[133..165] == a.key.to_bytes()
            && d[181..213] == c.usdc_mint
            && d[213..245] == b.key.to_bytes(),
        Error::BadPool,
    )?;
    // Static fee tier only; adaptive fee oracle updates require a separate profile.
    ensure(d[43..45] == d[41..43], Error::BadPool)?;
    let derived = Pubkey::create_program_address(
        &[
            b"whirlpool",
            &d[8..40],
            &d[101..133],
            &d[181..213],
            &d[43..45],
            &[d[40]],
        ],
        program.key,
    )
    .map_err(|_| Error::BadPool)?;
    ensure(derived == *state.key, Error::BadPool)?;
    let fee = u16::from_le_bytes(d[45..47].try_into().unwrap()) as u64;
    ensure(
        fee <= c.max_pool_fee_bps * 100 && u16::from_le_bytes(d[41..43].try_into().unwrap()) > 0,
        Error::BadPool,
    )?;
    let oracle_key = Pubkey::find_program_address(&[b"oracle", state.key.as_ref()], program.key).0;
    ensure(oracle_key == *oracle.key, Error::BadPool)?;
    ensure(
        token(a, &spl_token::native_mint::id())?.owner == *state.key
            && token(b, &Pubkey::new_from_array(c.usdc_mint))?.owner == *state.key,
        Error::BadPool,
    )
}
fn swap_pool(
    c: &Config,
    accounts: &[AccountInfo],
    sqrt_price_limit: u128,
    tick_arrays: &[[u8; 32]; 3],
) -> ProgramResult {
    ensure(accounts.len() == 8, Error::BadPool)?;
    pool(c, &accounts[..5])?;
    let program = &accounts[0];
    let state = &accounts[1];
    let d = state.try_borrow_data()?;
    let current = u128::from_le_bytes(d[65..81].try_into().unwrap());
    ensure(
        sqrt_price_limit >= 4_295_048_016 && sqrt_price_limit < current,
        Error::Slippage,
    )?;
    let spacing = i32::from(u16::from_le_bytes(d[41..43].try_into().unwrap()));
    let current_tick = i32::from_le_bytes(d[81..85].try_into().unwrap());
    let width = spacing.checked_mul(88).ok_or(Error::Overflow)?;
    let expected_first = current_tick.div_euclid(width) * width;
    let disc = hash(b"account:TickArray").to_bytes();
    for (i, a) in accounts[5..].iter().enumerate() {
        address(a, &tick_arrays[i])?;
        ensure(a.owner == program.key, Error::BadPool)?;
        let td = a.try_borrow_data()?;
        ensure(
            td.len() == 9988 && td[..8] == disc[..8] && td[9956..9988] == state.key.to_bytes(),
            Error::BadPool,
        )?;
        let start = i32::from_le_bytes(td[8..12].try_into().unwrap());
        // A-to-B input swaps traverse contiguous decreasing arrays, starting at current array.
        ensure(start == expected_first - (i as i32) * width, Error::BadPool)?;
        let key = Pubkey::find_program_address(
            &[
                b"tick_array",
                state.key.as_ref(),
                start.to_string().as_bytes(),
            ],
            program.key,
        )
        .0;
        ensure(key == *a.key, Error::BadPool)?;
    }
    Ok(())
}

#[inline(never)]
fn owner_withdraw<'a>(
    program: &Pubkey,
    policy: &AccountInfo<'a>,
    actor: &AccountInfo<'a>,
    accounts: &[AccountInfo<'a>],
    p: &mut Policy,
) -> ProgramResult {
    let mut iter = accounts.iter();
    ensure(
        actor.is_signer && actor.is_writable && actor.key.to_bytes() == p.owner,
        Error::Unauthorized,
    )?;
    let native = next_account_info(&mut iter)?;
    let usdc = next_account_info(&mut iter)?;
    let wsol = next_account_info(&mut iter)?;
    let destination = next_account_info(&mut iter)?;
    let tokens = next_account_info(&mut iter)?;
    let system = next_account_info(&mut iter)?;
    ensure(
        iter.as_slice().is_empty()
            && tokens.key == &spl_token::id()
            && system.key == &system_program::id(),
        Error::BadAccount,
    )?;
    let native_bump = [sol_vault(program, policy, native)?];
    address(usdc, &p.config.vault_usdc)?;
    address(wsol, &p.config.vault_wsol)?;
    let mint = Pubkey::new_from_array(p.config.usdc_mint);
    let associated = solana_program::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
    let ata = Pubkey::find_program_address(
        &[actor.key.as_ref(), spl_token::id().as_ref(), mint.as_ref()],
        &associated,
    )
    .0;
    ensure(*destination.key == ata, Error::BadAccount)?;
    let destination_ready = recovery_token(destination, &mint, actor.key)?
        .is_some_and(|t| t.state == AccountState::Initialized);
    let source = recovery_token(usdc, &mint, policy.key)?;
    if let Some(t) = &source {
        ensure(
            t.delegate == COption::None && t.close_authority == COption::None,
            Error::BadAccount,
        )?;
    }
    let close_usdc = source.as_ref().is_some_and(|t| {
        t.state == AccountState::Initialized && (t.amount == 0 || destination_ready)
    });
    let amount = if close_usdc {
        source.unwrap().amount
    } else {
        0
    };
    let close_wsol = !(wsol.owner == &system_program::id() && wsol.data_is_empty());
    if close_wsol {
        vault_token(wsol, &spl_token::native_mint::id(), policy.key)?;
    }
    let bump = [p.bump];
    let seeds: &[&[u8]] = &[POLICY_SEED, &p.owner, &p.config.instance_id, &bump];
    if amount > 0 {
        invoke_signed(
            &spl_token::instruction::transfer(
                tokens.key,
                usdc.key,
                destination.key,
                policy.key,
                &[],
                amount,
            )?,
            &[
                usdc.clone(),
                destination.clone(),
                policy.clone(),
                tokens.clone(),
            ],
            &[seeds],
        )?;
    }
    if close_usdc {
        invoke_signed(
            &spl_token::instruction::close_account(
                tokens.key,
                usdc.key,
                actor.key,
                policy.key,
                &[],
            )?,
            &[usdc.clone(), actor.clone(), policy.clone(), tokens.clone()],
            &[seeds],
        )?;
    }
    if close_wsol {
        invoke_signed(
            &spl_token::instruction::close_account(
                tokens.key,
                wsol.key,
                actor.key,
                policy.key,
                &[],
            )?,
            &[wsol.clone(), actor.clone(), policy.clone(), tokens.clone()],
            &[seeds],
        )?;
    }
    if native.lamports() > 0 {
        native_transfer(
            native,
            actor,
            system,
            native.lamports(),
            &[SOL_SEED, policy.key.as_ref(), &native_bump],
        )?;
    }
    p.paused = true;
    p.version = 2;
    save(policy, p)
}
pub fn process_instruction(
    program: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let ix = Instruction::try_from_slice(data).map_err(|_| ProgramError::InvalidInstructionData)?;
    let iter = &mut accounts.iter();
    let policy = next_account_info(iter)?;
    let actor = next_account_info(iter)?;
    if let Instruction::Initialize(c) = ix {
        let usdc = next_account_info(iter)?;
        let wsol = next_account_info(iter)?;
        let mint = next_account_info(iter)?;
        let vendor = next_account_info(iter)?;
        let treasury = next_account_info(iter)?;
        let system = next_account_info(iter)?;
        let native = next_account_info(iter)?;
        sol_vault(program, policy, native)?;
        ensure(
            actor.is_signer
                && c.period_seconds > 0
                && c.max_age_seconds > 0
                && c.max_age_seconds <= 60
                && c.max_age_slots > 0
                && c.max_age_slots <= 180
                && c.service_fee_lamports > 0
                && c.max_pool_fee_bps <= 100
                && c.min_usdc_per_sol > 0
                && c.max_swap_lamports_per_call <= c.max_swap_lamports_per_period
                && c.max_swap_lamports_per_period <= c.max_total_swap_lamports
                && c.max_total_swap_lamports <= c.allocation_lamports
                && c.service_fee_lamports <= c.max_fee_lamports_per_period
                && c.service_fee_lamports <= c.max_sol_debits_per_period
                && c.max_sol_debits_per_period <= c.allocation_lamports
                && c.policy_expires_timestamp > Clock::get()?.unix_timestamp,
            Error::InvalidRequest,
        )?;
        let (p, bump) = Pubkey::find_program_address(
            &[POLICY_SEED, actor.key.as_ref(), &c.instance_id],
            program,
        );
        ensure(p == *policy.key, Error::BadAccount)?;
        address(usdc, &c.vault_usdc)?;
        address(wsol, &c.vault_wsol)?;
        address(mint, &c.usdc_mint)?;
        address(vendor, &c.vendor_usdc)?;
        address(treasury, &c.treasury)?;
        ensure(treasury.key != native.key, Error::InvalidTreasury)?;
        ensure(
            treasury.key != policy.key
                && treasury.key != usdc.key
                && treasury.key != wsol.key
                && c.evaluator != [0; 32]
                && c.network != [0; 32]
                && c.module_digest != [0; 32],
            Error::BadAccount,
        )?;
        ensure(mint.owner == &spl_token::id(), Error::BadAccount)?;
        let m = Mint::unpack(&mint.try_borrow_data()?)?;
        ensure(m.decimals == 6 && m.is_initialized, Error::BadAccount)?;
        vault_token(usdc, mint.key, policy.key)?;
        let w = vault_token(wsol, &spl_token::native_mint::id(), policy.key)?;
        ensure(w.is_native != COption::None, Error::BadAccount)?;
        ensure(
            token(vendor, mint.key)?.owner != *policy.key,
            Error::BadAccount,
        )?;
        pool(&c, iter.as_slice())?;
        let bumpb = [bump];
        create(
            program,
            policy,
            actor,
            system,
            POLICY_BYTES,
            &[POLICY_SEED, actor.key.as_ref(), &c.instance_id, &bumpb],
        )?;
        return save(
            policy,
            &Policy {
                version: 1,
                bump,
                owner: actor.key.to_bytes(),
                paused: false,
                total_swap_lamports: 0,
                total_sol_debits: 0,
                config: c,
            },
        );
    }
    ensure(
        policy.owner == program && policy.is_writable && policy.data_len() == POLICY_BYTES,
        Error::BadAccount,
    )?;
    let mut p = policy_state(policy)?;
    let (k, bump) =
        Pubkey::find_program_address(&[POLICY_SEED, &p.owner, &p.config.instance_id], program);
    ensure(
        k == *policy.key && bump == p.bump && matches!(p.version, 1 | 2),
        Error::BadAccount,
    )?;
    match ix {
        Instruction::Pause(value) => {
            ensure(
                actor.is_signer && actor.key.to_bytes() == p.owner,
                Error::Unauthorized,
            )?;
            ensure(iter.as_slice().is_empty(), Error::BadAccount)?;
            ensure(value || p.version == 1, Error::Paused)?;
            p.paused = value;
            save(policy, &p)
        }
        Instruction::Withdraw => owner_withdraw(program, policy, actor, iter.as_slice(), &mut p),
        Instruction::Execute(r) => {
            ensure(p.version == 1 && !p.paused, Error::Paused)?;
            ensure(
                actor.is_signer && (!p.config.owner_only || actor.key.to_bytes() == p.owner),
                Error::Unauthorized,
            )?;
            let receipt = next_account_info(iter)?;
            let budget = next_account_info(iter)?;
            let treasury = next_account_info(iter)?;
            let usdc = next_account_info(iter)?;
            let wsol = next_account_info(iter)?;
            let vendor = next_account_info(iter)?;
            let mint = next_account_info(iter)?;
            let tokens = next_account_info(iter)?;
            let system = next_account_info(iter)?;
            let instructions = next_account_info(iter)?;
            let native = next_account_info(iter)?;
            let native_bump = [sol_vault(program, policy, native)?];
            let native_seeds: &[&[u8]] = &[SOL_SEED, policy.key.as_ref(), &native_bump];
            let c = &p.config;
            let now = Clock::get()?;
            ensure(
                r.program == program.to_bytes()
                    && r.policy == policy.key.to_bytes()
                    && r.owner == p.owner
                    && r.network == c.network
                    && r.module_digest == c.module_digest
                    && r.nonce != [0; 32]
                    && r.operation_id != [0; 32]
                    && r.challenge_hash != [0; 32]
                    && r.evidence_hash != [0; 32]
                    && r.service_fee_lamports == c.service_fee_lamports,
                Error::InvalidRequest,
            )?;
            service_allowed(c, &r)?;
            ensure(
                r.signing_timestamp >= 0
                    && r.signing_timestamp <= now.unix_timestamp
                    && r.signing_slot <= now.slot
                    && r.expires_timestamp >= now.unix_timestamp
                    && r.expires_slot >= now.slot
                    && now.unix_timestamp <= c.policy_expires_timestamp
                    && r.expires_timestamp <= c.policy_expires_timestamp
                    && r.expires_slot > r.signing_slot
                    && r.expires_timestamp > r.signing_timestamp
                    && r.expires_slot - r.signing_slot <= c.max_age_slots
                    && r.expires_timestamp - r.signing_timestamp <= c.max_age_seconds as i64,
                Error::Expired,
            )?;
            approval(program, instructions, &c.evaluator, &r)?;
            address(treasury, &c.treasury)?;
            address(usdc, &c.vault_usdc)?;
            address(wsol, &c.vault_wsol)?;
            address(vendor, &c.vendor_usdc)?;
            address(mint, &c.usdc_mint)?;
            ensure(
                tokens.key == &spl_token::id() && system.key == &system_program::id(),
                Error::BadAccount,
            )?;
            vault_token(usdc, mint.key, policy.key)?;
            let wsol_before = vault_token(wsol, &spl_token::native_mint::id(), policy.key)?.amount;
            token(vendor, mint.key)?;
            let period = (r.signing_timestamp as u64) / c.period_seconds;
            let pb = period.to_le_bytes();
            let (rk, rb) = Pubkey::find_program_address(
                &[RECEIPT_SEED, policy.key.as_ref(), &r.nonce],
                program,
            );
            ensure(rk == *receipt.key, Error::BadAccount)?;
            ensure(
                receipt.owner == &system_program::id() && receipt.data_is_empty(),
                Error::Replay,
            )?;
            let (bk, bb) =
                Pubkey::find_program_address(&[BUDGET_SEED, policy.key.as_ref(), &pb], program);
            ensure(bk == *budget.key, Error::BadAccount)?;
            let mut b = if budget.owner == program {
                ensure(budget.data_len() == BUDGET_BYTES, Error::BadAccount)?;
                read::<Budget>(budget)?
            } else {
                create(
                    program,
                    budget,
                    actor,
                    system,
                    BUDGET_BYTES,
                    &[BUDGET_SEED, policy.key.as_ref(), &pb, &[bb]],
                )?;
                Budget {
                    period,
                    usdc: 0,
                    swap_lamports: 0,
                    fee_lamports: 0,
                    sol_debits: 0,
                }
            };
            ensure(b.period == period, Error::BadAccount)?;
            b.fee_lamports = add(
                b.fee_lamports,
                r.service_fee_lamports,
                c.max_fee_lamports_per_period,
            )?;
            match r.action {
                Action::PayUsdc { amount } => {
                    ensure(
                        amount > 0 && iter.as_slice().is_empty(),
                        Error::InvalidRequest,
                    )?;
                    b.usdc = add(b.usdc, amount, c.max_usdc_per_period)?;
                }
                Action::SwapSolToUsdc {
                    amount_in_lamports,
                    min_out_usdc,
                    sqrt_price_limit,
                    tick_arrays,
                } => {
                    ensure(
                        amount_in_lamports > 0
                            && amount_in_lamports <= c.max_swap_lamports_per_call,
                        Error::Budget,
                    )?;
                    let floor = ((amount_in_lamports as u128) * (c.min_usdc_per_sol as u128)
                        + 999_999_999)
                        / 1_000_000_000;
                    ensure(
                        min_out_usdc > 0 && (min_out_usdc as u128) >= floor,
                        Error::Slippage,
                    )?;
                    swap_pool(c, iter.as_slice(), sqrt_price_limit, &tick_arrays)?;
                    b.swap_lamports = add(
                        b.swap_lamports,
                        amount_in_lamports,
                        c.max_swap_lamports_per_period,
                    )?;
                    p.total_swap_lamports = add(
                        p.total_swap_lamports,
                        amount_in_lamports,
                        c.max_total_swap_lamports,
                    )?;
                }
            }
            create(
                program,
                receipt,
                actor,
                system,
                RECEIPT_BYTES,
                &[RECEIPT_SEED, policy.key.as_ref(), &r.nonce, &[rb]],
            )?;
            let rec = Receipt {
                version: 1,
                request_hash: hash(&signed_message(&r)?).to_bytes(),
                operation_id: r.operation_id,
                signing_timestamp: r.signing_timestamp,
            };
            save(receipt, &rec)?;
            save(budget, &b)?;
            let input = match r.action {
                Action::PayUsdc { .. } => 0,
                Action::SwapSolToUsdc {
                    amount_in_lamports, ..
                } => amount_in_lamports,
            };
            b.sol_debits = add(
                b.sol_debits,
                input
                    .checked_add(r.service_fee_lamports)
                    .ok_or(Error::Overflow)?,
                c.max_sol_debits_per_period,
            )?;
            save(budget, &b)?;
            p.total_sol_debits = add(
                p.total_sol_debits,
                input
                    .checked_add(r.service_fee_lamports)
                    .ok_or(Error::Overflow)?,
                c.allocation_lamports,
            )?;
            // First transfer out of the policy wallet is the owner-approved service fee.
            native_transfer(
                native,
                treasury,
                system,
                r.service_fee_lamports,
                native_seeds,
            )?;
            let bumpb = [p.bump];
            let seeds: &[&[u8]] = &[POLICY_SEED, &p.owner, &p.config.instance_id, &bumpb];
            match r.action {
                Action::PayUsdc { amount } => {
                    invoke_signed(
                        &spl_token::instruction::transfer_checked(
                            tokens.key,
                            usdc.key,
                            mint.key,
                            vendor.key,
                            policy.key,
                            &[],
                            amount,
                            6,
                        )?,
                        &[
                            usdc.clone(),
                            mint.clone(),
                            vendor.clone(),
                            policy.clone(),
                            tokens.clone(),
                        ],
                        &[seeds],
                    )?;
                }
                Action::SwapSolToUsdc {
                    amount_in_lamports,
                    min_out_usdc,
                    sqrt_price_limit,
                    tick_arrays: _,
                } => {
                    let before = token(usdc, mint.key)?.amount;
                    native_transfer(native, wsol, system, amount_in_lamports, native_seeds)?;
                    invoke(
                        &spl_token::instruction::sync_native(tokens.key, wsol.key)?,
                        &[wsol.clone(), tokens.clone()],
                    )?;
                    // SyncNative also recognizes earlier unsolicited SOL donations.
                    // Subtract only this request's input to protect the full prior balance.
                    let synced_wsol_before =
                        vault_token(wsol, &spl_token::native_mint::id(), policy.key)?
                            .amount
                            .checked_sub(amount_in_lamports)
                            .ok_or(Error::Budget)?;
                    ensure(synced_wsol_before >= wsol_before, Error::Budget)?;
                    let wsol_before = synced_wsol_before;
                    let pa = iter.as_slice();
                    let mut bytes = hash(b"global:swap").to_bytes()[..8].to_vec();
                    bytes.extend(amount_in_lamports.to_le_bytes());
                    bytes.extend(min_out_usdc.to_le_bytes());
                    bytes.extend(sqrt_price_limit.to_le_bytes());
                    bytes.extend([1, 1]);
                    let metas = vec![
                        AccountMeta::new_readonly(*tokens.key, false),
                        AccountMeta::new_readonly(*policy.key, true),
                        AccountMeta::new(*pa[1].key, false),
                        AccountMeta::new(*wsol.key, false),
                        AccountMeta::new(*pa[2].key, false),
                        AccountMeta::new(*usdc.key, false),
                        AccountMeta::new(*pa[3].key, false),
                        AccountMeta::new(*pa[5].key, false),
                        AccountMeta::new(*pa[6].key, false),
                        AccountMeta::new(*pa[7].key, false),
                        AccountMeta::new_readonly(*pa[4].key, false),
                    ];
                    invoke_signed(
                        &SolInstruction {
                            program_id: *pa[0].key,
                            accounts: metas,
                            data: bytes,
                        },
                        &[
                            tokens.clone(),
                            policy.clone(),
                            pa[1].clone(),
                            wsol.clone(),
                            pa[2].clone(),
                            usdc.clone(),
                            pa[3].clone(),
                            pa[5].clone(),
                            pa[6].clone(),
                            pa[7].clone(),
                            pa[4].clone(),
                            pa[0].clone(),
                        ],
                        &[seeds],
                    )?;
                    ensure(
                        vault_token(usdc, mint.key, policy.key)?
                            .amount
                            .checked_sub(before)
                            .is_some_and(|out| out >= min_out_usdc),
                        Error::Slippage,
                    )?;
                    let wsol_after =
                        vault_token(wsol, &spl_token::native_mint::id(), policy.key)?.amount;
                    ensure(wsol_after >= wsol_before, Error::Budget)?;
                    // Whirlpool may stop at the price limit before consuming all input.
                    // Preserve prior WSOL and require the entire newly wrapped input.
                    ensure(wsol_after == wsol_before, Error::Slippage)?;
                }
            }
            save(policy, &p)?;
            set_return_data(&borsh::to_vec(&rec).map_err(|_| ProgramError::InvalidAccountData)?);
            Ok(())
        }
        _ => Err(ProgramError::InvalidInstructionData),
    }
}
