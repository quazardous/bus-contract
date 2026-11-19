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
- The contract's items live here: `Timings`, `TimingStats`, `PointTimings`, `ListenerTimings`, `OverBudget`, `BudgetSink`, `OVER_BUDGET_FACT`, and the `CONTRACT`.

## Status: owns its contract since 0.2

Since 0.2, with `bus-contract` 0.5, the items above are defined here,
not in the core. The core names no capability: it keeps the typed
lookup (`Bus::capability`), the `Contract` and `Facet` types, and
`offers`. `BusTimings` reaches the capability through that lookup only.

Upgrading from 0.1 and `bus-contract` 0.4: import the items from
`bus_contract_timings` and add `use bus_contract_timings::BusTimings;` where
`bus.timings()` is called. Every rename is in
[MIGRATION-0.5.md](https://github.com/quazardous/bus-contract/blob/main/MIGRATION-0.5.md).

## Guarantees

What a bus offering this capability guarantees, and what it does not,
property by property with stable ids, is the
[`timings` section of GUARANTEES.md](https://github.com/quazardous/bus-contract/blob/main/GUARANTEES.md#timings). Each property names the
[bus-kernel](https://crates.io/crates/bus-kernel) conformance check
that proves it.

## Versions

Released on its own, with its own [CHANGELOG](CHANGELOG.md). Needs
`bus-contract` 0.5. See
[RELEASING.md](https://github.com/quazardous/bus-contract/blob/main/RELEASING.md)
for the repository's crates and their publish order.

## License

MIT or Apache-2.0, at your option.
