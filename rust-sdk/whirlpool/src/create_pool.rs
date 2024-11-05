use std::collections::HashSet;
use std::error::Error;

use orca_whirlpools_client::accounts::{TickArray, Whirlpool};
use orca_whirlpools_client::instructions::{InitializePoolV2, InitializePoolV2InstructionArgs, InitializeTickArray, InitializeTickArrayInstructionArgs};
use orca_whirlpools_client::{get_fee_tier_address, get_tick_array_address, get_token_badge_address, get_whirlpool_address};
use orca_whirlpools_core::{get_full_range_tick_indexes, get_tick_array_start_tick_index, price_to_sqrt_price, sqrt_price_to_tick_index};
use solana_sdk::rent::Rent;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::system_program;
use solana_sdk::sysvar::SysvarId;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};
use solana_sdk::client::Client;
use spl_token::solana_program::program_pack::Pack;
use spl_token_2022::state::{Account, Mint};

use crate::{FUNDER, SPLASH_POOL_TICK_SPACING, WHIRLPOOLS_CONFIG_ADDRESS, WHIRLPOOLS_CONFIG_EXTENSION_ADDRESS};

/// Represents the instructions and metadata for creating a pool.
pub struct CreatePoolInstructions {
  /// The list of instructions needed to create the pool.
  pub instructions: Vec<Instruction>,

  /// The estimated rent exemption cost for initializing the pool, in lamports.
  pub est_initialization_cost: u64,

  /// The address of the newly created pool.
  pub pool_address: Pubkey,

  /// The list of signers for the instructions.
  pub signers: Vec<Keypair>,
}

pub fn create_splash_pool_instructions<C: Client>(
  rpc: &C,
  token_a: Pubkey,
  token_b: Pubkey,
  initial_price: Option<f64>,
  funder: Option<Pubkey>,
) -> Result<CreatePoolInstructions, Box<dyn Error>> {
  create_concentrated_liquidity_pool_instructions(
    rpc,
    token_a,
    token_b,
    SPLASH_POOL_TICK_SPACING,
    initial_price,
    funder,
  )
}

pub fn create_concentrated_liquidity_pool_instructions<C: Client>(
  rpc: &C,
  token_a: Pubkey,
  token_b: Pubkey,
  tick_spacing: u16,
  initial_price: Option<f64>,
  funder: Option<Pubkey>,
) -> Result<CreatePoolInstructions, Box<dyn Error>> {
  let initial_price = initial_price.unwrap_or(1.0);
  let funder = funder.unwrap_or(*FUNDER.try_lock()?);
  assert!(funder != Pubkey::default(), "Funder must be provided");
  assert!(token_a.to_bytes() < token_b.to_bytes(), "Token order needs to be flipped to match the canonical ordering (i.e. sorted on the byte repr. of the mint pubkeys)");

  let mint_a_info = rpc.get_account(&token_a)?
    .ok_or(format!("Mint {} not found", token_a))?;
  let mint_a = Mint::unpack(&mint_a_info.data)?;
  let decimals_a = mint_a.decimals;
  let token_program_a = mint_a_info.owner;
  let mint_b_info = rpc.get_account(&token_b)?
    .ok_or(format!("Mint {} not found", token_b))?;
  let mint_b = Mint::unpack(&mint_b_info.data)?;
  let decimals_b = mint_b.decimals;
  let token_program_b = mint_b_info.owner;

  let initial_sqrt_price: u128 = price_to_sqrt_price(initial_price, decimals_a, decimals_b).into();

  let pool_address = get_whirlpool_address(
    &*WHIRLPOOLS_CONFIG_ADDRESS.try_lock()?,
    &token_a,
    &token_b,
    tick_spacing,
  )?.0;

  let fee_tier = get_fee_tier_address(
    &*WHIRLPOOLS_CONFIG_EXTENSION_ADDRESS.try_lock()?,
    tick_spacing,
  )?.0;

  let token_badge_a = get_token_badge_address(
    &*WHIRLPOOLS_CONFIG_EXTENSION_ADDRESS.try_lock()?,
    &token_a,
  )?.0;

  let token_badge_b = get_token_badge_address(
    &*WHIRLPOOLS_CONFIG_EXTENSION_ADDRESS.try_lock()?,
    &token_b,
  )?.0;

  let token_vault_a = Keypair::new();
  let token_vault_b = Keypair::new();

  let mut state_space = 0;
  let mut instructions = vec![];

  instructions.push(
    InitializePoolV2 {
      whirlpools_config: *WHIRLPOOLS_CONFIG_ADDRESS.try_lock()?,
      token_mint_a: token_a,
      token_mint_b: token_b,
      token_badge_a,
      token_badge_b,
      funder,
      whirlpool: pool_address,
      token_vault_a: token_vault_a.pubkey(),
      token_vault_b: token_vault_b.pubkey(),
      fee_tier,
      token_program_a,
      token_program_b,
      system_program: system_program::id(),
      rent: Rent::id(),
    }.instruction(InitializePoolV2InstructionArgs {
      initial_sqrt_price,
      tick_spacing,
    })
  );

  state_space += Whirlpool::LEN;
  state_space += Account::LEN * 2;

  let full_range = get_full_range_tick_indexes(tick_spacing);
  let lower_tick_index = get_tick_array_start_tick_index(full_range.tick_lower_index, tick_spacing);
  let upper_tick_index = get_tick_array_start_tick_index(full_range.tick_upper_index, tick_spacing);
  let initial_tick_index =  sqrt_price_to_tick_index(initial_sqrt_price.into());
  let current_tick_index = get_tick_array_start_tick_index(initial_tick_index, tick_spacing);

  let tick_array_indexes = HashSet::from([lower_tick_index, upper_tick_index, current_tick_index]);
  for start_tick_index in tick_array_indexes {
    let tick_array_address = get_tick_array_address(&pool_address, start_tick_index)?;
    instructions.push(
      InitializeTickArray {
        whirlpool: pool_address,
        tick_array: tick_array_address.0,
        funder,
        system_program: system_program::id(),
      }.instruction(InitializeTickArrayInstructionArgs {
        start_tick_index,
      })
    );
    state_space += TickArray::LEN;
  }

  let est_initialization_cost = rpc.get_minimum_balance_for_rent_exemption(state_space)?;

  Ok(CreatePoolInstructions {
    instructions,
    est_initialization_cost,
    pool_address,
    signers: vec![token_vault_a, token_vault_b],
  })
}