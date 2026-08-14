use autara_client::client::{read::AutaraReadClient, tx_broadcast::AutaraClientError};
use autara_lib::{
    error::LendingError, event::AutaraEvent, math::ifixed_point::IFixedPoint,
    pda::find_borrow_position_pda, token::get_associated_token_address,
};

use crate::fixture::autara_fixture::{AutaraFixture, BTC, UNHEALTHY_LTV, USDC};

#[tokio::test]
async fn curator_can_manage_multiple_liquidators_and_restore_permissionless_mode() {
    let mut fixture = AutaraFixture::new().await;
    let market = fixture.create_market().await;
    let liquidator_one = *fixture.user_client().signer_pubkey();
    let liquidator_two = *fixture.user_two_client().signer_pubkey();

    fixture
        .curator_client()
        .add_whitelisted_liquidator(&market, liquidator_one)
        .await
        .unwrap();
    fixture
        .curator_client()
        .add_whitelisted_liquidator(&market, liquidator_two)
        .await
        .unwrap();
    fixture.reload().await;
    assert_eq!(
        fixture
            .user_client()
            .read_client()
            .get_market(&market)
            .unwrap()
            .market()
            .config()
            .active_whitelisted_liquidators(),
        2
    );

    fixture
        .curator_client()
        .remove_whitelisted_liquidator(&market, liquidator_one)
        .await
        .unwrap();
    fixture.reload().await;
    assert_eq!(
        fixture
            .user_client()
            .read_client()
            .get_market(&market)
            .unwrap()
            .market()
            .config()
            .active_whitelisted_liquidators(),
        1
    );

    fixture
        .curator_client()
        .remove_whitelisted_liquidator(&market, liquidator_two)
        .await
        .unwrap();
    fixture.reload().await;
    let market_state = fixture
        .user_client()
        .read_client()
        .get_market(&market)
        .unwrap();
    assert!(market_state
        .market()
        .config()
        .liquidations_are_permissionless());
}

#[tokio::test]
async fn cant_liquidate_healthy_position() {
    let mut fixture = AutaraFixture::new().await;
    let market = fixture.create_market().await;
    let deposit = USDC(1_000.);
    fixture
        .user_client()
        .supply(&market, deposit)
        .await
        .unwrap();
    fixture
        .user_client()
        .deposit_collateral(&market, BTC(0.1))
        .await
        .unwrap();
    fixture
        .user_client()
        .borrow(&market, USDC(0.1))
        .await
        .unwrap();
    fixture.reload_market(&market).await;
    let position = fixture
        .user_client()
        .read_client()
        .get_borrow_position(&market, fixture.user_client().signer_pubkey())
        .0;
    let err = fixture
        .user_client()
        .liquidate(&market, &position, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err, LendingError::PositionIsHealthy);
}

#[tokio::test]
async fn can_partially_liquidate_unhealthy_position() {
    let mut fixture = AutaraFixture::new().await;
    let market = fixture.create_market().await;

    let deposit = USDC(100_000.);
    fixture
        .user_client()
        .supply(&market, deposit)
        .await
        .unwrap();
    fixture
        .user_client()
        .deposit_collateral(&market, BTC(0.1))
        .await
        .unwrap();
    fixture
        .user_client()
        .borrow(&market, USDC(5000.))
        .await
        .unwrap();
    fixture.reload_market(&market).await;
    let position = fixture
        .user_client()
        .read_client()
        .get_borrow_position(&market, fixture.user_client().signer_pubkey())
        .0;
    let balance_before = fixture.fetch_user_balance().await;
    fixture.env().push_collateral_price(55000.).await.unwrap();
    fixture.reload_market(&market).await;
    let health_before_liquidation = fixture
        .user_client()
        .get_borrow_position_health(&market)
        .unwrap();
    assert!(health_before_liquidation.ltv < IFixedPoint::one());
    assert!(health_before_liquidation.ltv > UNHEALTHY_LTV);
    fixture
        .user_client()
        .liquidate(&market, &position, None, None, None)
        .await
        .unwrap();
    let balance_after = fixture.fetch_user_balance().await;
    fixture.reload_market(&market).await;
    let health_after_liquidation = fixture
        .user_client()
        .get_borrow_position_health(&market)
        .unwrap();
    assert!(!health_after_liquidation.ltv.is_zero());
    assert!(health_after_liquidation.ltv < health_before_liquidation.ltv);
    let diff = balance_after.delta(&balance_before);
    assert!(-diff.supply > 0);
    assert!(diff.collateral > 0);
}

#[tokio::test]
async fn can_fully_liquidate_unhealthy_position() {
    let mut fixture = AutaraFixture::new().await;
    let market = fixture.create_market().await;

    let deposit = USDC(100_000.);
    fixture
        .user_client()
        .supply(&market, deposit)
        .await
        .unwrap();
    fixture
        .user_client()
        .deposit_collateral(&market, BTC(0.1))
        .await
        .unwrap();
    fixture
        .user_client()
        .borrow(&market, USDC(5000.))
        .await
        .unwrap();
    fixture.reload_market(&market).await;
    let position = fixture
        .user_client()
        .read_client()
        .get_borrow_position(&market, fixture.user_client().signer_pubkey())
        .0;
    let balance_before = fixture.fetch_user_balance().await;
    fixture.env().push_collateral_price(1.).await.unwrap();
    fixture.reload_market(&market).await;
    let health_before_liquidation = fixture
        .user_client()
        .get_borrow_position_health(&market)
        .unwrap();
    assert!(health_before_liquidation.ltv > IFixedPoint::one());
    fixture
        .user_client()
        .liquidate(&market, &position, None, None, None)
        .await
        .unwrap();
    let balance_after = fixture.fetch_user_balance().await;
    fixture.reload_market(&market).await;
    let health_after_liquidation = fixture
        .user_client()
        .get_borrow_position_health(&market)
        .unwrap();
    assert!(health_after_liquidation.ltv.is_zero());
    assert!(health_after_liquidation.ltv < health_before_liquidation.ltv);
    let diff = balance_after.delta(&balance_before);
    assert!(-diff.supply > 0);
    assert_eq!(
        diff.collateral,
        health_before_liquidation.collateral_atoms as i64
    );
}

