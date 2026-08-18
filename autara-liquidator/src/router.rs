use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use apl_token::state::Account as TokenAccount;
use arch_sdk::{
    AsyncArchRpcClient,
    arch_program::{
        account::AccountMeta, instruction::Instruction, program_pack::Pack, pubkey::Pubkey,
    },
};
use tokio::sync::RwLock;
use whirlpool_core::{
    TICK_ARRAY_SIZE, TickArrayFacade, TickArrays, TickFacade, WhirlpoolFacade,
    WhirlpoolRewardInfoFacade, get_tick_array_start_tick_index, swap_quote_by_input_token,
};

use crate::venue::{Venue, VenueQuote};

const WHIRLPOOL_DISCRIMINATOR: [u8; 8] = [63, 149, 209, 12, 225, 128, 99, 9];
const TICK_ARRAY_DISCRIMINATOR: [u8; 8] = [69, 97, 189, 190, 110, 7, 66, 187];
const SWAP_V2_DISCRIMINATOR: [u8; 8] = [43, 4, 237, 11, 26, 201, 30, 98];

#[derive(Debug, Clone)]
pub struct ClammExecution {
    pub pool: Pubkey,
    pub callback: Instruction,
}

#[derive(Debug, Clone)]
pub struct WhirlpoolState {
    pub whirlpools_config: Pubkey,
    pub tick_spacing: u16,
    pub fee_rate: u16,
    pub protocol_fee_rate: u16,
    pub token_mint_a: Pubkey,
    pub token_vault_a: Pubkey,
    pub token_mint_b: Pubkey,
    pub token_vault_b: Pubkey,
    pub facade: WhirlpoolFacade,
}

#[derive(Debug, Clone)]
pub struct SwapV2Params {
    pub program_id: Pubkey,
    pub token_program_a: Pubkey,
    pub token_program_b: Pubkey,
    pub token_authority: Pubkey,
    pub whirlpool: Pubkey,
    pub token_mint_a: Pubkey,
    pub token_mint_b: Pubkey,
    pub token_owner_account_a: Pubkey,
    pub token_vault_a: Pubkey,
    pub token_owner_account_b: Pubkey,
    pub token_vault_b: Pubkey,
    pub tick_arrays: [Pubkey; 5],
    pub oracle: Pubkey,
    pub amount: u64,
    pub other_amount_threshold: u64,
    pub a_to_b: bool,
}

type PoolCache = HashMap<(Pubkey, Pubkey), Vec<Pubkey>>;

/// A CLAMM compatibility boundary that deliberately uses lending's Arch 0.6.2
/// types while sharing only version-independent quote math with CLAMM.
pub struct SwapRouter {
    rpc: AsyncArchRpcClient,
    program_id: Pubkey,
    config_pubkey: Pubkey,
    slippage_bps: u16,
    pool_cache: RwLock<PoolCache>,
}

impl SwapRouter {
    pub fn new(
        rpc: AsyncArchRpcClient,
        program_id: Pubkey,
        config_pubkey: Pubkey,
        slippage_bps: u16,
    ) -> Result<Self> {
        if slippage_bps > 10_000 {
            bail!("CLAMM slippage_bps must not exceed 10000");
        }
        Ok(Self {
            rpc,
            program_id,
            config_pubkey,
            slippage_bps,
            pool_cache: RwLock::new(HashMap::new()),
        })
    }

    pub async fn add_static_pool(&self, token_a: Pubkey, token_b: Pubkey, pool: Pubkey) {
        let key = sort_pair(token_a, token_b);
        let mut cache = self.pool_cache.write().await;
        let pools = cache.entry(key).or_default();
        if !pools.contains(&pool) {
            pools.push(pool);
        }
        tracing::info!(?pool, token_a = ?key.0, token_b = ?key.1, "Pinned configured CLAMM pool");
    }

    pub async fn has_pair(&self, token_a: Pubkey, token_b: Pubkey) -> bool {
        self.pool_cache
            .read()
            .await
            .contains_key(&sort_pair(token_a, token_b))
    }

