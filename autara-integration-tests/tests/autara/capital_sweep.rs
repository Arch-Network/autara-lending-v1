use arch_sdk::arch_program::pubkey::Pubkey;
use autara_client::client::read::AutaraReadClient;
use autara_lib::{
    error::LendingError,
    event::{AutaraEvent, AutaraEvents, CapitalSweepSettledEvent, CapitalSweepStartedEvent},
    state::borrow_position::BorrowPosition,
};
use autara_program::error::LendingAccountValidationError;

use crate::fixture::autara_fixture::{AutaraFixture, BTC, USDC};

fn borrow_position(fixture: &AutaraFixture, market: &Pubkey) -> (Pubkey, BorrowPosition) {
    let (key, position) = fixture
        .user_client()
        .read_client()
        .get_borrow_position(market, fixture.user_client().signer_pubkey());
    (key, *position.expect("borrow position should exist"))
}

fn started_event(events: &AutaraEvents) -> CapitalSweepStartedEvent {
    events
        .events
        .iter()
        .find_map(|event| match event {
            AutaraEvent::CapitalSweepStarted(event) => Some(*event),
            _ => None,
        })
        .expect("capital sweep started event should exist")
}

fn settled_event(events: &AutaraEvents) -> CapitalSweepSettledEvent {
    events
        .events
        .iter()
        .find_map(|event| match event {
            AutaraEvent::CapitalSweepSettled(event) => Some(*event),
            _ => None,
        })
        .expect("capital sweep settled event should exist")
}

async fn unhealthy_position() -> (AutaraFixture, Pubkey, Pubkey, BorrowPosition) {
    let mut fixture = AutaraFixture::new().await;
    let market = fixture.create_market().await;
    fixture
        .user_client()
        .supply(&market, USDC(100_000.))
        .await
        .unwrap();
    fixture
        .user_client()
        .deposit_collateral(&market, BTC(0.1))
        .await
        .unwrap();
    fixture
        .user_client()
        .borrow(&market, USDC(5_000.))
        .await
        .unwrap();
    fixture.env().push_collateral_price(55_000.).await.unwrap();
    fixture.reload_market(&market).await;
    let (position_key, position) = borrow_position(&fixture, &market);
    (fixture, market, position_key, position)
}

#[tokio::test]
async fn sweep_and_partial_settlement_move_real_tokens_and_unlock_position() {
    let (mut fixture, market, position_key, position_before) = unhealthy_position().await;
    let curator = *fixture.curator_client().signer_pubkey();
    let curator_before = fixture.fetch_balance(&curator).await;

    let begin_events = fixture
        .curator_client()
        .begin_capital_sweep(&market, &position_key)
        .await
        .unwrap();
    let begin_event = started_event(&begin_events);
    assert_eq!(begin_event.collateral_swept, BTC(0.1));

    fixture.reload_market(&market).await;
    let (_, pending) = borrow_position(&fixture, &market);
    let curator_after_begin = fixture.fetch_balance(&curator).await;
    assert_eq!(pending.collateral_deposited_atoms(), 0);
    assert_eq!(pending.swept_collateral_atoms(), BTC(0.1));
    assert_eq!(pending.borrowed_shares(), position_before.borrowed_shares());
    assert_eq!(
        curator_after_begin.collateral - curator_before.collateral,
        BTC(0.1)
    );
    assert_eq!(
        fixture
            .user_client()
            .repay(&market, Some(USDC(1.)))
            .await
            .unwrap_err(),
        LendingError::CapitalSweepPending
    );

    let settle_events = fixture
        .curator_client()
        .settle_capital_sweep(&market, &position_key, Some(USDC(100.)), None)
        .await
        .unwrap();
    let settlement = settled_event(&settle_events);
    assert_eq!(settlement.supply_repaid, USDC(100.));
    assert!(settlement.collateral_returned > 0);
    assert!(settlement.collateral_returned < begin_event.collateral_swept);

    fixture.reload_market(&market).await;
    let (_, after) = borrow_position(&fixture, &market);
    let curator_after_settle = fixture.fetch_balance(&curator).await;
    assert_eq!(after.swept_collateral_atoms(), 0);
    assert_eq!(
        after.collateral_deposited_atoms(),
        settlement.collateral_returned
    );
    assert!(after.borrowed_shares() < position_before.borrowed_shares());
    assert_eq!(
        curator_after_begin.supply - curator_after_settle.supply,
        settlement.supply_repaid
    );
    assert_eq!(
        curator_after_begin.collateral - curator_after_settle.collateral,
        settlement.collateral_returned
    );
}

