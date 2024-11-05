use orca_whirlpools_core::TransferFee;
use solana_sdk::account_info::AccountInfo;
use solana_sdk::{
    pubkey::Pubkey,
    instruction::Instruction,
    system_instruction,
};
use solana_sdk::signer::Signer;
use spl_token_2022::extension::transfer_fee::TransferFeeConfig;
use spl_token_2022::extension::{BaseStateWithExtensions, StateWithExtensions};
use spl_token_2022::state::Mint;
use std::{collections::HashMap, error::Error};
use solana_sdk::client::Client;

use crate::{SolWrappingStrategy, SOL_WRAPPING_STRATEGY};

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
}

fn mint_filter(mint: &Pubkey) -> Result<bool, Box<dyn Error>> {
    let sol_wrapping_strategy = *SOL_WRAPPING_STRATEGY.try_lock()?;
    if sol_wrapping_strategy == SolWrappingStrategy::None || sol_wrapping_strategy == SolWrappingStrategy::Ata {
        return Ok(true);
    }
    Ok(*mint != NATIVE_MINT)
}

pub async fn prepare_token_accounts_instructions<C: Client>(
    rpc: &C,
    owner: &dyn Signer,
    spec: Vec<TokenAccountStrategy>,
) -> TokenAccountInstructions {
    let mint_addresses: Vec<&Pubkey> = spec.iter().map(|x| match x {
        TokenAccountStrategy::WithoutBalance(mint) => mint,
        TokenAccountStrategy::WithBalance(mint, _) => mint,
    }).collect();
    let sol_mint_index = mint_addresses.iter().position(|&&x| x == NATIVE_MINT);
    let has_sol_mint = sol_mint_index.is_some();
        let ata_addresses = mint_addresses.iter().filter(|&&x| x != NATIVE_MINT).collect();

    // Fetch mints and token accounts
    let mints = fetch_all_mint(rpc, mint_addresses.iter().filter(|&&x| mint_filter(&x)).collect()).await;
    let token_addresses: Vec<Pubkey> = mints.iter().map(|mint| find_associated_token_pda(owner.pubkey(), mint)).collect();
    let token_accounts = fetch_all_maybe_token(rpc, &token_addresses).await;

    let mut token_account_addresses = HashMap::new();
    let mut create_instructions = Vec::new();
    let mut cleanup_instructions = Vec::new();

    for (i, mint) in mints.iter().enumerate() {
        let token_account = &token_accounts[i];
        token_account_addresses.insert(mint.clone(), token_account.clone());

        if token_account.exists {
            continue;
        }

        create_instructions.push(get_create_associated_token_instruction(owner, token_account, mint));
    }

    if has_sol_mint && SOL_WRAPPING_STRATEGY == "keypair" {
        let keypair = Keypair::new();
        let space = get_token_size();
        let lamports = rpc.get_minimum_balance_for_rent_exemption(space).await.unwrap();

        create_instructions.push(system_instruction::create_account(
            &owner.pubkey(),
            &keypair.pubkey(),
            lamports,
            space as u64,
            &TOKEN_PROGRAM_ID,
        ));

        cleanup_instructions.push(get_close_account_instruction(&keypair.pubkey(), owner));
        token_account_addresses.insert(NATIVE_MINT, keypair.pubkey());
    }

    // Additional logic for other SOL_WRAPPING_STRATEGY cases...

    TokenAccountInstructions {
        create_instructions,
        cleanup_instructions,
        token_account_addresses,
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

pub fn order_mints<'a>(mint1: &'a Pubkey, mint2: &'a Pubkey) -> [&'a Pubkey; 2] {
    if mint1.lt(mint2) {
        [mint1, mint2]
    } else {
        [mint2, mint1]
    }
}
