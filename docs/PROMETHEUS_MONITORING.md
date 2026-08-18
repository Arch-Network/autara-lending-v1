# Prometheus monitoring

`prometheus-alerts.yml` is a portable Prometheus rule group. It deliberately
contains no scrape configuration, target names, tenancy labels, URLs, or
Alertmanager routing. Load it through the monitoring system's existing rule
discovery mechanism after validating it in that environment.

## Metrics endpoints

The Autara server exposes `/metrics` on the address passed to
`autara-server --prometheus` (default `0.0.0.0:62777`). It exports the
server/indexer metrics in the default Prometheus registry:

- `autara_oracle_stale{market_address,side}`
- `autara_oracle_publish_time_age_seconds{market_address,side}`
- `autara_pusher_balance_lamports{pusher_pubkey}` when `PUSHER_PUBKEY` is set
- `autara_vault_reconciliation_delta_atoms{market_address,vault_type}`
- `autara_vault_reconciliation_success{market_address,vault_type}`
- `autara_market_liquidatable_positions{market_address}`

The dedicated oracle pusher (`ROLE=pusher`) exposes `/metrics` and `/health`
on Railway `PORT` (default `9090`). `/health` is `200` only after a successful
push within ~90s. Pusher-native metrics:

- `autara_pusher_pushes_succeeded_total`
- `autara_pusher_pushes_failed_total`
- `autara_pusher_fetch_failures_total`
- `autara_pusher_consecutive_failures`
- `autara_pusher_last_success_unixtime`
- `autara_pusher_signer_balance_lamports`

The liquidator exposes `/metrics` and `/health` on its configured
`--metrics-listen` address (default `127.0.0.1:9090`). Its alerts use:

- `autara_liquidator_consecutive_failures`
- `autara_liquidator_reload_failures_total`

The liquidator has no Prometheus readiness metric. Its `/health` endpoint
returns `200` only after a successful reload and `503` before the first reload
or after a reload failure. Configure your deployment's normal HTTP probe or
scrape-target health alert for this endpoint. Do not add an unscoped `up == 0`
rule: that would alert on unrelated targets in a shared Prometheus server.

## Setup checklist

1. Privately scrape the server and liquidator `/metrics` endpoints with the
   labels required by your monitoring platform. Confirm the metric families
   above are present before enabling alerts.
2. Load `prometheus-alerts.yml` using the platform's supported rule mechanism,
   then run its native rule validator (for example, `promtool check rules`).
3. Create deployment-scoped availability checks for the server `/metrics` and
   liquidator `/health`; route critical alerts to the launch DRI.
4. Send a test alert and record the rule revision, endpoint proof, receiver,
   release commit, and timestamp in launch evidence.

## Decisions still required

The rule file intentionally marks thresholds that code cannot determine:

- approved oracle `max_age` per market (the current launch document cites 60
  seconds);
- pusher warning balance in lamports for at least 24 hours of fee runway;
- permitted reconciliation dust in token atoms for each vault;
- severity/escalation policy for a nonzero liquidatable-position count; and
- the circuit-breaker response window, based on the production poll interval
  and `max_consecutive_failures`.

The vault reconciliation metrics are implemented: supply and collateral vault
balances are compared with protocol accounting on each indexer refresh. A value
of `autara_vault_reconciliation_success == 0` means collection or decoding
failed; the delta metric is meaningful only when collection succeeds.
