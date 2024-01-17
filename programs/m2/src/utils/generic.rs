use anchor_lang::Discriminator;
use mpl_token_metadata::{
    accounts::{Metadata, TokenRecord},
    types::{TokenDelegateRole, TokenStandard, TokenState},
};
use spl_associated_token_account::instruction;

use crate::constants::{
    DEFAULT_BID_EXPIRY_SECONDS_AFTER_NOW, DEFAULT_MAKER_FEE_BP, DEFAULT_TAKER_FEE_BP,
    VALID_PAYMENT_MINTS,
};

use {
    crate::errors::ErrorCode,
    crate::states::*,
    anchor_lang::{
        prelude::*,
        solana_program::{
            program::invoke,
            program::invoke_signed,
            program_option::COption,
            program_pack::{IsInitialized, Pack},
            system_instruction,
        },
    },
    anchor_spl::token::Mint,
    arrayref::array_ref,
    spl_associated_token_account::get_associated_token_address,
    std::convert::TryInto,
};

pub fn get_default_buyer_state_expiry(buyer_state_expiry: i64) -> i64 {
    match buyer_state_expiry {
        0 => Clock::get().unwrap().unix_timestamp + DEFAULT_BID_EXPIRY_SECONDS_AFTER_NOW,
        _ => buyer_state_expiry,
    }
}

pub fn get_actual_maker_taker_fee_bp(
    notary: &AccountInfo,
    maker_fee_bp: i16,
    taker_fee_bp: u16,
) -> (i16, u16) {
    match notary.is_signer {
        true => (maker_fee_bp, taker_fee_bp),
        false => (DEFAULT_MAKER_FEE_BP, DEFAULT_TAKER_FEE_BP),
    }
}

pub fn is_token_owner(token_account: &AccountInfo, owner: &Pubkey) -> Result<bool> {
    let acc: spl_token::state::Account = assert_initialized(token_account)?;
    Ok(acc.owner == *owner)
}

pub fn assert_is_ata(
    ata: &AccountInfo,
    wallet: &Pubkey,
    mint: &Pubkey,
    optional_owner: &Pubkey,
) -> Result<spl_token::state::Account> {
    assert_owned_by(ata, &spl_token::id())?;
    let ata_account: spl_token::state::Account = assert_initialized(ata)?;
    if ata_account.owner != *optional_owner {
        assert_keys_equal(&ata_account.owner, wallet)?;
    }
    assert_keys_equal(&ata_account.mint, mint)?;
    assert_keys_equal(&get_associated_token_address(wallet, mint), ata.key)?;
    Ok(ata_account)
}

pub fn assert_bump(seeds: &[&[u8]], program_id: &Pubkey, bump: u8) -> Result<()> {
    let (_acct, _bump) = Pubkey::find_program_address(seeds, program_id);
    if _bump != bump {
        return Err(ErrorCode::InvalidBump.into());
    }
    Ok(())
}

pub fn make_ata<'a>(
    ata: AccountInfo<'a>,
    payer: AccountInfo<'a>,
    wallet: AccountInfo<'a>,
    mint: AccountInfo<'a>,
    token_program: AccountInfo<'a>,
    system_program: AccountInfo<'a>,
) -> Result<()> {
    invoke(
        &instruction::create_associated_token_account(
            payer.key,
            wallet.key,
            mint.key,
            token_program.key,
        ),
        &[payer, ata, wallet, mint, system_program, token_program],
    )?;

    Ok(())
}

pub fn assert_metadata_valid(metadata: &UncheckedAccount, token_mint: &Pubkey) -> Result<()> {
    assert_derivation(
        &mpl_token_metadata::ID,
        &metadata.to_account_info(),
        &[
            Metadata::PREFIX,
            mpl_token_metadata::ID.as_ref(),
            token_mint.as_ref(),
        ],
    )?;
    assert_owned_by(metadata, &mpl_token_metadata::ID)?;
    if metadata.data_is_empty() {
        return Err(ErrorCode::MetadataDoesntExist.into());
    }
    Ok(())
}

pub fn assert_valid_notary(
    auction_house: &AuctionHouse,
    notary: &UncheckedAccount,
    enforce_prob: u8, // 0-100
) -> Result<()> {
    if auction_house.requires_notary {
        if ((Clock::get()?.unix_timestamp.abs() % 100) as u8) >= enforce_prob {
            return Ok(());
        }

        if !notary.to_account_info().is_signer {
            return Err(ErrorCode::InvalidAccountState.into());
        }

        if notary.key() != auction_house.notary {
            return Err(ErrorCode::InvalidAccountState.into());
        }
    }

    Ok(())
}

