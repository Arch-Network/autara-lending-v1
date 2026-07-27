use std::ops::Deref;

use arch_program::account::AccountInfo;
use arch_program::program_pack::Pack;
use arch_program::pubkey::Pubkey;
use autara_lib::oracle::oracle_config::OracleConfig;
use autara_lib::state::borrow_position::BorrowPosition;
use autara_lib::state::market::Market;
use autara_lib::state::market_config::LtvConfig;
use autara_lib::state::supply_position::SupplyPosition;
use autara_lib::token::get_associated_token_address;
use autara_program_lib::accounts::token::TokenAccount;
use bytemuck::Zeroable;

pub struct AutaraAccounts {
    pub global_admin: AccountInfoWrapper,
    pub nominated_admin: AccountInfoWrapper,
    pub market: AccountInfoWrapper,
    pub curator: AccountInfoWrapper,
    pub global_config: AccountInfoWrapper,
    pub user: AccountInfoWrapper,
    pub supply_position: AccountInfoWrapper,
    pub borrow_position: AccountInfoWrapper,
    pub market_supply_vault: AccountInfoWrapper,
    pub market_collateral_vault: AccountInfoWrapper,
    pub user_supply_ata: AccountInfoWrapper,
    pub user_collateral_ata: AccountInfoWrapper,
    pub apl_token_program: AccountInfoWrapper,
    pub oracle: AccountInfoWrapper,
}

impl AutaraAccounts {
    pub fn new() -> Self {
        let global_admin = create_signer();
        let nominated_admin = create_signer();
        let mut global_config_data = autara_lib::state::global_config::GlobalConfig::new(
            *global_admin.key,
            *global_admin.key,
            0,
        );
        global_config_data.set_nominated_admin(*nominated_admin.key);
        let (global_config_pda, _) = autara_lib::pda::find_global_config_pda(&crate::id());
        let global_config = create_autara_account(global_config_pda, global_config_data);
        let curator = create_signer();
        let mut market = Market::zeroed();
        let market_pubkey = Pubkey::new_unique();
        market
            .config_mut()
            .initialize(
                0,
                0,
                curator.key,
                &LtvConfig {
                    max_ltv: 0.5.into(),
                    unhealthy_ltv: 0.6.into(),
                    liquidation_bonus: 0.05.into(),
                },
                0.05.into(),
                1000000000000,
                0,
                &global_config_data,
            )
            .unwrap();
        let collateral_mint_address = Pubkey::new_unique();
        market
            .initialize_collateral_vault(
                collateral_mint_address,
                9,
                get_associated_token_address(&market_pubkey, &collateral_mint_address),
                OracleConfig::new_pyth([1; 32], Pubkey::new_unique()),
            )
            .unwrap();
        let supply_mint_address = Pubkey::new_unique();
        market
            .initlize_supply_vault(
                supply_mint_address,
                9,
                get_associated_token_address(&market_pubkey, &supply_mint_address),
                OracleConfig::new_pyth([2; 32], Pubkey::new_unique()),
                Default::default(),
                Default::default(),
            )
            .unwrap();
        let market = create_autara_account(market_pubkey, market);
        let user = create_signer();
        let supply_position = create_autara_account(
            Pubkey::new_unique(),
            SupplyPosition::new(*user.key, *market.key),
        );
        let borrow_position = create_autara_account(Pubkey::new_unique(), {
            let mut position = BorrowPosition::zeroed();
            position.initialize(*user.key, *market.key);
            position
        });
        let market_supply_vault = create_associated_token_account(market.key, &supply_mint_address);
        let market_collateral_vault =
            create_associated_token_account(market.key, &collateral_mint_address);
        let user_supply_ata = create_associated_token_account(user.key, &supply_mint_address);
        let user_collateral_ata =
            create_associated_token_account(user.key, &collateral_mint_address);
        Self {
            market,
            curator,
            user,
            supply_position,
            borrow_position,
            market_supply_vault,
            market_collateral_vault,
            user_supply_ata,
            user_collateral_ata,
            apl_token_program: create_token_program(),
            global_admin,
            global_config,
            nominated_admin,
            oracle: create_signer(),
        }
    }
}

