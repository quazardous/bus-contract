# Changelog

## 0.2.0

Breaking, with `bus-contract` 0.5.0: this crate owns its contract.
See [MIGRATION-0.5.md](https://github.com/quazardous/bus-contract/blob/main/MIGRATION-0.5.md).

- The contract's items are defined here, no longer re-exported from the
  core: `Timings` (and `on_over_budget`), `TimingStats`, `PointTimings`, `ListenerTimings`, `OverBudget`, `BudgetSink`, `OVER_BUDGET_FACT`.
- `CONTRACT`, the contract as data (`bus_contract::Contract`): what a
  point names in `PointDecl::needs` and what `offers` checks. It
  replaces `Capability`.
- `BusTimings` answers through the typed lookup `Bus::capability` only:
  the core's getter is gone, and so is the E0034 name clash.

## 0.1.0

First release: the public name of the timings contract, before the core
moves it here.

- Re-exports the `bus-contract` 0.4 core's timings items (`Timings`, `PointTimings`, `ListenerTimings`, `TimingStats`, `OverBudget`, `BudgetSink`, `OVER_BUDGET_FACT`).
  They are the core's own types, so code using either path works
  together.
- `BusTimings`, an extension trait implemented for every `Bus` (sized or
  not), gives `bus.timings()`. It answers through the core's getter, which
  defaults to the typed lookup `Bus::capability`.