#[allow(dead_code)]
pub fn assert_valid_delegation(
    src_account: &AccountInfo,
    dst_account: &AccountInfo,
    src_wallet: &AccountInfo,
    dst_wallet: &AccountInfo,
    transfer_authority: &AccountInfo,
    mint: &anchor_lang::prelude::Account<Mint>,
    paysize: u64,
) -> Result<()> {
    match spl_token::state::Account::unpack(&src_account.data.borrow()) {
        Ok(token_account) => {
            // Ensure that the delegated amount is exactly equal to the maker_size
            if token_account.delegated_amount != paysize {
                return Err(ErrorCode::InvalidAccountState.into());
            }
            // Ensure that authority is the delegate of this token account
            if token_account.delegate != COption::Some(*transfer_authority.key) {
                return Err(ErrorCode::InvalidAccountState.into());
            }

            assert_is_ata(src_account, src_wallet.key, &mint.key(), src_wallet.key)?;
            assert_is_ata(dst_account, dst_wallet.key, &mint.key(), dst_wallet.key)?;
        }
        Err(_) => {
            if mint.key() != spl_token::native_mint::id() {
                return Err(ErrorCode::ExpectedSolAccount.into());
            }

            if !src_wallet.is_signer {
                return Err(ErrorCode::SOLWalletMustSign.into());
            }

            assert_keys_equal(src_wallet.key, src_account.key)?;
            assert_keys_equal(dst_wallet.key, dst_account.key)?;
        }
    }

    Ok(())
}

pub fn assert_keys_equal(key1: &Pubkey, key2: &Pubkey) -> Result<()> {
    if key1 != key2 {
        Err(ErrorCode::PublicKeyMismatch.into())
    } else {
        Ok(())
    }
}

pub fn assert_initialized<T: Pack + IsInitialized>(account_info: &AccountInfo) -> Result<T> {
    let account: T = T::unpack_unchecked(&account_info.data.borrow())?;
    if !account.is_initialized() {
        Err(ErrorCode::UninitializedAccount.into())
    } else {
        Ok(account)
    }
}

pub fn assert_owned_by(account: &AccountInfo, owner: &Pubkey) -> Result<()> {
    if account.owner != owner {
        Err(ErrorCode::IncorrectOwner.into())
    } else {
        Ok(())
    }
}

pub fn assert_payment_mint(mint_ai: &AccountInfo) -> Result<()> {
    if !VALID_PAYMENT_MINTS.contains(mint_ai.key) || mint_ai.owner != &spl_token::id() {
        Err(ErrorCode::InvalidTokenMint.into())
    } else {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments, dead_code)]
pub fn pay_auction_house_fees<'a>(
    auction_house: &anchor_lang::prelude::Account<'a, AuctionHouse>,
    auction_house_treasury: &AccountInfo<'a>,
    escrow_payment_account: &AccountInfo<'a>,
    buyer_referral: &AccountInfo<'a>,
    seller_referral: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    signer_seeds: &[&[u8]],
    size: u64,
) -> Result<u64> {
    let treasury_bp = auction_house.seller_fee_basis_points;
    let buyer_referral_bp = auction_house.buyer_referral_bp;
    let mut buyer_referral_fee = 0_u64;
    let seller_referral_bp = auction_house.seller_referral_bp;
    let mut seller_referral_fee = 0_u64;

    if buyer_referral_bp > 0 {
        buyer_referral_fee = (buyer_referral_bp as u128)
            .checked_mul(size as u128)
            .ok_or(ErrorCode::NumericalOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::NumericalOverflow)? as u64;

        invoke_signed(
            &system_instruction::transfer(
                escrow_payment_account.key,
                buyer_referral.key,
                buyer_referral_fee,
            ),
            &[
                escrow_payment_account.clone(),
                buyer_referral.clone(),
                system_program.clone(),
            ],
            &[signer_seeds],
        )?;
    }

    if seller_referral_bp > 0 {
        seller_referral_fee = (seller_referral_bp as u128)
            .checked_mul(size as u128)
            .ok_or(ErrorCode::NumericalOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::NumericalOverflow)? as u64;

        invoke_signed(
            &system_instruction::transfer(
                escrow_payment_account.key,
                seller_referral.key,
                seller_referral_fee,
            ),
            &[
                escrow_payment_account.clone(),
                seller_referral.clone(),
                system_program.clone(),
            ],
            &[signer_seeds],
        )?;
    }

    let treasury_fee = (treasury_bp as u128)
        .checked_mul(size as u128)
        .ok_or(ErrorCode::NumericalOverflow)?
        .checked_div(10000)
        .ok_or(ErrorCode::NumericalOverflow)?
        .checked_sub(buyer_referral_fee as u128 + seller_referral_fee as u128)
        .ok_or(ErrorCode::NumericalOverflow)? as u64;

    invoke_signed(
        &system_instruction::transfer(
            escrow_payment_account.key,
            auction_house_treasury.key,
            treasury_fee,
        ),
        &[
            escrow_payment_account.clone(),
            auction_house_treasury.clone(),
            system_program.clone(),
        ],
        &[signer_seeds],
    )?;

    Ok(treasury_fee)
}