    pub async fn check_readiness(&self) -> Result<usize> {
        let program = self
            .rpc
            .read_account_info(self.program_id)
            .await
            .context("failed to read configured CLAMM program")?;
        if !program.is_executable {
            bail!("configured CLAMM program account is not executable");
        }
        let config = self
            .rpc
            .read_account_info(self.config_pubkey)
            .await
            .context("failed to read configured CLAMM config")?;
        if config.owner != self.program_id {
            bail!("configured CLAMM config is not owned by the configured program");
        }

        let configured: Vec<((Pubkey, Pubkey), Pubkey)> = self
            .pool_cache
            .read()
            .await
            .iter()
            .flat_map(|(pair, pools)| pools.iter().map(|pool| (*pair, *pool)))
            .collect();
        if configured.is_empty() {
            bail!("CLAMM has no configured pools");
        }
        let mut ready = 0usize;
        for ((token_x, token_y), pool_address) in configured {
            match self.check_pool(pool_address, token_x, token_y).await {
                Ok(()) => ready += 1,
                Err(error) => tracing::warn!(
                    ?pool_address,
                    %error,
                    "Configured CLAMM pool failed readiness"
                ),
            }
        }
        if ready == 0 {
            bail!("no configured CLAMM pool passed readiness");
        }
        Ok(ready)
    }

    pub async fn best_quote_exact_in(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount_in: u64,
        signer: Pubkey,
    ) -> Result<Option<VenueQuote<ClammExecution>>> {
        if amount_in == 0 || input_mint == output_mint {
            return Ok(None);
        }
        let pools = self
            .pool_cache
            .read()
            .await
            .get(&sort_pair(input_mint, output_mint))
            .cloned()
            .unwrap_or_default();

        let mut best: Option<VenueQuote<ClammExecution>> = None;
        for pool in pools {
            match self
                .quote_pool(pool, input_mint, output_mint, amount_in, signer)
                .await
            {
                Ok(quote) => {
                    if best
                        .as_ref()
                        .is_none_or(|previous| quote.estimated_out > previous.estimated_out)
                    {
                        best = Some(quote);
                    }
                }
                Err(error) => tracing::warn!(?pool, %error, "CLAMM pool quote rejected"),
            }
        }
        Ok(best)
    }

