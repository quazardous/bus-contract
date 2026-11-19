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
- The contract's items live here: `Relay`, `Codec`, `Relayed`, `RelayedRequest`, `Origin`, `Answer`, `Authority`, `Crossing`, `Cursor`, `IngestError`, `Lost`, `Resumed`, the relayed messages, `far_listener`, `ask_run`, the `CONTRACT` (which requires the journal one), the `relayable(&CODEC)`, `relayable_event::<P>()` and `relayable_request::<P>()` facets with `codec`, and `check` / `applied` for implementations. The journal items it sits on are re-exported.

## Status: owns its contract since 0.2

Since 0.2, with `bus-contract` 0.5, the items above are defined here,
not in the core. The core names no capability: it keeps the typed
lookup (`Bus::capability`), the `Contract` and `Facet` types, and
`offers`. `BusCrossing` reaches the capability through that lookup only.

Upgrading from 0.1 and `bus-contract` 0.4: import the items from
`bus_contract_crossing` and add `use bus_contract_crossing::BusCrossing;` where
`bus.relay()` is called. Every rename is in
[MIGRATION-0.5.md](https://github.com/quazardous/bus-contract/blob/main/MIGRATION-0.5.md).

## Guarantees

What a bus offering this capability guarantees, and what it does not,
property by property with stable ids, is the
[`crossing` section of GUARANTEES.md](https://github.com/quazardous/bus-contract/blob/main/GUARANTEES.md#crossing). Each property names the
[bus-kernel](https://crates.io/crates/bus-kernel) conformance check
that proves it.

## Versions

Released on its own, with its own [CHANGELOG](CHANGELOG.md). Needs
`bus-contract` 0.5. See
[RELEASING.md](https://github.com/quazardous/bus-contract/blob/main/RELEASING.md)
for the repository's crates and their publish order.

## License

MIT or Apache-2.0, at your option.