pub fn split_payer_from_remaining_accounts<'a, 'info>(
    remaining_accounts: &'a [AccountInfo<'info>],
) -> (&'a [AccountInfo<'info>], Option<&'a AccountInfo<'info>>) {
    if let Some((last, rest)) = remaining_accounts.split_last() {
        if last.is_signer {
            (rest, Some(last))
        } else {
            (remaining_accounts, None)
        }
    } else {
        (remaining_accounts, None)
    }
}

/// Cheap method to just grab mint Pubkey from token account, instead of deserializing entire thing
#[allow(dead_code)]
pub fn get_mint_from_token_account(token_account_info: &AccountInfo) -> Result<Pubkey> {
    // TokeAccount layout:   mint(32), owner(32), ...
    let data = token_account_info.try_borrow_data()?;
    let mint_data = array_ref![data, 0, 32];
    Ok(Pubkey::new_from_array(*mint_data))
}

/// Cheap method to just grab delegate Pubkey from token account, instead of deserializing entire thing
pub fn get_delegate_from_token_account(token_account_info: &AccountInfo) -> Result<Option<Pubkey>> {
    // TokeAccount layout:   mint(32), owner(32), ...
    let data = token_account_info.try_borrow_data()?;
    let key_data = array_ref![data, 76, 32];
    let coption_data = u32::from_le_bytes(*array_ref![data, 72, 4]);
    if coption_data == 0 {
        Ok(None)
    } else {
        Ok(Some(Pubkey::new_from_array(*key_data)))
    }
}

#[allow(dead_code)]
pub fn get_balance_from_token_account(token_account_info: &AccountInfo) -> Result<u64> {
    // TokeAccount layout:   mint(32), owner(32), ...
    let data = token_account_info.try_borrow_data()?;
    let balance_data = array_ref![data, 64, 8];
    Ok(u64::from_le_bytes(*balance_data))
}

/// Create account almost from scratch, lifted from
/// https://github.com/solana-labs/solana-program-library/blob/7d4873c61721aca25464d42cc5ef651a7923ca79/associated-token-account/program/src/processor.rs#L51-L98
#[inline(always)]
#[allow(dead_code)]
pub fn create_or_allocate_account_raw<'a>(
    program_id: &Pubkey,
    new_account_info: &AccountInfo<'a>,
    payer_info: &AccountInfo<'a>,
    required_lamports: &u64,
    size: &usize,
    new_acct_seeds: &[&[u8]],
) -> Result<()> {
    if *required_lamports > 0 {
        invoke(
            &system_instruction::transfer(payer_info.key, new_account_info.key, *required_lamports),
            &[payer_info.clone(), new_account_info.clone()],
        )?;
    }

    let accounts = &[new_account_info.clone()];
    invoke_signed(
        &system_instruction::allocate(new_account_info.key, (*size).try_into().unwrap()),
        accounts,
        &[new_acct_seeds],
    )?;

    invoke_signed(
        &system_instruction::assign(new_account_info.key, program_id),
        accounts,
        &[new_acct_seeds],
    )?;

    Ok(())
}

pub fn assert_derivation(program_id: &Pubkey, account: &AccountInfo, path: &[&[u8]]) -> Result<u8> {
    let (key, bump) = Pubkey::find_program_address(path, program_id);
    if key != *account.key {
        return Err(ErrorCode::DerivedKeyInvalid.into());
    }
    Ok(bump)
}

pub fn try_close_buyer_escrow<'info>(
    escrow: &AccountInfo<'info>,
    buyer: &AccountInfo<'info>,
    system_program: &Program<'info, System>,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    let min_rent = Rent::get()?.minimum_balance(0);
    let escrow_lamports = escrow.lamports();
    if escrow_lamports == 0 || escrow_lamports > min_rent {
        Ok(())
    } else {
        anchor_lang::solana_program::program::invoke_signed(
            &anchor_lang::solana_program::system_instruction::transfer(
                &escrow.key(),
                &buyer.key(),
                escrow_lamports,
            ),
            &[
                escrow.to_account_info(),
                buyer.to_account_info(),
                system_program.to_account_info(),
            ],
            seeds,
        )?;
        Ok(())
    }
}

