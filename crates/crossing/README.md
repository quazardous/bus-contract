# bus-contract-crossing

The **crossing** contract of [bus-contract](https://crates.io/crates/bus-contract):
what may cross a process boundary and what it means when it does: the `Relay` capability, on top of the journal, with codecs, origins, authorities and asks answered across processes.

`bus-contract` is a small core (points, order, alter and collect,
requests, local listeners, introspection) plus optional capabilities.
Each capability is its own contract, published as its own crate and
versioned on its own, so a change to one does not break the users of
the others.

It depends on [`bus-contract-journal`](https://crates.io/crates/bus-contract-journal): a bus that offers `Relay` offers `Journal`, and this crate re-exports `BusJournal`, `Journal`, `JournalEntry` and `Since`.

## Use

```rust
use std::sync::Arc;
use bus_contract_crossing::BusCrossing;

/// Whether the bus can relay to another process.
fn has_crossing(bus: &Arc<dyn bus_contract::Bus>) -> bool {
    bus.relay().is_some()
}
```

- `BusCrossing` gives `bus.relay()` on any bus: `dyn Bus`, `Arc<dyn Bus>` or a
  concrete one. It returns `None` when the bus does not offer the
  capability.
- The contract's items are re-exported here: `Relay`, `Relayed`, `RelayedRequest`, `Codec`, `Cursor`, `RelayedEntry`, `Resumed`, `Lost`, `IngestError`, `Crossing`, `Authority`, `RelayedAsk`, `Answer`, `RelayedAnswer`, `Origin`, `EncodeFn`, `DecodeFn`, `EncodeValueFn`, `DecodeValueFn`, `ask_run`, `far_listener`.

## Status: a facade over bus-contract 0.4

In this release (0.1) the items above are the `bus-contract` 0.4
core's own items, re-exported: nothing changes for a bus or a library,
and code importing from here works with code importing from the core.
The point of this release is to let users switch their imports before
the break:

- **Now**: import the crossing items from `bus_contract_crossing` instead of
  `bus_contract`.
- **`bus-contract` 0.5**: the items move here, and the core loses its
  `relay()` getter. Code calling `bus.relay()` adds
  `use bus_contract_crossing::BusCrossing;`, and nothing else changes. This crate then
  moves to 0.2.

One catch while the core still has its getter: on an `Arc<dyn Bus>`,
in a file that imports both `bus_contract::Bus` and `BusCrossing`,
`bus.relay()` names two methods and does not compile (E0034). Either
leave `BusCrossing` out of that file until 0.5, or write
`BusCrossing::relay(&bus)`. On a `&dyn Bus` or a generic `B: Bus` the call
compiles either way and both give the same answer.

## Guarantees

What a bus offering this capability guarantees, and what it does not,
property by property with stable ids, is the
[`crossing` section of GUARANTEES.md](https://github.com/quazardous/bus-contract/blob/main/GUARANTEES.md#crossing). Each property names the
[bus-kernel](https://crates.io/crates/bus-kernel) conformance check
that proves it.

## Versions

Released on its own, with its own [CHANGELOG](CHANGELOG.md). Needs
`bus-contract` 0.4.4 or later (the typed capability lookup). See
[RELEASING.md](https://github.com/quazardous/bus-contract/blob/main/RELEASING.md)
for the repository's crates and their publish order.

## License

MIT or Apache-2.0, at your option.
