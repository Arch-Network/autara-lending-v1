#!/usr/bin/env node
// Rigorous end-to-end liquidator test suite (testnet4).
//
// Creates a battery of faulty loans across markets/directions, runs the REAL
// autara-liquidator binary against them, then asserts each outcome on-chain and
// re-runs to check idempotency. Exits non-zero if any scenario fails.
//
//   node scripts/liq-test-suite.mjs
//
// Prereqs: target/debug/{autara-cli,over-borrow,autara-liquidator} built;
// liquidator-config.local.json present; mint-authority keyfiles in keys/.

import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import crypto from 'node:crypto';

const RPC = 'https://rpc.testnet.arch.network';
const ROOT = '/Users/deepanshuhooda/work/autara-lending-v1';
const CLI = `${ROOT}/target/debug/autara-cli`;
const OVERBORROW = `${ROOT}/target/debug/over-borrow`;
const BOT = `${ROOT}/target/debug/autara-liquidator`;
const NET = ['--network', 'testnet'];

const TBTC = '726179cf49b6dc407c1438cec98815d92277b625b09de81818f5f3a57989f1f1';
const TUSDC = 'a2ff4e218e9ddda64c35ee926c00a7715ec7116065b04d8c537f6030c87e49e5';
const BTC_FEED = 'e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43';
const USDC_FEED = 'eaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a';
const BTC_FEED_ACCT = 'c8110dc356bd4583f5f3eb30124933151e2ceb6e57721edd892d2b397f86b3ad';
const USDC_FEED_ACCT = 'df179c8ce964fe42f59a9f9bcc6c8a73f5ac98e78dc3adc6c0b79ec1ae1fe33d';

const KEY = (n) => `${ROOT}/keys/${n}`;
const TUSDC_AUTH = KEY('liqtest-tusdc-authority.key');
const TBTC_AUTH = KEY('liqtest-tbtc-authority.key');
const SUPPLIER = KEY('autara-cli-signer.key');               // funded; supplies liquidity
const BORROWER = KEY('liqtest-suite-borrower.key');          // regenerated fresh each run
const SUPPLIER_PK = pubkeyOf(SUPPLIER);

// Markets: [hex, unhealthyLtv, dir]  (dir: 'primary' = tBTC collat / borrow tUSDC; 'reverse' = tUSDC collat / borrow tBTC)
const M = {
  m80:   { hex: '30272668a9327f79a559343879c40d802249a3494153cae6660b273121ad54b3', unhealthy: 0.85, dir: 'primary' },
  m86:   { hex: 'e5420175199b803a1dd85df0fc1722095d44241da679cbd58a47d2759ec2b24d', unhealthy: 0.90, dir: 'primary' },
  m80b:  { hex: '397de52683ef24ec6284886693b07510b472faf6963278f1d9e1e878fea66522', unhealthy: 0.90, dir: 'primary' },
  rev80: { hex: 'fb6bb2d6de9c23053c655949653b3e2786dc769901c376b55ef75b904ee88f7e', unhealthy: 0.85, dir: 'reverse' },
  rev86: { hex: 'f829fef6498ce2e76cfcc15736c24bb1615233d2ad6d8c606be9fc20a834ed6e', unhealthy: 0.90, dir: 'reverse' },
};

// ---------- helpers ----------
function pubkeyOf(keyfile) {
  const sec = fs.readFileSync(keyfile, 'utf8').trim();
  const e = crypto.createECDH('secp256k1');
  e.setPrivateKey(Buffer.from(sec, 'hex'));
  return e.getPublicKey(null, 'uncompressed').slice(1, 33).toString('hex');
}
function sh(bin, args, { quiet = true } = {}) {
  try {
    const out = execFileSync(bin, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
    if (!quiet) process.stdout.write(out);
    return out;
  } catch (e) {
    return (e.stdout || '') + (e.stderr || '');
  }
}
async function rpc(method, params) {
  const r = await fetch(RPC, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) });
  return r.json();
}
async function readAcct(hex) { const j = await rpc('read_account_info', Array.from(Buffer.from(hex, 'hex'))); return j.error ? null : j.result; }
async function lamports(hex) { const a = await readAcct(hex); return a ? a.lamports : 0; }
async function price(acct) { const a = await readAcct(acct); return Number(Buffer.from(a.data).readBigUInt64LE(32)) * 1e-8; }
const sleep = (s) => new Promise((r) => setTimeout(r, s * 1000));
function log(...a) { console.log(...a); }