pub fn check_programmable(metadata_parsed: &Metadata) -> Result<()> {
    match metadata_parsed.token_standard {
        None => return Err(ErrorCode::InvalidTokenStandard.into()),
        Some(ref t) => {
            if *t != TokenStandard::ProgrammableNonFungible {
                return Err(ErrorCode::InvalidTokenStandard.into());
            }
        }
    }
    Ok(())
}

pub fn close_account_anchor(info: &AccountInfo, dest: &AccountInfo) -> Result<()> {
    let curr_lamp = info.lamports();
    **info.lamports.borrow_mut() = 0;
    **dest.lamports.borrow_mut() = dest
        .lamports()
        .checked_add(curr_lamp)
        .ok_or(ErrorCode::NumericalOverflow)?;
    info.try_borrow_mut_data()?[0..8].copy_from_slice(&[0; 8]);
    Ok(())
}

pub fn get_delegate_info_and_token_state_from_token_record(
    info: &AccountInfo,
) -> Result<(Option<Pubkey>, Option<TokenDelegateRole>, TokenState)> {
    let token_record = TokenRecord::safe_deserialize(&info.data.borrow())?;
    Ok((
        token_record.delegate,
        token_record.delegate_role,
        token_record.state,
    ))
}

pub fn create_or_realloc_seller_trade_state<'a>(
    sts: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    sts_seeds: &[&[u8]],
) -> Result<()> {
    let rent = Rent::get()?;
    let required_lamports = rent
        .minimum_balance(SellerTradeStateV2::LEN)
        .saturating_sub(sts.lamports());
    if sts.data_is_empty() {
        // brand new account, need to create it with correct length
        invoke_signed(
            &system_instruction::create_account(
                payer.key,
                sts.key,
                required_lamports,
                SellerTradeStateV2::LEN as u64,
                &crate::ID,
            ),
            &[payer.clone(), sts.clone()],
            &[sts_seeds],
        )?;

        sts.try_borrow_mut_data()?[..8].copy_from_slice(&SellerTradeStateV2::discriminator());
        Ok(())
    } else if sts.data_len() == SellerTradeState::LEN {
        // old buyer trade state that we want to migrate
        // zero out original data
        sts.try_borrow_mut_data()?
            .copy_from_slice(&[0; SellerTradeState::LEN]);
        // reallocate new space
        sts.realloc(SellerTradeStateV2::LEN, true)?;
        // transfer lamports so become rent exempt
        if required_lamports > 0 {
            invoke(
                &system_instruction::transfer(payer.key, sts.key, required_lamports),
                &[payer.clone(), sts.clone()],
            )?;
        }

        // write discriminator
        sts.try_borrow_mut_data()?[0..8].copy_from_slice(&SellerTradeStateV2::discriminator());
        Ok(())
    } else if sts.try_borrow_data()?[0..8] == SellerTradeStateV2::discriminator() {
        Ok(())
    } else {
        Err(ErrorCode::InvalidAccountState.into())
    }
}

#[macro_export]
macro_rules! index_ra {
    ($iter:ident, $i:expr) => {
        $iter.get($i).ok_or(ErrorCode::MissingRemainingAccount)?
    };
}