    async fn quote_pool(
        &self,
        pool_address: Pubkey,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount_in: u64,
        signer: Pubkey,
    ) -> Result<VenueQuote<ClammExecution>> {
        let pool_account = self
            .rpc
            .read_account_info(pool_address)
            .await
            .with_context(|| format!("failed to read CLAMM pool {pool_address:?}"))?;
        if pool_account.owner != self.program_id {
            bail!("CLAMM pool owner does not match configured program");
        }
        let pool = decode_whirlpool(&pool_account.data)?;
        if pool.whirlpools_config != self.config_pubkey {
            bail!("CLAMM pool config does not match configured config");
        }
        if pool.facade.liquidity == 0 {
            bail!("CLAMM pool has zero active liquidity");
        }
        if !((input_mint == pool.token_mint_a && output_mint == pool.token_mint_b)
            || (input_mint == pool.token_mint_b && output_mint == pool.token_mint_a))
        {
            bail!("CLAMM pool mints do not match requested pair");
        }

        let tick_array_keys = derive_tick_array_keys(
            &self.program_id,
            &pool_address,
            pool.facade.tick_current_index,
            pool.tick_spacing,
        )?;
        let tick_accounts = self
            .rpc
            .get_multiple_accounts(tick_array_keys.iter().map(|(key, _)| *key).collect())
            .await
            .context("failed to read CLAMM tick arrays")?;
        if tick_accounts.len() != tick_array_keys.len() {
            bail!("CLAMM RPC returned an unexpected tick-array count");
        }
        let mut tick_arrays = [uninitialized_tick_array(0); 5];
        for (index, ((expected_key, expected_start), account)) in
            tick_array_keys.iter().zip(tick_accounts.iter()).enumerate()
        {
            tick_arrays[index] = match account {
                Some(account) => {
                    if account.key != *expected_key || account.owner != self.program_id {
                        bail!("CLAMM tick-array account identity mismatch");
                    }
                    let decoded = decode_tick_array(&account.data, pool_address)?;
                    if decoded.start_tick_index != *expected_start {
                        bail!("CLAMM tick-array start index mismatch");
                    }
                    decoded
                }
                None => uninitialized_tick_array(*expected_start),
            };
        }

        let mint_accounts = self
            .rpc
            .get_multiple_accounts(vec![pool.token_mint_a, pool.token_mint_b])
            .await
            .context("failed to read CLAMM mint accounts")?;
        let mint_a = mint_accounts
            .first()
            .and_then(Option::as_ref)
            .context("CLAMM token A mint account is missing")?;
        let mint_b = mint_accounts
            .get(1)
            .and_then(Option::as_ref)
            .context("CLAMM token B mint account is missing")?;
        if mint_a.key != pool.token_mint_a || mint_b.key != pool.token_mint_b {
            bail!("CLAMM mint account identity mismatch");
        }

        let specified_token_a = input_mint == pool.token_mint_a;
        let quote = swap_quote_by_input_token(
            amount_in,
            specified_token_a,
            self.slippage_bps,
            pool.facade,
            TickArrays::from(tick_arrays),
            None,
            None,
        )
        .map_err(|error| anyhow!("CLAMM quote math failed: {error:?}"))?;
        if quote.token_est_out == 0 || quote.token_min_out == 0 {
            bail!("CLAMM quote produced zero output");
        }

        let token_owner_account_a =
            autara_lib::token::get_associated_token_address(&signer, &pool.token_mint_a);
        let token_owner_account_b =
            autara_lib::token::get_associated_token_address(&signer, &pool.token_mint_b);
        let oracle = derive_pda(&self.program_id, &[b"oracle", pool_address.as_ref()])?;
        let callback = build_swap_v2_callback(&SwapV2Params {
            program_id: self.program_id,
            token_program_a: mint_a.owner,
            token_program_b: mint_b.owner,
            token_authority: signer,
            whirlpool: pool_address,
            token_mint_a: pool.token_mint_a,
            token_mint_b: pool.token_mint_b,
            token_owner_account_a,
            token_vault_a: pool.token_vault_a,
            token_owner_account_b,
            token_vault_b: pool.token_vault_b,
            tick_arrays: tick_array_keys.map(|(key, _)| key),
            oracle,
            amount: quote.token_in,
            other_amount_threshold: quote.token_min_out,
            a_to_b: specified_token_a,
        })?;

        Ok(VenueQuote {
            venue: Venue::Clamm,
            amount_in,
            estimated_out: quote.token_est_out,
            execution: ClammExecution {
                pool: pool_address,
                callback,
            },
        })
    }

    async fn check_pool(
        &self,
        pool_address: Pubkey,
        token_x: Pubkey,
        token_y: Pubkey,
    ) -> Result<()> {
        let pool_account = self
            .rpc
            .read_account_info(pool_address)
            .await
            .context("failed to read CLAMM pool")?;
        if pool_account.owner != self.program_id {
            bail!("CLAMM pool owner mismatch");
        }
        let pool = decode_whirlpool(&pool_account.data)?;
        if pool.whirlpools_config != self.config_pubkey {
            bail!("CLAMM pool config mismatch");
        }
        if pool.facade.liquidity == 0 {
            bail!("CLAMM pool has zero active liquidity");
        }
        if sort_pair(pool.token_mint_a, pool.token_mint_b) != sort_pair(token_x, token_y) {
            bail!("CLAMM pool mints do not match configured pair");
        }
        let vaults = self
            .rpc
            .get_multiple_accounts(vec![pool.token_vault_a, pool.token_vault_b])
            .await
            .context("failed to read CLAMM vaults")?;
        let vault_a = decode_vault(
            vaults.first().and_then(Option::as_ref),
            pool.token_vault_a,
            pool.token_mint_a,
        )?;
        let vault_b = decode_vault(
            vaults.get(1).and_then(Option::as_ref),
            pool.token_vault_b,
            pool.token_mint_b,
        )?;
        if vault_a.amount == 0 || vault_b.amount == 0 {
            bail!("CLAMM pool has an empty token vault");
        }
        tracing::info!(
            ?pool_address,
            liquidity = pool.facade.liquidity,
            token_a_amount = vault_a.amount,
            token_b_amount = vault_b.amount,
            "CLAMM pool ready"
        );
        Ok(())
    }
}

