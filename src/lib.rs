#![allow(unexpected_cfgs)]

use core::array::from_ref as b;

use const_crypto::bs58;
use pinocchio::{
    account_info::{AccountInfo, Ref, RefMut},
    instruction::Signer,
    program_error::ProgramError,
    pubkey::{find_program_address, pubkey_eq, Pubkey},
    seeds,
    sysvars::instructions::{Instructions, IntrospectedInstruction},
    ProgramResult,
};
use pinocchio_system::create_account_with_minimum_balance_signed;
use pinocchio_token::{instructions::Transfer, state::TokenAccount};

pinocchio::entrypoint!(process);

pub const ID: Pubkey = [5; 32];
pub const TOKEN_PROGRAM: Pubkey =
    bs58::decode_pubkey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

#[repr(C)]
pub struct Pool {
    pub usdc_atoms_per_sol: U128,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    // seeds: [mint_a(32), mint_b(32), bump(1)] = 65 bytes
    pub pool_seeds: [u8; 65],
}

use uint::construct_uint;

construct_uint! {
    pub struct U128(2);
}

impl Pool {
    pub const LEN: usize = core::mem::size_of::<Self>();

    pub fn from_account<'a>(account: &'a AccountInfo) -> Result<Ref<'a, Self>, ProgramError> {
        let data = account.try_borrow_data()?;
        if data.len() < Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Ref::map(data, |d| unsafe { &*d.as_ptr().cast() }))
    }

    pub fn from_account_mut<'a>(
        account: &'a AccountInfo,
    ) -> Result<RefMut<'a, Self>, ProgramError> {
        let data = account.try_borrow_mut_data()?;
        if data.len() < Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(RefMut::map(data, |d| unsafe {
            &mut *d.as_mut_ptr().cast()
        }))
    }
}