pub fn create_or_realloc_buyer_trade_state<'a>(
    bts: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    bts_seeds: &[&[u8]],
) -> Result<()> {
    let rent = Rent::get()?;
    let required_lamports = rent
        .minimum_balance(BuyerTradeStateV2::LEN)
        .saturating_sub(bts.lamports());
    if bts.data_is_empty() {
        // brand new account, need to create it with correct length
        invoke_signed(
            &system_instruction::create_account(
                payer.key,
                bts.key,
                required_lamports,
                BuyerTradeStateV2::LEN as u64,
                &crate::ID,
            ),
            &[payer.clone(), bts.clone()],
            &[bts_seeds],
        )?;

        bts.try_borrow_mut_data()?[..8].copy_from_slice(&BuyerTradeStateV2::discriminator());
        Ok(())
    } else if bts.data_len() == BuyerTradeState::LEN {
        // old buyer trade state that we want to migrate
        // zero out original data
        bts.try_borrow_mut_data()?
            .copy_from_slice(&[0; BuyerTradeState::LEN]);
        // reallocate new space
        bts.realloc(BuyerTradeStateV2::LEN, true)?;
        // transfer lamports so become rent exempt
        if required_lamports > 0 {
            invoke(
                &system_instruction::transfer(payer.key, bts.key, required_lamports),
                &[payer.clone(), bts.clone()],
            )?;
        }

        // write discriminator
        bts.try_borrow_mut_data()?[0..8].copy_from_slice(&BuyerTradeStateV2::discriminator());
        Ok(())
    } else if bts.try_borrow_data()?[0..8] == BuyerTradeStateV2::discriminator() {
        Ok(())
    } else {
        Err(ErrorCode::InvalidAccountState.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_keys_equal_returns_ok_when_keys_are_equal() -> Result<()> {
        let pubkey = Pubkey::new_from_array([1; 32]);
        let same_pubkey = Pubkey::new_from_array([1; 32]);
        assert_keys_equal(&pubkey, &same_pubkey)
    }

    #[test]
    fn assert_owned_by_returns_ok_when_given_account_is_owned_by_given_owner() -> Result<()> {
        let mut lamports: u64 = 1;
        let mut data = [1];
        let owner = Pubkey::new_unique();
        let account = AccountInfo::new(
            &owner,
            false,
            false,
            &mut lamports,
            &mut data,
            &owner,
            false,
            4,
        );

        assert_owned_by(&account, &owner)
    }

    #[test]
    fn assert_initialized_returns_ok_when_account_is_frozen() {
        let mut buffer = vec![0; spl_token::state::Account::get_packed_len()];
        let mut lamports: u64 = 1;
        let owner = Pubkey::new_unique();
        let spl_token_account = spl_token::state::Account {
            mint: Pubkey::new_unique(),
            owner,
            amount: 1,
            delegate: COption::None,
            state: spl_token::state::AccountState::Frozen,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        spl_token::state::Account::pack(spl_token_account, &mut buffer)
            .expect("Could not pack SPL token account into buffer");

        let account_info = AccountInfo::new(
            &owner,
            false,
            false,
            &mut lamports,
            &mut buffer,
            &owner,
            false,
            4,
        );

        match assert_initialized::<spl_token::state::Account>(&account_info) {
            Ok(result) => assert_eq!(result, spl_token_account),
            _ => panic!("expected Ok(spl_token_account)"),
        }
    }

    #[test]
    fn assert_is_ata_returns_ok_when_account_is_ata() -> Result<()> {
        let mut buffer = vec![0; spl_token::state::Account::get_packed_len()];
        let mut lamports: u64 = 1;
        let owner = spl_token::id();
        let mint = Pubkey::new_unique();
        let spl_token_account = spl_token::state::Account {
            mint,
            owner,
            amount: 1,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        spl_token::state::Account::pack(spl_token_account, &mut buffer)
            .expect("Could not pack SPL token account into buffer");

        let key = get_associated_token_address(&owner, &mint);
        let account_info = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports,
            &mut buffer,
            &owner,
            false,
            4,
        );

        assert_is_ata(&account_info, &owner, &mint, &owner).map(|_| ())
    }

    #[test]
    fn get_mint_from_token_account_returns_mint_pubkey() {
        let mut buffer = vec![0; spl_token::state::Account::get_packed_len()];
        let mut lamports: u64 = 1;
        let owner = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let spl_token_account = spl_token::state::Account {
            owner,
            amount: 1,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
            mint,
        };

        spl_token::state::Account::pack(spl_token_account, &mut buffer)
            .expect("Could not pack SPL token account into buffer");

        let account_info = AccountInfo::new(
            &owner,
            false,
            false,
            &mut lamports,
            &mut buffer,
            &owner,
            false,
            4,
        );

        match get_mint_from_token_account(&account_info) {
            Ok(result) => assert_eq!(result, mint),
            _ => panic!("expected Ok(mint)"),
        }
    }

    #[test]
    fn get_balance_from_token_account_returns_balance() {
        let mut buffer = vec![0; spl_token::state::Account::get_packed_len()];
        let mut lamports: u64 = 1;
        let owner = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let balance: u64 = 10004;
        let spl_token_account = spl_token::state::Account {
            owner,
            amount: balance,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
            mint,
        };

        spl_token::state::Account::pack(spl_token_account, &mut buffer)
            .expect("Could not pack SPL token account into buffer");

        let account_info = AccountInfo::new(
            &owner,
            false,
            false,
            &mut lamports,
            &mut buffer,
            &owner,
            false,
            4,
        );

        match get_balance_from_token_account(&account_info) {
            Ok(result) => assert_eq!(result, balance),
            _ => panic!("expected Ok(balance)"),
        }
    }
}
