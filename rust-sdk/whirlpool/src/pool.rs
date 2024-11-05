use std::error::Error;

use orca_whirlpools_client::{accounts::{FeeTier, Whirlpool, WhirlpoolsConfig}, get_fee_tier_address, get_whirlpool_address, types::WhirlpoolRewardInfo, programs::WHIRLPOOL_ID};

use orca_whirlpools_core::sqrt_price_to_price;
use solana_account_decoder::UiAccountEncoding;
use solana_client::{rpc_client::RpcClient, rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig}, rpc_filter::{Memcmp, RpcFilterType}};
use solana_program::pubkey::Pubkey;
use solana_sdk::{program_error::ProgramError, program_pack::Pack};
use spl_token::state::Mint;

use crate::{token::order_mints, SPLASH_POOL_TICK_SPACING, WHIRLPOOLS_CONFIG_ADDRESS};

#[derive(Debug, Clone)]
pub struct UninitializedPool {
  pub whirlpools_config: Pubkey,
  pub tick_spacing: u16,
  pub fee_rate: u16,
  pub protocol_fee_rate: u16,
  pub token_mint_a: Pubkey,
  pub token_mint_b: Pubkey,
}

#[derive(Debug, Clone)]
pub struct InitializedPool {
  pub discriminator: [u8; 8],
  pub whirlpools_config: Pubkey,
  pub whirlpool_bump: [u8; 1],
  pub tick_spacing: u16,
  pub tick_spacing_seed: [u8; 2],
  pub fee_rate: u16,
  pub protocol_fee_rate: u16,
  pub liquidity: u128,
  pub sqrt_price: u128,
  pub price: f64,
  pub tick_current_index: i32,
  pub protocol_fee_owed_a: u64,
  pub protocol_fee_owed_b: u64,
  pub token_mint_a: Pubkey,
  pub token_mint_b: Pubkey,
  pub token_vault_a: Pubkey,
  pub token_vault_b: Pubkey,
  pub fee_growth_global_a: u128,
  pub fee_growth_global_b: u128,
  pub reward_last_updated_timestamp: u64,
  pub reward_infos: [WhirlpoolRewardInfo; 3],
}

impl Into<Whirlpool> for InitializedPool {
  fn into(self) -> Whirlpool {
    Whirlpool {
      discriminator: self.discriminator,
      whirlpools_config: self.whirlpools_config,
      whirlpool_bump: self.whirlpool_bump,
      tick_spacing: self.tick_spacing,
      tick_spacing_seed: self.tick_spacing_seed,
      fee_rate: self.fee_rate,
      protocol_fee_rate: self.protocol_fee_rate,
      liquidity: self.liquidity,
      sqrt_price: self.sqrt_price,
      tick_current_index: self.tick_current_index,
      protocol_fee_owed_a: self.protocol_fee_owed_a,
      protocol_fee_owed_b: self.protocol_fee_owed_b,
      token_mint_a: self.token_mint_a,
      token_vault_a: self.token_vault_a,
      fee_growth_global_a: self.fee_growth_global_a,
      token_mint_b: self.token_mint_b,
      token_vault_b: self.token_vault_b,
      fee_growth_global_b: self.fee_growth_global_b,
      reward_last_updated_timestamp: self.reward_last_updated_timestamp,
      reward_infos: self.reward_infos,
    }
  }
}

impl InitializedPool {
  fn from_bytes(bytes: &[u8], mint_a: Mint, mint_b: Mint) -> Result<Self, Box<dyn Error>> {
    let whirlpool = Whirlpool::from_bytes(&bytes)?;
    let price = sqrt_price_to_price(whirlpool.sqrt_price, mint_a.decimals, mint_b.decimals);
    Ok(InitializedPool {
      discriminator: whirlpool.discriminator,
      whirlpools_config: whirlpool.whirlpools_config,
      whirlpool_bump: whirlpool.whirlpool_bump,
      tick_spacing: whirlpool.tick_spacing,
      tick_spacing_seed: whirlpool.tick_spacing_seed,
      fee_rate: whirlpool.fee_rate,
      protocol_fee_rate: whirlpool.protocol_fee_rate,
      token_mint_a: whirlpool.token_mint_a,
      token_mint_b: whirlpool.token_mint_b,
    liquidity: whirlpool.liquidity,
    sqrt_price: whirlpool.sqrt_price,
    price,
    tick_current_index: whirlpool.tick_current_index,
    protocol_fee_owed_a: whirlpool.protocol_fee_owed_a,
    protocol_fee_owed_b: whirlpool.protocol_fee_owed_b,
    token_vault_a: whirlpool.token_vault_a,
    token_vault_b: whirlpool.token_vault_b,
    fee_growth_global_a: whirlpool.fee_growth_global_a,
    fee_growth_global_b: whirlpool.fee_growth_global_b,
    reward_last_updated_timestamp: whirlpool.reward_last_updated_timestamp,
    reward_infos: whirlpool.reward_infos,
  })
  }
}

#[derive(Debug, Clone)]
pub enum PoolInfo {
  Initialized(InitializedPool),
  Uninitialized(UninitializedPool),
}


pub fn fetch_splash_pool(rpc: &RpcClient, token_1: Pubkey, token_2: Pubkey) -> Result<PoolInfo, Box<dyn Error>> {
  fetch_concentrated_liquidity_pool(rpc, token_1, token_2, SPLASH_POOL_TICK_SPACING)
}