fn decode_vault(
    account: Option<&arch_sdk::AccountInfoWithPubkey>,
    expected_key: Pubkey,
    expected_mint: Pubkey,
) -> Result<TokenAccount> {
    let account = account.context("CLAMM vault account is missing")?;
    if account.key != expected_key || account.owner != apl_token::id() {
        bail!("CLAMM vault identity or owner mismatch");
    }
    let vault = TokenAccount::unpack(&account.data).context("failed to decode CLAMM vault")?;
    if vault.mint != expected_mint {
        bail!("CLAMM vault mint mismatch");
    }
    Ok(vault)
}

pub fn decode_whirlpool(data: &[u8]) -> Result<WhirlpoolState> {
    let mut reader = WireReader::new(data);
    if reader.array::<8>()? != WHIRLPOOL_DISCRIMINATOR {
        bail!("invalid Whirlpool discriminator");
    }
    let whirlpools_config = reader.pubkey()?;
    reader.u8()?; // bump
    let tick_spacing = reader.u16()?;
    reader.array::<2>()?; // tick spacing seed
    let fee_rate = reader.u16()?;
    let protocol_fee_rate = reader.u16()?;
    let liquidity = reader.u128()?;
    let sqrt_price = reader.u128()?;
    let tick_current_index = reader.i32()?;
    reader.u64()?; // protocol fee owed A
    reader.u64()?; // protocol fee owed B
    let token_mint_a = reader.pubkey()?;
    let token_vault_a = reader.pubkey()?;
    let fee_growth_global_a = reader.u128()?;
    let token_mint_b = reader.pubkey()?;
    let token_vault_b = reader.pubkey()?;
    let fee_growth_global_b = reader.u128()?;
    let reward_last_updated_timestamp = reader.u64()?;
    let mut reward_infos = [WhirlpoolRewardInfoFacade::default(); 3];
    for reward in &mut reward_infos {
        reader.pubkey()?; // mint
        reader.pubkey()?; // vault
        reader.pubkey()?; // authority
        reward.emissions_per_second_x64 = reader.u128()?;
        reward.growth_global_x64 = reader.u128()?;
    }
    reader.finish()?;

    Ok(WhirlpoolState {
        whirlpools_config,
        tick_spacing,
        fee_rate,
        protocol_fee_rate,
        token_mint_a,
        token_vault_a,
        token_mint_b,
        token_vault_b,
        facade: WhirlpoolFacade {
            tick_spacing,
            fee_rate,
            protocol_fee_rate,
            liquidity,
            sqrt_price,
            tick_current_index,
            fee_growth_global_a,
            fee_growth_global_b,
            reward_last_updated_timestamp,
            reward_infos,
        },
    })
}

pub fn decode_tick_array(data: &[u8], expected_whirlpool: Pubkey) -> Result<TickArrayFacade> {
    let mut reader = WireReader::new(data);
    if reader.array::<8>()? != TICK_ARRAY_DISCRIMINATOR {
        bail!("invalid TickArray discriminator");
    }
    let start_tick_index = reader.i32()?;
    let mut ticks = [TickFacade::default(); TICK_ARRAY_SIZE];
    for tick in &mut ticks {
        tick.initialized = reader.bool()?;
        tick.liquidity_net = reader.i128()?;
        tick.liquidity_gross = reader.u128()?;
        tick.fee_growth_outside_a = reader.u128()?;
        tick.fee_growth_outside_b = reader.u128()?;
        for reward in &mut tick.reward_growths_outside {
            *reward = reader.u128()?;
        }
    }
    let whirlpool = reader.pubkey()?;
    reader.finish()?;
    if whirlpool != expected_whirlpool {
        bail!("TickArray belongs to a different Whirlpool");
    }
    Ok(TickArrayFacade {
        start_tick_index,
        ticks,
    })
}

