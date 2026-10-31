# Q-144 — Incremental execution primitives for market replay

**Status:** proposed; awaiting human plan approval. The Q project board is the status of record.
**Batch:** 42 — Market replay.
**Owner:** q_core. **Depends on:** Q-138 (Done).
**Plan:** [Implementation plan](../plans/Q-144-incremental-execution-primitives-plan.md).
**Consumer:** Q-145, the separate replay market and broker service in q_backend.

## Intent and architectural decision

Expose small, incremental execution primitives that a continuously running simulated broker can call for
one order and one observed market event. The simulator owns accounts, orders, transport and scheduling;
q_core owns numerical trigger, fill and fee rules. Neither embeds a second strategy evaluator.

Alternatives considered: Python-only matching duplicates shared semantics; running a whole candle or tick
backtest for every broker update imports batch lifecycle and terminal-force-close behavior. Shared native
primitives fit both execution and research and retain one owner for numerical rules. This is the proposed
approach, subject to review with Q-145.

Current evidence: `crates/q-engine/src/candle/protective.rs` has trade-based stop/limit rules (stop inclusive,
limit strictly through); `crates/q-engine/src/tick/simulate.rs` fills buys at ask and sells at bid. They are
different explicit models. Their historical results must not be silently unified or changed.

## Scope and interface

Add a public `q_engine::execution` module and Python `q_core.execution` bindings. Stateless functions use
integer price ticks and quantities in integer volume steps; validated instrument metadata supplies tick size,
volume step, point value and currency precision at the Python adapter boundary. Reject nonfinite prices,
invalid side/kind, negative sizes/delays/costs, crossed/nonpositive quotes and conversion overflow. No float
epsilon determines a touch. Conversion uses decimal arithmetic and rejects off-grid order levels.

The public Python API is versioned by the normal q_core date release and advertises
`q_core.execution.INCREMENTAL_EXECUTION = 1`:

- `match_quote(*, side: str, kind: str, level_ticks: int | None, bid_ticks: int,
  ask_ticks: int, slippage_ticks: int = 0) -> int | None`.
  Side is `buy` or `sell`; kind is `market`, `stop`, or `limit`. Market needs no level; others require one.
  Market buys pay ask plus adverse slippage, sells receive bid minus adverse slippage. A buy stop triggers
  when ask >= level; sell stop when bid <= level, and fills as market. A buy limit is executable when ask <=
  level, sell limit when bid >= level; fills at the current executable quote, with price improvement, never
  worse than its limit. Slippage is applied only to market/stop fills. A triggered stop is converted by the
  simulator to a market order; it must not become untriggered while awaiting execution latency.
- `match_trade(*, side: str, kind: str, level_ticks: int, trade_ticks: int) -> int | None`:
  the existing candle resting-order semantics, inclusive stops at the observed trade and strict-through
  limits at their level. This makes the model distinction explicit, rather than promising quote/trade parity.
- `fill_cost(*, quantity_steps: int, volume_step: str, price_ticks: int, tick_size: str,
  point_value: str, cost_per_contract: str, cost_bps: str, currency_decimals: int) -> str`:
  decimal currency result, half-even rounding once at the final currency precision. Fee = quantity ×
  cost_per_contract + abs(quantity × price × point_value) × cost_bps / 10000.

Rust exposes equivalent typed functions with `Result` validation. Python raises `ValueError` for invalid
inputs. Python arbitrary integers outside the native representation raise a documented overflow exception.
`match_quote` is Q-145's default model; `match_trade` preserves the research model. Existing candle resting
orders delegate equivalent comparisons to shared helpers. The tick engine delegates equivalent quote market
pricing and trigger helpers; its current target and stop outcomes remain unchanged. Reuse the existing fee
helpers where their units agree; do not change research fee accounting or rounding to force this API on it.

Stop-loss is a stop order on the closing side; take-profit is a limit on the closing side. Event priority,
expiry, activation, quote freshness, liquidity assumptions, account netting, OCO and order identity are
orchestration responsibilities of Q-145. No partial fills, order book, queue simulation or strategy code.

## Acceptance and focused checks

1. Native and Python vectors cover both sides, market/stop/limit, touch, through, gap, improvement, adverse
   slippage, fees, off-grid/overflow rejection and invalid quotes. The quote/trade distinction has explicit
   examples: ask=100 and buy limit=100 fills under quote policy; trade=100 does not under trade policy.
2. All existing q-engine candle/tick regression and golden tests for touched numerical paths retain their
   results. Sharing helpers must not change the legacy price model or terminal-close behavior.
3. Repeated calls with the same arguments give identical results and retain no mutable state. Rust/Python
   results agree on the fixture vectors, including large valid prices and minimum size.
4. Documentation records units, rounding, exceptions and model differences; bindings advertise the capability.
5. Targeted q-engine tests, Python binding tests and formatting/lint of affected crates pass. No Docker, Wine,
   MT5, GPU, UI build or benchmark required. Full release CI runs through the existing release workflow.

## Delivery

Implement only after human approval and `./work start Q-144 --agent <agent> --worktree`. Put Cargo targets on
`/mnt/build` before compiling. Commit on the task branch; mark In Review using `./work board set` with evidence.
The human finishes Q-144 through `./work finish`, producing the pushed date tag. Q-145 must pin that release;
it cannot substitute a path dependency or unpushed revision. No contracts change is needed for this native API.