#[tokio::test]
async fn can_liquidate_with_callback_unhealthy_position() {
    let mut fixture = AutaraFixture::new().await;
    let market = fixture.create_market().await;
    let deposit = USDC(100_000.);
    fixture
        .user_client()
        .supply(&market, deposit)
        .await
        .unwrap();
    fixture
        .user_client()
        .deposit_collateral(&market, BTC(0.1))
        .await
        .unwrap();
    fixture
        .user_client()
        .borrow(&market, USDC(5000.))
        .await
        .unwrap();
    fixture.reload_market(&market).await;
    let position = fixture
        .user_client()
        .read_client()
        .get_borrow_position(&market, fixture.user_client().signer_pubkey())
        .0;
    let balance_before = fixture
        .fetch_balance(fixture.user_two_client().signer_pubkey())
        .await;
    fixture.env().push_collateral_price(55000.).await.unwrap();
    fixture.reload_market(&market).await;
    let health_before_liquidation = fixture
        .user_client()
        .get_borrow_position_health(&market)
        .unwrap();
    assert!(health_before_liquidation.ltv < IFixedPoint::one());
    assert!(health_before_liquidation.ltv > UNHEALTHY_LTV);
    let transfer = 123456;
    let callback = apl_token::instruction::transfer(
        &apl_token::id(),
        &get_associated_token_address(
            fixture.user_client().signer_pubkey(),
            &fixture.env().supply_mint,
        ),
        &get_associated_token_address(
            fixture.user_two_client().signer_pubkey(),
            &fixture.env().supply_mint,
        ),
        fixture.user_client().signer_pubkey(),
        &[],
        transfer,
    )
    .unwrap();
    fixture
        .user_client()
        .liquidate(&market, &position, None, None, Some(callback))
        .await
        .unwrap();
    let balance_after = fixture
        .fetch_balance(fixture.user_two_client().signer_pubkey())
        .await;
    fixture.reload_market(&market).await;
    let health_after_liquidation = fixture
        .user_client()
        .get_borrow_position_health(&market)
        .unwrap();
    assert!(!health_after_liquidation.ltv.is_zero());
    assert!(health_after_liquidation.ltv < health_before_liquidation.ltv);
    let diff = balance_after.delta(&balance_before);
    assert_eq!(diff.supply, transfer as i64)
}

#[tokio::test]
async fn cant_liquidate_twice_with_callback_unhealthy_position() {
    let mut fixture = AutaraFixture::new().await;
    let market = fixture.create_market().await;
    let deposit = USDC(100_000.);
    fixture
        .user_client()
        .supply(&market, deposit)
        .await
        .unwrap();
    fixture
        .user_client()
        .deposit_collateral(&market, BTC(0.1))
        .await
        .unwrap();
    fixture
        .user_client()
        .borrow(&market, USDC(5000.))
        .await
        .unwrap();
    fixture.reload_market(&market).await;
    let position = fixture
        .user_client()
        .read_client()
        .get_borrow_position(&market, fixture.user_client().signer_pubkey())
        .0;
    fixture.env().push_collateral_price(55000.).await.unwrap();
    fixture.reload_market(&market).await;
    let health_before_liquidation = fixture
        .user_client()
        .get_borrow_position_health(&market)
        .unwrap();
    assert!(health_before_liquidation.ltv < IFixedPoint::one());
    assert!(health_before_liquidation.ltv > UNHEALTHY_LTV);
    let market_w = fixture
        .user_client()
        .read_client()
        .get_market(&market)
        .unwrap();
    let oracles = market_w.market().get_oracle_keys();
    let callback = autara_lib::ixs::liquidate_ix(
        fixture.env().autara_program_pubkey,
        market,
        find_borrow_position_pda(
            &fixture.env().autara_program_pubkey,
            &market,
            fixture.user_client().signer_pubkey(),
        )
        .0,
        *fixture.user_client().signer_pubkey(),
        get_associated_token_address(
            fixture.user_client().signer_pubkey(),
            &fixture.env().supply_mint,
        ),
        get_associated_token_address(
            fixture.user_client().signer_pubkey(),
            &fixture.env().collateral_mint,
        ),
        *market_w.market().supply_vault().vault(),
        *market_w.market().collateral_vault().vault(),
        oracles.0,
        oracles.1,
        u64::MAX,
        0,
        None,
        None,
    );
    let err = fixture
        .user_client()
        .liquidate(&market, &position, None, None, Some(callback))
        .await
        .unwrap_err();
    let AutaraClientError::AutaraTxError { events, .. } = &err else {
        panic!("Expected AutaraTxError, got: {:?}", err);
    };
    let first_event = events.events.first().unwrap();
    // check first liquidate did happen
    assert!(matches!(first_event, AutaraEvent::Liquidate(_)));
    assert_eq!(err, LendingError::PositionIsHealthy);
}