pub fn build_swap_v2_callback(params: &SwapV2Params) -> Result<Instruction> {
    if params.amount == 0 || params.other_amount_threshold == 0 {
        bail!("CLAMM callback amount and threshold must be positive");
    }
    let mut data = Vec::with_capacity(49);
    data.extend_from_slice(&SWAP_V2_DISCRIMINATOR);
    data.extend_from_slice(&params.amount.to_le_bytes());
    data.extend_from_slice(&params.other_amount_threshold.to_le_bytes());
    data.extend_from_slice(&0u128.to_le_bytes());
    data.push(1); // amount_specified_is_input
    data.push(u8::from(params.a_to_b));
    data.push(1); // Some(RemainingAccountsInfo)
    data.extend_from_slice(&1u32.to_le_bytes());
    data.push(0); // AccountsType::SupplementalTickArrays
    data.push(2);

    let accounts = vec![
        AccountMeta::new_readonly(params.token_program_a, false),
        AccountMeta::new_readonly(params.token_program_b, false),
        AccountMeta::new_readonly(params.token_authority, true),
        AccountMeta::new(params.whirlpool, false),
        AccountMeta::new_readonly(params.token_mint_a, false),
        AccountMeta::new_readonly(params.token_mint_b, false),
        AccountMeta::new(params.token_owner_account_a, false),
        AccountMeta::new(params.token_vault_a, false),
        AccountMeta::new(params.token_owner_account_b, false),
        AccountMeta::new(params.token_vault_b, false),
        AccountMeta::new(params.tick_arrays[0], false),
        AccountMeta::new(params.tick_arrays[1], false),
        AccountMeta::new(params.tick_arrays[2], false),
        AccountMeta::new(params.oracle, false),
        AccountMeta::new_readonly(params.program_id, false),
        AccountMeta::new(params.tick_arrays[3], false),
        AccountMeta::new(params.tick_arrays[4], false),
    ];
    Ok(Instruction {
        program_id: params.program_id,
        accounts,
        data,
    })
}

fn sort_pair(a: Pubkey, b: Pubkey) -> (Pubkey, Pubkey) {
    if a < b { (a, b) } else { (b, a) }
}

fn uninitialized_tick_array(start_tick_index: i32) -> TickArrayFacade {
    TickArrayFacade {
        start_tick_index,
        ticks: [TickFacade::default(); TICK_ARRAY_SIZE],
    }
}

fn derive_tick_array_keys(
    program_id: &Pubkey,
    whirlpool: &Pubkey,
    current_tick_index: i32,
    tick_spacing: u16,
) -> Result<[(Pubkey, i32); 5]> {
    if tick_spacing == 0 {
        bail!("CLAMM tick spacing must be positive");
    }
    let start = get_tick_array_start_tick_index(current_tick_index, tick_spacing);
    let offset = i32::from(tick_spacing)
        .checked_mul(TICK_ARRAY_SIZE as i32)
        .context("CLAMM tick-array offset overflow")?;
    let indexes = [
        start,
        start.checked_add(offset).context("tick index overflow")?,
        start
            .checked_add(offset.checked_mul(2).context("tick index overflow")?)
            .context("tick index overflow")?,
        start.checked_sub(offset).context("tick index overflow")?,
        start
            .checked_sub(offset.checked_mul(2).context("tick index overflow")?)
            .context("tick index overflow")?,
    ];
    indexes
        .map(|index| {
            let index_string = index.to_string();
            derive_pda(
                program_id,
                &[b"tick_array", whirlpool.as_ref(), index_string.as_bytes()],
            )
            .map(|key| (key, index))
        })
        .into_iter()
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .map_err(|_| anyhow!("failed to build CLAMM tick-array key set"))
}

fn derive_pda(program_id: &Pubkey, seeds: &[&[u8]]) -> Result<Pubkey> {
    Pubkey::try_find_program_address(seeds, program_id)
        .map(|(key, _)| key)
        .context("failed to derive CLAMM PDA")
}

