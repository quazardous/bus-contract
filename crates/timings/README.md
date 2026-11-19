# bus-contract-timings

The **timings** contract of [bus-contract](https://crates.io/crates/bus-contract):
how long listeners take, per point and per listener, and the passes over a point's budget.

`bus-contract` is a small core (points, order, alter and collect,
requests, local listeners, introspection) plus optional capabilities.
Each capability is its own contract, published as its own crate and
versioned on its own, so a change to one does not break the users of
the others.

## Use

```rust
use std::sync::Arc;
use bus_contract_timings::BusTimings;

/// Whether the bus offers the timings capability.
fn has_timings(bus: &Arc<dyn bus_contract::Bus>) -> bool {
    bus.timings().is_some()
}
```

- `BusTimings` gives `bus.timings()` on any bus: `dyn Bus`, `Arc<dyn Bus>` or a
  concrete one. It returns `None` when the bus does not offer the
  capability.
- The contract's items are re-exported here: `Timings`, `PointTimings`, `ListenerTimings`, `TimingStats`, `OverBudget`, `BudgetSink`, `OVER_BUDGET_FACT`.

## Status: a facade over bus-contract 0.4

In this release (0.1) the items above are the `bus-contract` 0.4
core's own items, re-exported: nothing changes for a bus or a library,
and code importing from here works with code importing from the core.
The point of this release is to let users switch their imports before
the break:

- **Now**: import the timings items from `bus_contract_timings` instead of
  `bus_contract`.
- **`bus-contract` 0.5**: the items move here, and the core loses its
  `timings()` getter. Code calling `bus.timings()` adds
  `use bus_contract_timings::BusTimings;`, and nothing else changes. This crate then
  moves to 0.2.

One catch while the core still has its getter: on an `Arc<dyn Bus>`,
in a file that imports both `bus_contract::Bus` and `BusTimings`,
`bus.timings()` names two methods and does not compile (E0034). Either
leave `BusTimings` out of that file until 0.5, or write
`BusTimings::timings(&bus)`. On a `&dyn Bus` or a generic `B: Bus` the call
compiles either way and both give the same answer.

## Guarantees

What a bus offering this capability guarantees, and what it does not,
property by property with stable ids, is the
[`timings` section of GUARANTEES.md](https://github.com/quazardous/bus-contract/blob/main/GUARANTEES.md#timings). Each property names the
[bus-kernel](https://crates.io/crates/bus-kernel) conformance check
that proves it.

## Versions

Released on its own, with its own [CHANGELOG](CHANGELOG.md). Needs
`bus-contract` 0.4.4 or later (the typed capability lookup). See
[RELEASING.md](https://github.com/quazardous/bus-contract/blob/main/RELEASING.md)
for the repository's crates and their publish order.

## License

MIT or Apache-2.0, at your option.
