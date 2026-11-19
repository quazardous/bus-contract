# bus-contract-journal

The **journal** contract of [bus-contract](https://crates.io/crates/bus-contract):
events of journaled points kept with a number, read again from one, delivered after the publisher has moved on.

`bus-contract` is a small core (points, order, alter and collect,
requests, local listeners, introspection) plus optional capabilities.
Each capability is its own contract, published as its own crate and
versioned on its own, so a change to one does not break the users of
the others.

## Use

```rust
use std::sync::Arc;
use bus_contract_journal::BusJournal;

/// Whether the bus offers the journal capability.
fn has_journal(bus: &Arc<dyn bus_contract::Bus>) -> bool {
    bus.journal().is_some()
}
```

- `BusJournal` gives `bus.journal()` on any bus: `dyn Bus`, `Arc<dyn Bus>` or a
  concrete one. It returns `None` when the bus does not offer the
  capability.
- The contract's items live here: `Journal`, `JournalEntry`, `Since`, `Origin`, `kind_matches`, the `CONTRACT`, the `JOURNALED` facet with `is_journaled`, and `check` / `not_journaled` for implementations.

## Status: owns its contract since 0.2

Since 0.2, with `bus-contract` 0.5, the items above are defined here,
not in the core. The core names no capability: it keeps the typed
lookup (`Bus::capability`), the `Contract` and `Facet` types, and
`offers`. `BusJournal` reaches the capability through that lookup only.

Upgrading from 0.1 and `bus-contract` 0.4: import the items from
`bus_contract_journal` and add `use bus_contract_journal::BusJournal;` where
`bus.journal()` is called. Every rename is in
[MIGRATION-0.5.md](https://github.com/quazardous/bus-contract/blob/main/MIGRATION-0.5.md).

## Guarantees

What a bus offering this capability guarantees, and what it does not,
property by property with stable ids, is the
[`journal` section of GUARANTEES.md](https://github.com/quazardous/bus-contract/blob/main/GUARANTEES.md#journal). Each property names the
[bus-kernel](https://crates.io/crates/bus-kernel) conformance check
that proves it.

## Versions

Released on its own, with its own [CHANGELOG](CHANGELOG.md). Needs
`bus-contract` 0.5. See
[RELEASING.md](https://github.com/quazardous/bus-contract/blob/main/RELEASING.md)
for the repository's crates and their publish order.

## License

MIT or Apache-2.0, at your option.