#[tokio::test]
async fn healthy_oracle_recovery_returns_all_collateral_without_repaying_debt() {
    let (mut fixture, market, position_key, position_before) = unhealthy_position().await;
    let curator = *fixture.curator_client().signer_pubkey();
    fixture
        .curator_client()
        .begin_capital_sweep(&market, &position_key)
        .await
        .unwrap();
    let curator_after_begin = fixture.fetch_balance(&curator).await;

    fixture.env().push_collateral_price(100_000.).await.unwrap();
    fixture.reload_market(&market).await;
    let events = fixture
        .curator_client()
        .settle_capital_sweep(&market, &position_key, None, None)
        .await
        .unwrap();
    let settlement = settled_event(&events);
    assert_eq!(settlement.supply_repaid, 0);
    assert_eq!(settlement.collateral_liquidated, 0);
    assert_eq!(
        settlement.collateral_returned,
        position_before.collateral_deposited_atoms()
    );

    fixture.reload_market(&market).await;
    let (_, after) = borrow_position(&fixture, &market);
    let curator_after_settle = fixture.fetch_balance(&curator).await;
    assert_eq!(after.swept_collateral_atoms(), 0);
    assert_eq!(
        after.collateral_deposited_atoms(),
        position_before.collateral_deposited_atoms()
    );
    assert_eq!(after.borrowed_shares(), position_before.borrowed_shares());
    assert_eq!(curator_after_settle.supply, curator_after_begin.supply);
    assert_eq!(
        curator_after_begin.collateral - curator_after_settle.collateral,
        position_before.collateral_deposited_atoms()
    );
}

#[tokio::test]
async fn insolvency_after_sweep_closes_debt_and_leaves_collateral_with_curator() {
    let (mut fixture, market, position_key, _) = unhealthy_position().await;
    let curator = *fixture.curator_client().signer_pubkey();
    fixture
        .curator_client()
        .begin_capital_sweep(&market, &position_key)
        .await
        .unwrap();
    let curator_after_begin = fixture.fetch_balance(&curator).await;

    fixture.env().push_collateral_price(1.).await.unwrap();
    fixture.reload_market(&market).await;
    let events = fixture
        .curator_client()
        .settle_capital_sweep(&market, &position_key, None, None)
        .await
        .unwrap();
    let settlement = settled_event(&events);
    assert!(settlement.supply_repaid > 0);
    assert_eq!(settlement.collateral_returned, 0);

    fixture.reload_market(&market).await;
    let (_, after) = borrow_position(&fixture, &market);
    let curator_after_settle = fixture.fetch_balance(&curator).await;
    assert_eq!(after.swept_collateral_atoms(), 0);
    assert_eq!(after.collateral_deposited_atoms(), 0);
    assert_eq!(after.borrowed_shares().bits(), 0);
    assert_eq!(
        curator_after_begin.supply - curator_after_settle.supply,
        settlement.supply_repaid
    );
    assert_eq!(
        curator_after_settle.collateral,
        curator_after_begin.collateral
    );
}

#[tokio::test]
async fn only_curator_can_sweep_and_failed_settlement_rolls_back_state_and_tokens() {
    let (mut fixture, market, position_key, _) = unhealthy_position().await;
    assert_eq!(
        fixture
            .user_two_client()
            .begin_capital_sweep(&market, &position_key)
            .await
            .unwrap_err(),
        LendingAccountValidationError::InvalidMarketAuthority
    );

    fixture
        .curator_client()
        .begin_capital_sweep(&market, &position_key)
        .await
        .unwrap();
    fixture.reload_market(&market).await;
    let (_, pending_before) = borrow_position(&fixture, &market);
    let curator = *fixture.curator_client().signer_pubkey();
    let curator_before = fixture.fetch_balance(&curator).await;

    assert_eq!(
        fixture
            .user_two_client()
            .settle_capital_sweep(&market, &position_key, None, None)
            .await
            .unwrap_err(),
        LendingAccountValidationError::InvalidMarketAuthority
    );
    assert_eq!(
        fixture
            .curator_client()
            .settle_capital_sweep(&market, &position_key, Some(USDC(100.)), Some(0))
            .await
            .unwrap_err(),
        LendingError::CapitalSweepDidNotMeetRequirements
    );

    fixture.reload_market(&market).await;
    let (_, pending_after) = borrow_position(&fixture, &market);
    let curator_after = fixture.fetch_balance(&curator).await;
    assert_eq!(
        pending_after.swept_collateral_atoms(),
        pending_before.swept_collateral_atoms()
    );
    assert_eq!(
        pending_after.collateral_deposited_atoms(),
        pending_before.collateral_deposited_atoms()
    );
    assert_eq!(
        pending_after.borrowed_shares(),
        pending_before.borrowed_shares()
    );
    assert_eq!(curator_after, curator_before);
}