// Parse `autara-cli read positions --authority <pk>` into { marketHex: {collateral, ltv} }
function readPositions(authPk) {
  const out = sh(CLI, [...NET, 'read', 'positions', '--authority', authPk]);
  const res = {};
  const re = /Market:\s*([0-9a-f]{64})[\s\S]*?Collateral:\s*(\d+)\s*atoms[\s\S]*?LTV:\s*([0-9.]+)/g;
  let m;
  while ((m = re.exec(out)) !== null) res[m[1]] = { collateral: Number(m[2]), ltv: Number(m[3]) };
  return res;
}
function mint(tokenHex, authKey, toPk, atoms) {
  const o = sh(CLI, [...NET, '--signer', authKey, 'token', 'mint', '--token', tokenHex, '--to', toPk, '--amount', String(atoms)]);
  if (!/Mint successful/.test(o)) throw new Error(`mint failed (${tokenHex.slice(0,8)} -> ${toPk.slice(0,8)} ${atoms}):\n${o.slice(-300)}`);
}
function supply(marketHex, atoms) {
  const o = sh(CLI, [...NET, '--signer', SUPPLIER, 'tx', 'supply', '--market', marketHex, '--amount', String(atoms)]);
  // supply prints an events dump on success; treat presence of "Error"/"error" as failure
  if (/Error|panic|failed to/.test(o) && !/OracleRate/.test(o)) throw new Error(`supply failed (${marketHex.slice(0,8)} ${atoms}):\n${o.slice(-300)}`);
}
function overBorrow({ marketHex, depositAtoms, borrowAtoms, inflate, collFeed }) {
  const o = sh(OVERBORROW, ['--signer', BORROWER, ...NET, '--market', marketHex,
    '--deposit-atoms', String(depositAtoms), '--borrow-atoms', String(borrowAtoms),
    '--inflated-btc-price', String(inflate), '--btc-feed', collFeed]);
  if (!/STATUS: Processed/.test(o)) throw new Error(`over-borrow failed (${marketHex.slice(0,8)}):\n${o.slice(-500)}`);
}
function runBot(seconds, logPath) {
  return new Promise((resolve) => {
    const out = fs.openSync(logPath, 'w');
    const p = spawn(BOT, ['--config', 'liquidator-config.local.json'], { cwd: ROOT, stdio: ['ignore', out, out], env: { ...process.env, RUST_LOG: 'info' } });
    const t = setTimeout(() => { try { p.kill('SIGKILL'); } catch {} }, seconds * 1000);
    p.on('exit', () => { clearTimeout(t); try { fs.closeSync(out); } catch {}; resolve(); });
  });
}

// ---------- main ----------
const results = [];
function record(name, pass, detail) { results.push({ name, pass, detail }); log(`  [${pass ? 'PASS' : 'FAIL'}] ${name} — ${detail}`); }

