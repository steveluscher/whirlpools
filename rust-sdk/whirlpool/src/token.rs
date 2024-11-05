use orca_whirlpools_core::TransferFee;
use solana_sdk::account_info::AccountInfo;
use solana_sdk::signature::Keypair;
use solana_sdk::{
    pubkey::Pubkey,
    instruction::Instruction,
    system_instruction,
};
use solana_sdk::signer::Signer;
use spl_token::instruction::sync_native;
use spl_token::solana_program::program_pack::Pack;
use spl_token_2022::extension::transfer_fee::TransferFeeConfig;
use spl_token_2022::extension::{BaseStateWithExtensions, StateWithExtensions};
use spl_token_2022::state::Mint;
use std::{collections::HashMap, error::Error};
use solana_sdk::client::Client;
use spl_associated_token_account::{get_associated_token_address_with_program_id, instruction::create_associated_token_account};

use crate::{NativeMintWrappingStrategy, NATIVE_MINT_WRAPPING_STRATEGY};

pub const NATIVE_MINT: Pubkey = Pubkey::new_from_array([
    6, 155, 136, 87, 254, 171, 129, 132, 251, 104, 127, 99, 70, 24, 192, 53, 218, 196, 57, 220, 26, 235, 59, 85, 152, 160, 240, 0, 0, 0, 0, 1
]);

#[derive(Debug)]
pub enum TokenAccountStrategy {
    WithoutBalance(Pubkey),
    WithBalance(Pubkey, u64),
}

#[derive(Debug)]
pub struct TokenAccountInstructions {
    pub create_instructions: Vec<Instruction>,
    pub cleanup_instructions: Vec<Instruction>,
    pub token_account_addresses: HashMap<Pubkey, Pubkey>,
    pub additional_signers: Vec<Keypair>,
}

fn mint_filter(mint: &Pubkey, wrapping_strategy: NativeMintWrappingStrategy) -> bool {
    if wrapping_strategy == NativeMintWrappingStrategy::None || wrapping_strategy == NativeMintWrappingStrategy::Ata {
        return true;
    }
    *mint != NATIVE_MINT
}

pub async fn prepare_token_accounts_instructions<C: Client>(
    rpc: &C,
    owner: Pubkey,
    spec: Vec<TokenAccountStrategy>,
) -> Result<TokenAccountInstructions, Box<dyn Error>> {
    let mint_addresses_with_native_mint: Vec<Pubkey> = spec.iter().map(|x| match x {
        TokenAccountStrategy::WithoutBalance(mint) => *mint,
        TokenAccountStrategy::WithBalance(mint, _) => *mint,
    }).collect();
    let native_mint_wrapping_strategy = *NATIVE_MINT_WRAPPING_STRATEGY.try_lock()?;
    let native_mint_index = mint_addresses_with_native_mint.iter().position(|&x| x == NATIVE_MINT);
    let has_native_mint = native_mint_index.is_some();

    let mint_addresses: Vec<Pubkey> = mint_addresses_with_native_mint.iter()
        .filter(|&&x| x != NATIVE_MINT)
        .map(|x| *x)
        .collect();

    let mint_account_infos: Vec<AccountInfo> = mint_addresses.iter()
        .map(|x| rpc.get_account(x))
        .collect()?;

    let mints: Vec<Mint> = mint_account_infos.iter()
        .map(|x| Mint::unpack(&x.data.borrow()).unwrap())
        .collect()?;

    let ata_addresses: Vec<Pubkey> = mint_addresses.iter().enumerate()
        .map(|(i, mint)| get_associated_token_address_with_program_id(&owner, mint_account_infos[i].owner, mint))
        .collect();

    let ata_account_infos: Vec<Option<AccountInfo>> = ata_addresses.iter()
        .map(|x| rpc.get_account(x))
        .collect()?;

    let mut token_account_addresses: HashMap<Pubkey, Pubkey> = HashMap::new();
    let mut create_instructions: Vec<Instruction> = Vec::new();
    let mut cleanup_instructions: Vec<Instruction> = Vec::new();
    let mut additional_signers: Vec<Keypair> = Vec::new();

    for (i, mint) in mints.iter().enumerate() {
        let ata_address = ata_addresses[i];
        token_account_addresses.insert(*mint, ata_address);

        if ata_account_infos[i].is_some() {
            continue;
        }

        create_instructions.push(
            create_associated_token_account(
                &owner,
                owner,
                mint_addresses[i],
                mint_account_infos[i].owner
            )
        );
    }

    if has_native_mint && native_mint_wrapping_strategy == NativeMintWrappingStrategy::Keypair {
        let keypair = Keypair::new();
        let space = get_token_size(); // TODO: size is variable in token 2022?
        let lamports = rpc.get_minimum_balance_for_rent_exemption(space)?;

        create_instructions.push(system_instruction::create_account(
            &owner.pubkey(),
            &keypair.pubkey(),
            lamports,
            space as u64,
            &TOKEN_PROGRAM_ID,
        ));

        cleanup_instructions.push(get_close_account_instruction(&keypair.pubkey(), owner));
        token_account_addresses.insert(NATIVE_MINT, keypair.pubkey());
        additional_signers.push(keypair);
    }

    if has_native_mint && native_mint_wrapping_strategy == NativeMintWrappingStrategy::Seed {

    }

    let mut existing_native_mint_balance = 0;
    if has_native_mint && native_mint_wrapping_strategy == NativeMintWrappingStrategy::Ata {
        if let Some(native_ata_account) = ata_account_infos[native_mint_index.unwrap_or(0)] {
            let token_account = Account::unpack(&native_ata_account.data.borrow())?;
            existing_native_mint_balance = token_account.amount;
        } else {
            cleanup_instructions.push(close_account(&native_ata_account.key, owner));
        }
    }

    if has_native_mint && native_mint_wrapping_strategy != NativeMintWrappingStrategy::None {
        let native_mint_spec = spec[native_mint_index.unwrap_or(0)];
        if let TokenAccountStrategy::WithBalance(_, required_balance) = native_mint_spec && existing_native_mint_balance < required_balance {
            create_instructions.push(
                system_instruction::transfer(
                    &owner.pubkey(),
                    &token_account_addresses[&NATIVE_MINT],
                    required_balance - existing_native_mint_balance
                )
            );
            create_instructions.push(
                sync_native(
                    &TOKEN_PROGRAM_ID,
                    &token_account_addresses[&NATIVE_MINT],
                )
            );
        }
    }


    TokenAccountInstructions {
        create_instructions,
        cleanup_instructions,
        token_account_addresses,
        additional_signers,
    }
}

pub fn get_current_transfer_fee(
    mint_account_info: Option<&AccountInfo>,
    current_epoch: u64,
) -> Option<TransferFee> {
    let token_mint_data = mint_account_info?.try_borrow_data().ok()?;
    let token_mint_unpacked = StateWithExtensions::<Mint>::unpack(&token_mint_data).ok()?;
    if let Ok(transfer_fee_config) = token_mint_unpacked.get_extension::<TransferFeeConfig>() {
        let fee = transfer_fee_config.get_epoch_fee(current_epoch);
        return Some(TransferFee {
            fee_bps: fee.transfer_fee_basis_points.into(),
            max_fee: fee.maximum_fee.into(),
        });
    }

    None
}

pub fn order_mints(mint1: Pubkey, mint2: Pubkey) -> [Pubkey; 2] {
    if mint1.lt(&mint2) {
        [mint1, mint2]
    } else {
        [mint2, mint1]
    }
}