pub fn fetch_concentrated_liquidity_pool(rpc: &RpcClient, token_1: Pubkey, token_2: Pubkey, tick_spacing: u16) -> Result<PoolInfo, Box<dyn Error>> {
  let whirlpools_config_address = &*WHIRLPOOLS_CONFIG_ADDRESS.try_lock()?;
  let [token_a, token_b] = order_mints(token_1, token_2);
  let whirlpool_pda = get_whirlpool_address(
    whirlpools_config_address,
    &token_a,
      &token_b,
      tick_spacing
    )?;

  let fee_tier_address = get_fee_tier_address(whirlpools_config_address, tick_spacing)?;

  let account_infos = rpc.get_multiple_accounts(&[whirlpool_pda.0, *whirlpools_config_address, fee_tier_address.0, token_a, token_b])?;

  let whirlpools_config_info = account_infos[1]
    .as_ref()
    .ok_or(format!("Whirlpools config {} not found", whirlpools_config_address))?;
  let whirlpools_config = WhirlpoolsConfig::from_bytes(&whirlpools_config_info.data)?;

  let fee_tier_info = account_infos[2]
    .as_ref()
    .ok_or(format!("Fee tier {} not found", fee_tier_address.0))?;
  let fee_tier = FeeTier::from_bytes(&fee_tier_info.data)?;

  let mint_a_info = account_infos[3]
    .as_ref()
    .ok_or(format!("Mint {} not found", token_a))?;
  let mint_a = Mint::unpack(&mint_a_info.data)?;

  let mint_b_info = account_infos[4]
    .as_ref()
    .ok_or(format!("Mint {} not found", token_b))?;
  let mint_b = Mint::unpack(&mint_b_info.data)?;


  if let Some(whirlpool_info) = &account_infos[0] {
    let initialized_pool = InitializedPool::from_bytes(&whirlpool_info.data, mint_a, mint_b)?;
    Ok(PoolInfo::Initialized(initialized_pool))
  } else {
    Ok(PoolInfo::Uninitialized(UninitializedPool {
      whirlpools_config: *whirlpools_config_address,
      tick_spacing,
      fee_rate: fee_tier.default_fee_rate,
      protocol_fee_rate: whirlpools_config.default_protocol_fee_rate,
      token_mint_a: token_a,
      token_mint_b: token_b,
    }))
  }
}

pub fn fetch_whirlpools_by_token_pair(rpc: &RpcClient, token_1: Pubkey, token_2: Pubkey) -> Result<Vec<PoolInfo>, Box<dyn Error>> {
  let whirlpools_config_address = &*WHIRLPOOLS_CONFIG_ADDRESS.try_lock()?;
  let [token_a, token_b] = order_mints(token_1, token_2);

  let discriminator_filter = Memcmp::new_base58_encoded(
    0,
    &[1u8; 8].as_ref(),
  );

  let whirlpools_config_filter = Memcmp::new_base58_encoded(
    40,
    &[1u8; 165].as_ref(),
  );

  let fee_tiers: Vec<FeeTier> = rpc.get_program_accounts_with_config(&WHIRLPOOL_ID, RpcProgramAccountsConfig {
    filters: Some(vec![
      RpcFilterType::Memcmp(discriminator_filter),
      RpcFilterType::Memcmp(whirlpools_config_filter),
    ]),
    account_config: RpcAccountInfoConfig {
      encoding: Some(UiAccountEncoding::Base64),
      ..Default::default()
    },
    ..Default::default()
  })?
  .iter()
  .map(|x| FeeTier::from_bytes(&x.1.data))
  .collect::<Result<Vec<FeeTier>, _>>()?;

  let account_infos = rpc.get_multiple_accounts(&[*whirlpools_config_address, token_a, token_b])?;

  let whirlpools_config_info = account_infos[0]
    .as_ref()
    .ok_or(format!("Whirlpools config {} not found", whirlpools_config_address))?;
  let whirlpools_config = WhirlpoolsConfig::from_bytes(&whirlpools_config_info.data)?;

  let mint_a_info = account_infos[1]
    .as_ref()
    .ok_or(format!("Mint {} not found", token_a))?;
  let mint_a = Mint::unpack(&mint_a_info.data)?;

  let mint_b_info = account_infos[2]
    .as_ref()
    .ok_or(format!("Mint {} not found", token_b))?;
  let mint_b = Mint::unpack(&mint_b_info.data)?;


let whirlpool_addresses: Vec<Pubkey> = fee_tiers.iter()
  .map(|fee_tier| fee_tier.tick_spacing)
  .map(|tick_spacing| get_whirlpool_address(whirlpools_config_address, &token_a, &token_b, tick_spacing))
  .map(|x| x.map(|y| y.0))
  .collect::<Result<Vec<Pubkey>, ProgramError>>()?;

let whirlpool_infos = rpc.get_multiple_accounts(&whirlpool_addresses)?;

  let mut whirlpools: Vec<PoolInfo> = Vec::new();
  for i in 0..whirlpool_infos.len() {
    let pool_info = whirlpool_infos[i].as_ref();
    let fee_tier = &fee_tiers[i];

    if let Some(pool_info) = pool_info {
      let initialized_pool = InitializedPool::from_bytes(&pool_info.data, mint_a, mint_b)?;
      whirlpools.push(PoolInfo::Initialized(initialized_pool));
    } else {
      whirlpools.push(PoolInfo::Uninitialized(UninitializedPool {
        whirlpools_config: *whirlpools_config_address,
        tick_spacing: fee_tier.tick_spacing,
        fee_rate: fee_tier.default_fee_rate,
        protocol_fee_rate: whirlpools_config.default_protocol_fee_rate,
        token_mint_a: token_a,
        token_mint_b: token_b,
      }));
    }
  }

  Ok(whirlpools)
}