(async () => {
  log('=== Liquidator test suite ===');
  // Fresh borrower each run => clean position state (re-runnable).
  fs.writeFileSync(BORROWER, crypto.randomBytes(32).toString('hex'));
  const BORROWER_PK = pubkeyOf(BORROWER);
  log('borrower', BORROWER_PK.slice(0, 16), '(fresh) | supplier', SUPPLIER_PK.slice(0, 16));

  // ---- 0. funds & gas ----
  log('\n[setup] faucet borrower gas + mint tokens + seed liquidity');
  {
    const pkBytes = Array.from(Buffer.from(BORROWER_PK, 'hex'));
    let bal = await lamports(BORROWER_PK);
    for (let i = 0; i < 6 && bal < 3_000_000; i++) {
      await rpc('create_account_with_faucet', pkBytes); // funds a brand-new account
      await rpc('request_airdrop', pkBytes);            // tops up an existing one
      await sleep(2);
      const nb = await lamports(BORROWER_PK);
      if (nb <= bal) break; // faucet can't increase it further; proceed with what we have
      bal = nb;
    }
    if (bal < 200_000) throw new Error('borrower underfunded for gas: ' + bal);
    log('[setup] borrower gas:', bal);
  }
  // borrower collateral: tBTC for primary deposits, tUSDC for reverse deposits
  mint(TBTC, TBTC_AUTH, BORROWER_PK, 50_000_000);       // 0.5 tBTC
  mint(TUSDC, TUSDC_AUTH, BORROWER_PK, 6_000_000_000);  // 6,000 tUSDC
  // supplier liquidity
  mint(TUSDC, TUSDC_AUTH, SUPPLIER_PK, 60_000_000_000); // 60,000 tUSDC
  mint(TBTC, TBTC_AUTH, SUPPLIER_PK, 150_000_000);      // 1.5 tBTC
  supply(M.m80b.hex, 15_000_000_000); // 397de526 likely empty
  supply(M.m80.hex, 6_000_000_000);
  supply(M.m86.hex, 6_000_000_000);
  supply(M.rev80.hex, 50_000_000);    // 0.5 tBTC into reverse markets
  supply(M.rev86.hex, 50_000_000);

  const btc = await price(BTC_FEED_ACCT), usdc = await price(USDC_FEED_ACCT);
  log(`[setup] prices BTC=$${btc.toFixed(2)} USDC=$${usdc.toFixed(4)}`);

  // borrowAtoms for a target ltv. primary: collat tBTC(8) / supply tUSDC(6). reverse: collat tUSDC(6) / supply tBTC(8).
  const borrowFor = (dir, depositAtoms, targetLtv) => {
    if (dir === 'primary') { const collVal = (depositAtoms / 1e8) * btc; return Math.round((targetLtv * collVal / usdc) * 1e6); }
    else { const collVal = (depositAtoms / 1e6) * usdc; return Math.round((targetLtv * collVal / btc) * 1e8); }
  };

  // ---- 1. scenario matrix ----
  const DEP_BTC = 10_000_000;     // 0.1 tBTC (primary collateral)
  const DEP_USDC = 2_000_000_000; // 2,000 tUSDC (reverse collateral)
  const scenarios = [
    { key: 'S1 full   (tBTC→tUSDC, unhlt .85)', m: M.m80,   dep: DEP_BTC,  ltv: 1.15, inflate: 200000, feed: BTC_FEED,  expect: 'full' },
    { key: 'S2 partial(tBTC→tUSDC, unhlt .90)', m: M.m86,   dep: DEP_BTC,  ltv: 0.95, inflate: 200000, feed: BTC_FEED,  expect: 'partial' },
    { key: 'S3 healthy(tBTC→tUSDC, unhlt .90)', m: M.m80b,  dep: DEP_BTC,  ltv: 0.60, inflate: Math.round(btc), feed: BTC_FEED,  expect: 'untouched' },
    { key: 'S4 full   (tUSDC→tBTC rev, unhlt .85)', m: M.rev80, dep: DEP_USDC, ltv: 1.40, inflate: 5, feed: USDC_FEED, expect: 'full' },
    { key: 'S5 healthy(tUSDC→tBTC rev, unhlt .90)', m: M.rev86, dep: DEP_USDC, ltv: 0.60, inflate: Math.max(2, Math.round(usdc)), feed: USDC_FEED, expect: 'untouched' },
  ];

  log('\n[setup] creating positions');
  for (const s of scenarios) {
    const borrowAtoms = borrowFor(s.m.dir, s.dep, s.ltv);
    overBorrow({ marketHex: s.m.hex, depositAtoms: s.dep, borrowAtoms, inflate: s.inflate, collFeed: s.feed });
    log(`  created ${s.key}  deposit=${s.dep} borrow=${borrowAtoms}`);
  }
  await sleep(6); // let feeder restore real prices

  // ---- 2. snapshot pre-state ----
  const pre = readPositions(BORROWER_PK);
  log('\n[pre] positions:');
  for (const s of scenarios) { const p = pre[s.m.hex] || {}; log(`  ${s.key}: collateral=${p.collateral} ltv=${p.ltv}`); }

  // sanity: liquidatable scenarios must be unhealthy pre-run; healthy ones must be < unhealthy
  for (const s of scenarios) {
    const p = pre[s.m.hex];
    if (!p) { record(`PRECHECK ${s.key}`, false, 'no position found after setup'); continue; }
    if (s.expect === 'untouched' && p.ltv >= s.m.unhealthy) record(`PRECHECK ${s.key}`, false, `should be healthy but ltv ${p.ltv} >= unhealthy ${s.m.unhealthy}`);
    if (s.expect !== 'untouched' && p.ltv < s.m.unhealthy) record(`PRECHECK ${s.key}`, false, `should be unhealthy but ltv ${p.ltv} < unhealthy ${s.m.unhealthy}`);
  }

  // ---- 3. run the bot ----
  log('\n[run] launching liquidator (90s)...');
  await runBot(90, '/tmp/liq-suite-run1.log');
  const log1 = fs.readFileSync('/tmp/liq-suite-run1.log', 'utf8');

  // ---- 4. verify ----
  log('\n[verify] post-liquidation state:');
  const post = readPositions(BORROWER_PK);
  for (const s of scenarios) {
    const a = pre[s.m.hex], b = post[s.m.hex] || { collateral: 0, ltv: 0 };
    if (s.expect === 'full') {
      record(s.key, b.collateral === 0 && b.ltv === 0, `collateral ${a.collateral}→${b.collateral}, ltv ${a.ltv}→${b.ltv} (expect 0/0)`);
    } else if (s.expect === 'partial') {
      const ok = b.collateral > 0 && b.collateral < a.collateral && b.ltv > 0 && b.ltv < a.ltv && b.ltv < s.m.unhealthy + 0.001;
      record(s.key, ok, `collateral ${a.collateral}→${b.collateral}, ltv ${a.ltv}→${b.ltv} (expect partial: 0<coll<${a.collateral}, ltv↓ to <${s.m.unhealthy})`);
    } else { // untouched
      record(s.key, b.collateral === a.collateral && Math.abs(b.ltv - a.ltv) < 0.02, `collateral ${a.collateral}→${b.collateral}, ltv ${a.ltv}→${b.ltv} (expect unchanged)`);
    }
  }
  // no hard errors on our markets
  const fails = (log1.match(/Liquidation FAILED/g) || []).length;
  record('no Liquidation FAILED in bot log', fails === 0, `${fails} failure log line(s)`);

  // PropAMM-path certification (config has propamm enabled; PropAMM price >> CLAMM pool, so it should win).
  const routedPropamm = (log1.match(/-> PropAMM/g) || []).length;
  record('routed liquidation(s) via PropAMM', routedPropamm > 0, `${routedPropamm} PropAMM route decision(s)`);
  const propammFailed = (log1.match(/PropAMM swap FAILED/g) || []).length;
  record('no PropAMM swap FAILED', propammFailed === 0, `${propammFailed} PropAMM swap failure(s)`);
  const propammOk = (log1.match(/PropAMM swap SUCCESS/g) || []).length;
  record('PropAMM swap(s) executed', propammOk > 0, `${propammOk} PropAMM swap success(es)`);

  // ---- 5. idempotency re-run ----
  log('\n[idempotency] re-running bot (40s); nothing should change...');
  const before2 = readPositions(BORROWER_PK);
  await runBot(40, '/tmp/liq-suite-run2.log');
  const after2 = readPositions(BORROWER_PK);
  let stable = true, detail = '';
  for (const s of scenarios) {
    const x = before2[s.m.hex] || { collateral: 0, ltv: 0 }, y = after2[s.m.hex] || { collateral: 0, ltv: 0 };
    if (x.collateral !== y.collateral) { stable = false; detail += `${s.key} coll ${x.collateral}->${y.collateral}; `; }
  }
  record('idempotent re-run (no further liquidation)', stable, stable ? 'all positions unchanged on 2nd pass' : detail);

  // ---- report ----
  const passed = results.filter((r) => r.pass).length, total = results.length;
  log(`\n=== RESULT: ${passed}/${total} checks passed ===`);
  if (passed !== total) { log('FAILURES:'); results.filter((r) => !r.pass).forEach((r) => log(`  - ${r.name}: ${r.detail}`)); process.exit(1); }
  log('ALL CHECKS PASSED ✅');
})().catch((e) => { console.error('SUITE ERROR:', e.message); process.exit(2); });