struct WireReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> WireReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self.offset.checked_add(N).context("wire offset overflow")?;
        let bytes = self
            .data
            .get(self.offset..end)
            .context("truncated CLAMM account data")?;
        self.offset = end;
        Ok(bytes.try_into().expect("slice length checked"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => bail!("invalid Borsh bool value {value}"),
        }
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn u128(&mut self) -> Result<u128> {
        Ok(u128::from_le_bytes(self.array()?))
    }

    fn i128(&mut self) -> Result<i128> {
        Ok(i128::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn pubkey(&mut self) -> Result<Pubkey> {
        Ok(Pubkey::from_slice(&self.array::<32>()?))
    }

    fn finish(self) -> Result<()> {
        if self.offset != self.data.len() {
            bail!("unexpected trailing CLAMM account data");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use arch_sdk::arch_program::pubkey::Pubkey;

    use super::{SwapV2Params, build_swap_v2_callback, decode_tick_array, decode_whirlpool};

    const WHIRLPOOL_DISCRIMINATOR: [u8; 8] = [63, 149, 209, 12, 225, 128, 99, 9];
    const TICK_ARRAY_DISCRIMINATOR: [u8; 8] = [69, 97, 189, 190, 110, 7, 66, 187];

    #[test]
    fn decodes_current_whirlpool_wire_layout() {
        let data = whirlpool_fixture();

        let pool = decode_whirlpool(&data).unwrap();

        assert_eq!(pool.whirlpools_config, key(1));
        assert_eq!(pool.tick_spacing, 64);
        assert_eq!(pool.fee_rate, 300);
        assert_eq!(pool.protocol_fee_rate, 25);
        assert_eq!(pool.facade.liquidity, 987_654_321);
        assert_eq!(pool.facade.sqrt_price, 123_456_789);
        assert_eq!(pool.facade.tick_current_index, 51_772);
        assert_eq!(pool.token_mint_a, key(2));
        assert_eq!(pool.token_vault_a, key(3));
        assert_eq!(pool.token_mint_b, key(4));
        assert_eq!(pool.token_vault_b, key(5));
        assert_eq!(pool.facade.fee_growth_global_a, 44);
        assert_eq!(pool.facade.fee_growth_global_b, 55);
        assert_eq!(pool.facade.reward_last_updated_timestamp, 66);
        assert_eq!(pool.facade.reward_infos[0].emissions_per_second_x64, 70);
        assert_eq!(pool.facade.reward_infos[2].growth_global_x64, 91);
    }

    #[test]
    fn rejects_wrong_whirlpool_discriminator_and_trailing_bytes() {
        let mut data = whirlpool_fixture();
        data[0] ^= 1;
        assert!(decode_whirlpool(&data).is_err());

        let mut data = whirlpool_fixture();
        data.push(0);
        assert!(decode_whirlpool(&data).is_err());
    }

    #[test]
    fn decodes_current_tick_array_wire_layout_and_pool_binding() {
        let data = tick_array_fixture();

        let array = decode_tick_array(&data, key(9)).unwrap();

        assert_eq!(array.start_tick_index, 50_688);
        assert!(array.ticks[0].initialized);
        assert_eq!(array.ticks[0].liquidity_net, -42);
        assert_eq!(array.ticks[0].liquidity_gross, 43);
        assert_eq!(array.ticks[0].fee_growth_outside_a, 44);
        assert_eq!(array.ticks[0].reward_growths_outside, [46, 47, 48]);
        assert!(!array.ticks[1].initialized);
        assert!(decode_tick_array(&data, key(8)).is_err());
    }

    #[test]
    fn builds_current_swap_v2_callback_byte_for_byte() {
        let params = SwapV2Params {
            program_id: key(1),
            token_program_a: key(2),
            token_program_b: key(3),
            token_authority: key(4),
            whirlpool: key(5),
            token_mint_a: key(6),
            token_mint_b: key(7),
            token_owner_account_a: key(8),
            token_vault_a: key(9),
            token_owner_account_b: key(10),
            token_vault_b: key(11),
            tick_arrays: [key(12), key(13), key(14), key(15), key(16)],
            oracle: key(17),
            amount: 0x0102_0304_0506_0708,
            other_amount_threshold: 0x1112_1314_1516_1718,
            a_to_b: false,
        };

        let instruction = build_swap_v2_callback(&params).unwrap();

        let mut expected_data = vec![43, 4, 237, 11, 26, 201, 30, 98];
        expected_data.extend_from_slice(&params.amount.to_le_bytes());
        expected_data.extend_from_slice(&params.other_amount_threshold.to_le_bytes());
        expected_data.extend_from_slice(&0u128.to_le_bytes());
        expected_data.push(1); // amount_specified_is_input
        expected_data.push(0); // a_to_b
        expected_data.push(1); // Some(remaining_accounts_info)
        expected_data.extend_from_slice(&1u32.to_le_bytes()); // one slice
        expected_data.push(0); // SupplementalTickArrays enum variant
        expected_data.push(2); // two supplemental arrays

        assert_eq!(instruction.program_id, params.program_id);
        assert_eq!(instruction.data, expected_data);
        assert_eq!(instruction.accounts.len(), 17);
        let expected_keys = [
            key(2),
            key(3),
            key(4),
            key(5),
            key(6),
            key(7),
            key(8),
            key(9),
            key(10),
            key(11),
            key(12),
            key(13),
            key(14),
            key(17),
            key(1),
            key(15),
            key(16),
        ];
        assert_eq!(
            instruction
                .accounts
                .iter()
                .map(|account| account.pubkey)
                .collect::<Vec<_>>(),
            expected_keys,
        );
        assert!(instruction.accounts[2].is_signer);
        assert!(!instruction.accounts[2].is_writable);
        for index in [3usize, 6, 7, 8, 9, 10, 11, 12, 13, 15, 16] {
            assert!(instruction.accounts[index].is_writable, "account {index}");
        }
        for index in [0usize, 1, 2, 4, 5, 14] {
            assert!(!instruction.accounts[index].is_writable, "account {index}");
        }
    }

    fn key(byte: u8) -> Pubkey {
        Pubkey::from_slice(&[byte; 32])
    }

    fn whirlpool_fixture() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&WHIRLPOOL_DISCRIMINATOR);
        push_key(&mut data, 1);
        data.push(255);
        data.extend_from_slice(&64u16.to_le_bytes());
        data.extend_from_slice(&64u16.to_le_bytes());
        data.extend_from_slice(&300u16.to_le_bytes());
        data.extend_from_slice(&25u16.to_le_bytes());
        data.extend_from_slice(&987_654_321u128.to_le_bytes());
        data.extend_from_slice(&123_456_789u128.to_le_bytes());
        data.extend_from_slice(&51_772i32.to_le_bytes());
        data.extend_from_slice(&11u64.to_le_bytes());
        data.extend_from_slice(&12u64.to_le_bytes());
        push_key(&mut data, 2);
        push_key(&mut data, 3);
        data.extend_from_slice(&44u128.to_le_bytes());
        push_key(&mut data, 4);
        push_key(&mut data, 5);
        data.extend_from_slice(&55u128.to_le_bytes());
        data.extend_from_slice(&66u64.to_le_bytes());
        for reward in 0..3u8 {
            push_key(&mut data, 20 + reward * 3);
            push_key(&mut data, 21 + reward * 3);
            push_key(&mut data, 22 + reward * 3);
            data.extend_from_slice(&(70u128 + u128::from(reward) * 10).to_le_bytes());
            data.extend_from_slice(&(71u128 + u128::from(reward) * 10).to_le_bytes());
        }
        assert_eq!(data.len(), 653);
        data
    }

    fn tick_array_fixture() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&TICK_ARRAY_DISCRIMINATOR);
        data.extend_from_slice(&50_688i32.to_le_bytes());
        for index in 0..88 {
            if index == 0 {
                data.push(1);
                data.extend_from_slice(&(-42i128).to_le_bytes());
                for value in 43u128..=48 {
                    data.extend_from_slice(&value.to_le_bytes());
                }
            } else {
                data.push(0);
                data.extend_from_slice(&[0; 16 * 7]);
            }
        }
        push_key(&mut data, 9);
        assert_eq!(data.len(), 9_988);
        data
    }

    fn push_key(data: &mut Vec<u8>, byte: u8) {
        data.extend_from_slice(&[byte; 32]);
    }
}
