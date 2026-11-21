# Capabilities

The `Bus` trait is the core every implementation offers. An
implementation may offer more, each a contract of its own, defined in a
crate of its own and found at run time through the typed lookup
`Bus::capability`, which returns `None` by default. One `dyn Bus` type
works everywhere, whatever the implementation offers.

The four contracts, their crates, traits, getters and facets are in the
[README's table](../README.md#what-is-in-the-workspace). Each crate
exports its contract as `CONTRACT` (`bus_contract_defer::CONTRACT`,
`bus_contract_timings::CONTRACT`, `bus_contract_journal::CONTRACT`,
`bus_contract_crossing::CONTRACT`, which requires the journal's), and
its facets: `JOURNALED` for the journal, `relayable(&CODEC)`,
`relayable_event::<P>()` and `relayable_request::<P>()` for crossing
(see [crossing.md](crossing.md)). Each has its page:
[defer.md](defer.md), [timings.md](timings.md),
[journal-and-relay.md](journal-and-relay.md).

## Needing one, or doing without

- A library that needs a capability names its contract in its table
  (`PointDecl::needs(&[&bus_contract_defer::CONTRACT])`), or gives the
  point a facet of it (`PointDecl::facets(&[JOURNALED])`): what the
  declaration says for that contract, data only that contract's crate
  reads. `declare_all` refuses such a point on a bus without the
  contract (`Refused::Unsupported`) and leaves it undeclared. If you
  mark a point `JOURNALED`, the bus must provide `Journal`, or
  `declare_all` refuses the point.
- A library that can do without one calls the extension trait
  (`use bus_contract_defer::BusDefer;` then `bus.defer()`) and falls
  back when it gets `None`.
- `offers(bus, &bus_contract_defer::CONTRACT)` tells whether a bus has
  one, and the contracts it requires.

## Offering one

The lookup is keyed by the trait's `TypeId`: `Bus::capability(TypeId)`
returns the bus as `Any` with a cast to the capability's trait
(`Provided`), and `capability::<dyn Journal, _>(&bus)` or
`bus.get::<dyn Journal>()` on a `dyn Bus` gives `Option<&dyn Journal>`.
An implementation writes one method, `capability`, with one small cast
per capability:

```rust,ignore
fn as_journal(bus: &dyn Any) -> Option<&(dyn Journal + 'static)> {
    bus.downcast_ref::<MyBus>().map(|b| b as _)
}
static AS_JOURNAL: Cast<dyn Journal> = as_journal;

impl Bus for MyBus {
    // …the core methods…
    fn capability(&self, contract: TypeId) -> Option<Provided<'_>> {
        (contract == TypeId::of::<dyn Journal>())
            .then(|| Provided::new::<dyn Journal>(self, &AS_JOURNAL))
    }
}
```

Two semver-incompatible versions of a contract crate have different
trait types: a library on one, over a bus implementing the other, is
refused by `declare_all` with the contract's name instead of failing to
build. If your library and your bus pull two major versions of a
contract crate, `declare_all` names the contract it refused: align the
versions.