pub struct AccountInfoWrapper(pub AccountInfo<'static>);

impl Deref for AccountInfoWrapper {
    type Target = AccountInfo<'static>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AccountInfoWrapper {
    pub fn mutate_owner(&mut self) {
        self.0.owner = Box::leak(Box::new(Pubkey::new_unique()));
    }

    pub fn non_signer(&mut self) {
        self.0.is_signer = false;
    }
}

pub fn create_signer() -> AccountInfoWrapper {
    let key = Box::leak(Box::new(arch_program::pubkey::Pubkey::new_unique()));
    let lamports = Box::leak(Box::new(1_000_000u64));
    let account_data = Box::leak(Box::new(vec![0; 1]));
    AccountInfoWrapper(AccountInfo::new(
        key,
        lamports,
        account_data,
        Box::leak(Box::new(Default::default())),
        Box::leak(Box::new(Default::default())),
        true,
        true,
        false,
    ))
}

pub fn create_autara_account<T>(key: Pubkey, data: T) -> AccountInfoWrapper
where
    T: bytemuck::Pod,
{
    let key = Box::leak(Box::new(key));
    let lamports = Box::leak(Box::new(1_000_000u64));
    let account_data = Box::leak(Box::new(bytemuck::bytes_of(&data).to_vec()));
    AccountInfoWrapper(AccountInfo::new(
        key,
        lamports,
        account_data,
        Box::leak(Box::new(crate::id())),
        Box::leak(Box::new(Default::default())),
        true,
        true,
        false,
    ))
}

pub fn create_associated_token_account(
    owner: &arch_program::pubkey::Pubkey,
    mint: &arch_program::pubkey::Pubkey,
) -> AccountInfoWrapper {
    let key = Box::leak(Box::new(get_associated_token_address(owner, mint)));
    let lamports = Box::leak(Box::new(1_000_000u64));
    let account_data = Box::leak(Box::new(vec![0; TokenAccount::LEN]));
    TokenAccount::pack(
        TokenAccount {
            mint: *mint,
            owner: *owner,
            amount: 1 << 50,
            state: apl_token::state::AccountState::Initialized,
            ..Default::default()
        },
        &mut account_data.as_mut_slice(),
    )
    .expect("Failed to pack token account data");
    AccountInfoWrapper(AccountInfo::new(
        key,
        lamports,
        account_data,
        Box::leak(Box::new(apl_token::id())),
        Box::leak(Box::new(Default::default())),
        true,
        true,
        false,
    ))
}

/// A token account living at an arbitrary address (not the canonical ATA), owned by
/// `token_owner`. Used to check that market vaults are pinned to the market's own ATA.
pub fn create_token_account_at(
    key: arch_program::pubkey::Pubkey,
    token_owner: &arch_program::pubkey::Pubkey,
    mint: &arch_program::pubkey::Pubkey,
) -> AccountInfoWrapper {
    let key = Box::leak(Box::new(key));
    let lamports = Box::leak(Box::new(1_000_000u64));
    let account_data = Box::leak(Box::new(vec![0; TokenAccount::LEN]));
    TokenAccount::pack(
        TokenAccount {
            mint: *mint,
            owner: *token_owner,
            amount: 0,
            state: apl_token::state::AccountState::Initialized,
            ..Default::default()
        },
        &mut account_data.as_mut_slice(),
    )
    .expect("Failed to pack token account data");
    AccountInfoWrapper(AccountInfo::new(
        key,
        lamports,
        account_data,
        Box::leak(Box::new(apl_token::id())),
        Box::leak(Box::new(Default::default())),
        false,
        true,
        false,
    ))
}

pub fn create_mint(key: arch_program::pubkey::Pubkey, decimals: u8) -> AccountInfoWrapper {
    let key = Box::leak(Box::new(key));
    let lamports = Box::leak(Box::new(1_000_000u64));
    let account_data = Box::leak(Box::new(vec![0; apl_token::state::Mint::LEN]));
    apl_token::state::Mint::pack(
        apl_token::state::Mint {
            decimals,
            is_initialized: true,
            ..Default::default()
        },
        &mut account_data.as_mut_slice(),
    )
    .expect("Failed to pack mint data");
    AccountInfoWrapper(AccountInfo::new(
        key,
        lamports,
        account_data,
        Box::leak(Box::new(apl_token::id())),
        Box::leak(Box::new(Default::default())),
        false,
        false,
        false,
    ))
}

pub fn create_program_account(key: arch_program::pubkey::Pubkey) -> AccountInfoWrapper {
    let key = Box::leak(Box::new(key));
    let lamports = Box::leak(Box::new(1_000_000u64));
    let account_data = Box::leak(Box::new(vec![0; 1]));
    AccountInfoWrapper(AccountInfo::new(
        key,
        lamports,
        account_data,
        Box::leak(Box::new(Default::default())),
        Box::leak(Box::new(Default::default())),
        false,
        false,
        true,
    ))
}

pub fn create_token_program() -> AccountInfoWrapper {
    let key = Box::leak(Box::new(apl_token::id()));
    let lamports = Box::leak(Box::new(1_000_000u64));
    let account_data = Box::leak(Box::new(vec![0; 1]));
    AccountInfoWrapper(AccountInfo::new(
        key,
        lamports,
        account_data,
        Box::leak(Box::new(Default::default())),
        Box::leak(Box::new(Default::default())),
        false,
        false,
        true,
    ))
}