pub fn process(_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let (&disc, rest) = data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;

    match disc {
        0 => process_init(accounts, rest),
        1 => process_swap(accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn process_init(accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let [payer, pool, vault_a, vault_b, mint_a, mint_b, _system_program, _token_program] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if data.len() < 8 {
        return Err(ProgramError::InvalidInstructionData);
    }

    let usdc_atoms_per_sol = unsafe { data.as_ptr().cast::<u64>().read_unaligned() };

    // derive pool PDA
    let (expected_pool, bump_pool) = find_program_address(&[mint_a.key(), mint_b.key()], &ID);
    if !pubkey_eq(pool.key(), &expected_pool) {
        return Err(ProgramError::InvalidSeeds);
    }

    // derive vault PDAs
    let (expected_vault_a, bump_a) = find_program_address(&[pool.key(), mint_a.key()], &ID);
    let (expected_vault_b, bump_b) = find_program_address(&[pool.key(), mint_b.key()], &ID);

    if !pubkey_eq(vault_a.key(), &expected_vault_a) {
        return Err(ProgramError::InvalidSeeds);
    }
    if !pubkey_eq(vault_b.key(), &expected_vault_b) {
        return Err(ProgramError::InvalidSeeds);
    }

    // create pool account
    let seeds_pool = seeds!(mint_a.key(), mint_b.key(), b(&bump_pool));
    let signer_pool = Signer::from(&seeds_pool);

    create_account_with_minimum_balance_signed(
        pool,
        Pool::LEN,
        &ID,
        payer,
        None,
        &[signer_pool],
    )?;

    // init pool account
    let mut pool_data = Pool::from_account_mut(pool)?;
    pool_data.usdc_atoms_per_sol = U128::from(usdc_atoms_per_sol);
    pool_data.vault_a = *vault_a.key();
    pool_data.vault_b = *vault_b.key();

    // build pool seeds: [mint_a_key, mint_b_key, bump]
    pool_data.pool_seeds[0..32].copy_from_slice(mint_a.key());
    pool_data.pool_seeds[32..64].copy_from_slice(mint_b.key());
    pool_data.pool_seeds[64] = bump_pool;

    let seeds_a = seeds!(pool.key(), mint_a.key(), b(&bump_a));
    let signer_a = Signer::from(&seeds_a);

    let seeds_b = seeds!(pool.key(), mint_b.key(), b(&bump_b));
    let signer_b = Signer::from(&seeds_b);

    // create vault_a
    create_account_with_minimum_balance_signed(
        vault_a,
        TokenAccount::LEN,
        &TOKEN_PROGRAM,
        payer,
        None,
        &[signer_a],
    )?;

    // init vault_a (pool owns vault)
    pinocchio_token::instructions::InitializeAccount3 {
        account: vault_a,
        mint: mint_a,
        owner: pool.key(),
    }
    .invoke()?;

    // create vault_b
    create_account_with_minimum_balance_signed(
        vault_b,
        TokenAccount::LEN,
        &TOKEN_PROGRAM,
        payer,
        None,
        &[signer_b],
    )?;

    // init vault_b (pool owns vault)
    pinocchio_token::instructions::InitializeAccount3 {
        account: vault_b,
        mint: mint_b,
        owner: pool.key(),
    }
    .invoke()?;

    Ok(())
}

fn process_swap(accounts: &[AccountInfo]) -> ProgramResult {
    let [_payer, pool, user_out, pool_vault_a, pool_vault_b, ix_sysvar, _token_program] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    let pool_data = Pool::from_account(pool)?;

    // verify vaults match stored keys
    if !pubkey_eq(pool_vault_a.key(), &pool_data.vault_a) {
        return Err(ProgramError::InvalidSeeds);
    }
    if !pubkey_eq(pool_vault_b.key(), &pool_data.vault_b) {
        return Err(ProgramError::InvalidSeeds);
    }

    let instruction_sysvar = unsafe { Instructions::new_unchecked(ix_sysvar.try_borrow_data()?) };
    let cur_idx = instruction_sysvar.load_current_index() as usize;
    if cur_idx == 0 {
        return Err(IntrospectardioError::PrevIxNotTokenProgram.into());
    }
    let prev_idx = cur_idx - 1;

    let curr_ixn =
        unsafe { instruction_sysvar.deserialize_instruction_unchecked(cur_idx as usize) };
    // not a cpi (or whitelisted caller)
    if *curr_ixn.get_program_id() != ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    let prev_ix = instruction_sysvar.load_instruction_at(prev_idx)?;
    let amount_in = validate_prev_ix(prev_ix, pool_vault_a.clone())?;

    // Calculate amount out
    let Some(Ok(amount_out)) = U128::from(amount_in)
        .checked_mul(pool_data.usdc_atoms_per_sol)
        .map(|x| x / 1_000_000_000)
        .map(|x| x.try_into())
    else {
        return Err(IntrospectardioError::LargeOrder)?;
    };

    // Transfer out (pool signs for vault_b)
    let mint_a_key = &pool_data.pool_seeds[0..32];
    let mint_b_key = &pool_data.pool_seeds[32..64];
    let bump = &pool_data.pool_seeds[64..65];
    let seeds = seeds!(mint_a_key, mint_b_key, bump);
    let signer = Signer::from(&seeds);

    Transfer {
        from: pool_vault_b,
        to: user_out,
        authority: pool,
        amount: amount_out,
    }
    .invoke_signed(&[signer])?;

    Ok(())
}

#[repr(u32)]
pub enum IntrospectardioError {
    PrevIxNotTokenProgram,
    UnexpectedTokenProgramDataLen,
    UnexpectedTokenProgramIx,
    UnexpectedTransferDest,
    LargeOrder,
}

impl From<IntrospectardioError> for ProgramError {
    fn from(value: IntrospectardioError) -> ProgramError {
        ProgramError::Custom(value as u32)
    }
}

// Previous instruction must be
// 1) token program invocation
// 2) transfer ix data len
// 3) transfer ix disc
// 4) transfer dest is pool vault
//
// If we are executing this code, it's because the instruction succeeded!
fn validate_prev_ix(
    prev_ix: IntrospectedInstruction,
    pool_vault_in: AccountInfo,
) -> Result<u64, ProgramError> {
    // 1) token program invocation
    if !pubkey_eq(prev_ix.get_program_id(), &TOKEN_PROGRAM) {
        return Err(IntrospectardioError::PrevIxNotTokenProgram.into());
    }
    let prev_ix_data = prev_ix.get_instruction_data();

    // 2) transfer ix data len
    let correct_data_len = prev_ix_data.len() >= 9;
    if !correct_data_len {
        return Err(IntrospectardioError::UnexpectedTokenProgramDataLen.into());
    }

    // 3) transfer ix disc
    const TRANSFER_DISC: u8 = 3;
    let correct_disc = prev_ix_data[0] == TRANSFER_DISC;
    if !correct_disc {
        return Err(IntrospectardioError::UnexpectedTokenProgramIx.into());
    }

    // 4) transfer dest is pool vault
    // SAFETY: transfer succeeded so num accounts is correct
    let dest = unsafe { prev_ix.get_account_meta_at_unchecked(1) };
    let correct_dest = dest.key.eq(pool_vault_in.key());
    if !correct_dest {
        return Err(IntrospectardioError::UnexpectedTransferDest.into());
    }

    // read amount in
    let amount_in = unsafe { prev_ix_data.as_ptr().add(1).cast::<u64>().read_unaligned() };
    Ok(amount_in)
}