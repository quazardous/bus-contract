# Changelog

## 0.1.0

First release: the public name of the defer contract, before the core
moves it here.

- Re-exports the `bus-contract` 0.4 core's defer items (`Defer`, `PendingRun`).
  They are the core's own types, so code using either path works
  together.
- `BusDefer`, an extension trait implemented for every `Bus` (sized or
  not), gives `bus.defer()`. It answers through the core's getter, which
  defaults to the typed lookup `Bus::capability`.
