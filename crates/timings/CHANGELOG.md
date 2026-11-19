# Changelog

## 0.1.0

First release: the public name of the timings contract, before the core
moves it here.

- Re-exports the `bus-contract` 0.4 core's timings items (`Timings`, `PointTimings`, `ListenerTimings`, `TimingStats`, `OverBudget`, `BudgetSink`, `OVER_BUDGET_FACT`).
  They are the core's own types, so code using either path works
  together.
- `BusTimings`, an extension trait implemented for every `Bus` (sized or
  not), gives `bus.timings()`. It answers through the core's getter, which
  defaults to the typed lookup `Bus::capability`.
