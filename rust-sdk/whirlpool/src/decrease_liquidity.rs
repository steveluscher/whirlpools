use std::error::Error;

use orca_whirlpools_client::{accounts::{Position, Whirlpool}, get_position_address, get_tick_array_address, instructions::{DecreaseLiquidityV2, DecreaseLiquidityV2InstructionArgs}};
use orca_whirlpools_core::{decrease_liquidity_quote, decrease_liquidity_quote_a, decrease_liquidity_quote_b, get_tick_array_start_tick_index, CollectFeesQuote, CollectRewardsQuote, DecreaseLiquidityQuote};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{instruction::Instruction, program_pack::Pack, pubkey::Pubkey, signature::Keypair};
use spl_associated_token_account::get_associated_token_address_with_program_id;
use spl_token_2022::state::Mint;

use crate::{token::{get_current_transfer_fee, prepare_token_accounts_instructions, TokenAccountStrategy}, FUNDER, SLIPPAGE_TOLERANCE_BPS};

// TODO: support transfer hooks

#[derive(Debug, Clone)]
pub enum DecreaseLiquidityParam {
  TokenA(u64),
  TokenB(u64),
  Liquidity(u128),
}

#[derive(Debug)]
pub struct DecreaseLiquidityInstruction {
  pub quote: DecreaseLiquidityQuote,
  pub instructions: Vec<Instruction>,
  pub additional_signers: Vec<Keypair>,
}

pub fn decrease_liquidity_instructions(
  rpc: &RpcClient,
  position_mint_address: Pubkey,
  param: DecreaseLiquidityParam,
  slippage_tolerance_bps: Option<u16>,
  authority: Option<Pubkey>,
) -> Result<DecreaseLiquidityInstruction, Box<dyn Error>> {
  let slippage_tolerance_bps = slippage_tolerance_bps.unwrap_or(*SLIPPAGE_TOLERANCE_BPS.try_lock()?);
  let authority = authority.unwrap_or(*FUNDER.try_lock()?);
  if authority != Pubkey::default() {
    return Err("Authority must be provided".into());
  }

  let position_address = get_position_address(&position_mint_address)?.0;
  let position_info = rpc.get_account(&position_address)?;
  let position = Position::from_bytes(&position_info.data)?;

  let pool_info = rpc.get_account(&position.whirlpool)?;
  let pool = Whirlpool::from_bytes(&pool_info.data)?;

  let mint_infos = rpc.get_multiple_accounts(&[
    pool.token_mint_a,
    pool.token_mint_b,
    position_mint_address,
  ])?;

  let mint_a_info = mint_infos[0]
    .as_ref()
    .ok_or("Token A mint info not found")?;
  let mint_b_info = mint_infos[1]
    .as_ref()
    .ok_or("Token B mint info not found")?;
  let position_mint_info = mint_infos[2]
    .as_ref()
    .ok_or("Position mint info not found")?;

  let current_epoch = rpc.get_epoch_info()?.epoch;
  let transfer_fee_a = get_current_transfer_fee(mint_a_info, current_epoch);
  let transfer_fee_b = get_current_transfer_fee(mint_b_info, current_epoch);

  let quote = match param {
    DecreaseLiquidityParam::TokenA(amount) => decrease_liquidity_quote_a(amount, slippage_tolerance_bps, pool.sqrt_price, position.tick_lower_index, position.tick_upper_index, transfer_fee_a, transfer_fee_b),
    DecreaseLiquidityParam::TokenB(amount) => decrease_liquidity_quote_b(amount, slippage_tolerance_bps, pool.sqrt_price, position.tick_lower_index, position.tick_upper_index, transfer_fee_a, transfer_fee_b),
    DecreaseLiquidityParam::Liquidity(amount) => decrease_liquidity_quote(amount, slippage_tolerance_bps, pool.sqrt_price, position.tick_lower_index, position.tick_upper_index, transfer_fee_a, transfer_fee_b),
  }?;

  let mut instructions: Vec<Instruction> = Vec::new();

  let lower_tick_array_start_index = get_tick_array_start_tick_index(position.tick_lower_index, pool.tick_spacing);
  let upper_tick_array_start_index = get_tick_array_start_tick_index(position.tick_upper_index, pool.tick_spacing);

  let position_token_account_address = get_associated_token_address_with_program_id(&authority, &position_mint_address, &position_mint_info.owner);
  let lower_tick_array_address = get_tick_array_address(&position.whirlpool, lower_tick_array_start_index)?.0;
  let upper_tick_array_address = get_tick_array_address(&position.whirlpool, upper_tick_array_start_index)?.0;

  let token_accounts = prepare_token_accounts_instructions(rpc, authority, vec![
      TokenAccountStrategy::WithoutBalance(pool.token_mint_a),
      TokenAccountStrategy::WithoutBalance(pool.token_mint_b),
  ])?;

  instructions.extend(token_accounts.create_instructions);

  let token_owner_account_a = token_accounts.token_account_addresses.get(&pool.token_mint_a).unwrap();
  let token_owner_account_b = token_accounts.token_account_addresses.get(&pool.token_mint_b).unwrap();

  instructions.push(
    DecreaseLiquidityV2 {
      whirlpool: position.whirlpool,
      token_program_a: mint_a_info.owner,
      token_program_b: mint_b_info.owner,
      memo_program: spl_memo::ID,
      position_authority: authority,
      position: position_address,
      position_token_account: position_token_account_address,
      token_mint_a: pool.token_mint_a,
      token_mint_b: pool.token_mint_b,
      token_owner_account_a: *token_owner_account_a,
      token_owner_account_b: *token_owner_account_b,
      token_vault_a: pool.token_vault_a,
      token_vault_b: pool.token_vault_b,
      tick_array_lower: lower_tick_array_address,
      tick_array_upper: upper_tick_array_address,
    }.instruction(DecreaseLiquidityV2InstructionArgs {
      liquidity_amount: quote.liquidity_delta,
      token_min_a: quote.token_min_a,
      token_min_b: quote.token_min_b,
      remaining_accounts_info: None,
    })
  );

  instructions.extend(token_accounts.cleanup_instructions);

  Ok(DecreaseLiquidityInstruction {
    quote,
    instructions,
    additional_signers: token_accounts.additional_signers,
  })
}


#[derive(Debug)]
pub struct ClosePositionInstruction {
  pub instructions: Vec<Instruction>,
  pub additional_signers: Vec<Keypair>,
  pub quote: DecreaseLiquidityQuote,
  pub fees_quote: CollectFeesQuote,
  pub rewards_quote: CollectRewardsQuote,
}


pub fn close_position_instructions(
  rpc: &RpcClient,
  position_mint_address: Pubkey,
  slippage_tolerance_bps: Option<u16>,
  authority: Option<Pubkey>,
) -> Result<ClosePositionInstruction, Box<dyn Error>> {
  let slippage_tolerance_bps = slippage_tolerance_bps.unwrap_or(*SLIPPAGE_TOLERANCE_BPS.try_lock()?);
  let authority = authority.unwrap_or(*FUNDER.try_lock()?);
  if authority != Pubkey::default() {
    return Err("Authority must be provided".into());
  }


}
