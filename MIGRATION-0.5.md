# Migrating to bus-contract 0.5

`bus-contract` 0.5.0 is the one breaking release planned before 1.0.
Two things change:

1. **The core stops naming the capabilities.** `Defer`, `Timings`,
   `Journal` and `Relay`, with their types, move to their own crates
   (`bus-contract-defer`, `-timings`, `-journal`, `-crossing`, each
   0.2.0). The core keeps the typed lookup (`Bus::capability`), the
   `Contract` and `Facet` types, and `offers`.
2. **Ids are checked.** `Point::ID` is a `PointId`, checked when made
   (in a `const`, at compile time); a bus refuses a bad listener id.

Releases that go together:

| Crate | Version |
|---|---|
| `bus-contract` | 0.5.0 |
| `bus-contract-defer`, `-timings`, `-journal`, `-crossing` | 0.2.0 |
| `bus-kernel` (the reference implementation) | 0.3.0 |

Writing one migration guide per breaking release follows
eventbus-contract's `MIGRATION-0.2.md`
(<https://github.com/mapseekai/eventbus-contract>).

## Cargo

```toml
[dependencies]
bus-contract = "0.5"
# only those you use:
bus-contract-defer = "0.2"
bus-contract-timings = "0.2"
bus-contract-journal = "0.2"
bus-contract-crossing = "0.2"   # brings bus-contract-journal
```

A library that uses only the core (points, listeners, alter, collect,
requests, local listeners) needs `bus-contract` only, and changes only
its point ids (below).

## Point ids

Before:

```rust,ignore
impl Point for DocClose {
    const ID: &'static str = "doc.close";
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}
```

After:

```rust,ignore
use bus_contract::PointId;

impl Point for DocClose {
    const ID: PointId = PointId::new("doc.close"); // a bad id does not compile
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}
```

The rule (`check_id`): at least two dot-separated segments, none
empty; the first, the crate's prefix, is `[a-z][a-z0-9_-]*`; the others
are `[A-Za-z0-9_-]+`. `close` (no prefix), `Doc.close`, `doc..close`
and `doc.*` are refused.

A `PointId` reads as a `&str` through `Deref`, so most uses keep
compiling. Where a `&str` value is needed, borrow it or ask for it:

```rust,ignore
// before                          // after
bus.listeners(P::ID)               bus.listeners(&P::ID)        // or P::ID.as_str()
let id: &'static str = P::ID;      let id: &'static str = P::ID.as_str();
format!("{}", P::ID)               format!("{}", P::ID)         // unchanged (Display)
entry.kind == P::ID                entry.kind == P::ID          // unchanged (String == PointId)
```

`PointDecl::id` is a `PointId`; `PointKey::id`, `PointInfo::id`,
`Failure::point` and the `Refused` fields stay `&'static str`.

## Listener ids

The typed calls still take any string, and the bus now checks it when
the listener registers: an id that breaks the rule is refused with
`Refused::BadId { id, error }` and nothing is registered.

```rust,ignore
// before: accepted                // after: refused, Refused::BadId
bus.listen::<DocClose>("autosave", opts, run)?;
// after: a prefixed id
bus.listen::<DocClose>("docs.autosave", opts, run)?;
```

`ListenerId` is optional: `const SAVE: ListenerId =
ListenerId::new("docs.autosave");` checks it at compile time,
`ListenerId::try_new(format!("docs.{name}"))` at run time, and it goes
where a `String` is taken (`bus.listen::<P>(SAVE, …)`). `after` /
`before` targets are not checked.

## Reaching a capability

The getters `Bus::defer()`, `Bus::timings()`, `Bus::journal()` and
`Bus::relay()` are gone. Each contract crate has an extension trait,
implemented for every bus (`dyn Bus`, `Arc<dyn Bus>`, a concrete one),
with the same method name. Add its `use` where the method is called:

| Call | Add |
|---|---|
| `bus.defer()` | `use bus_contract_defer::BusDefer;` |
| `bus.timings()` | `use bus_contract_timings::BusTimings;` |
| `bus.journal()` | `use bus_contract_journal::BusJournal;` (or `bus_contract_crossing::BusJournal`) |
| `bus.relay()` | `use bus_contract_crossing::BusCrossing;` |

Before:

```rust,ignore
use bus_contract::Bus;

let journal = bus.journal().expect("declared journaled");
```

After:

```rust,ignore
use bus_contract::Bus;
use bus_contract_journal::BusJournal;

let journal = bus.journal().expect("declared journaled");
```

The E0034 clash 0.4.5 warned about (`Bus::journal` and
`BusJournal::journal` both in scope) is gone with the getters.
`bus.get::<dyn Journal>()` and `capability::<dyn Journal, _>(&bus)`
still work.

## Moved items

Replace the `bus_contract::` path with the new crate:

| Items | From `bus_contract` to |
|---|---|
| `Defer` | `bus_contract_defer` |
| `Timings`, `TimingStats`, `PointTimings`, `ListenerTimings`, `OverBudget`, `BudgetSink`, `OVER_BUDGET_FACT` | `bus_contract_timings` |
| `Journal`, `JournalEntry`, `Since`, `Origin`, `kind_matches` | `bus_contract_journal` (also re-exported by `bus_contract_crossing`) |
| `Relay`, `Codec`, `Relayed`, `RelayedRequest`, `Answer`, `Authority`, `Crossing`, `Cursor`, `IngestError`, `Lost`, `Resumed`, `RelayedEntry`, `RelayedAsk`, `RelayedAnswer`, `EncodeFn`, `DecodeFn`, `EncodeValueFn`, `DecodeValueFn`, `far_listener`, `ask_run` | `bus_contract_crossing` |

```rust,ignore
// before
use bus_contract::{Bus, Codec, Journal, Origin, Relayed};
// after
use bus_contract::Bus;
use bus_contract_crossing::{Codec, Journal, Origin, Relayed};
```

The typed helpers moved with their traits (`dyn Defer::ask`,
`dyn Timings::on_over_budget`, `dyn Journal::publish` /
`publish_caused` / `wait_for`, `dyn Relay::authorize`): unchanged once
the trait is imported from its crate.

## Contract tables: needs and facets

`Capability` is gone. A capability contract is a `Contract` value its
crate exports as `CONTRACT`.

```rust,ignore
// before
PointDecl::of::<LibraryWrite>("A library write.").needs(&[Capability::Defer])
offers(&*bus, Capability::Defer)
// after
PointDecl::of::<LibraryWrite>("A library write.").needs(&[&bus_contract_defer::CONTRACT])
offers(&*bus, &bus_contract_defer::CONTRACT)
```

| Before | After |
|---|---|
| `Capability::Defer` | `&bus_contract_defer::CONTRACT` |
| `Capability::Timings` | `&bus_contract_timings::CONTRACT` |
| `Capability::Journal` | `&bus_contract_journal::CONTRACT` |
| `Capability::Relay` | `&bus_contract_crossing::CONTRACT` (requires the journal one; `offers` checks both) |
| `capability.as_str()` | `contract.name` (`"defer"`, `"timings"`, `"journal"`, `"crossing"`) |

What a declaration says for one contract is a `Facet`, made by the
contract's crate, `const`-friendly:

```rust,ignore
// before
const POINTS: &[PointDecl] = &[
    PointDecl::of::<LibraryImported>("Photos were imported.")
        .relayable(Codec::of::<LibraryImported>()),
    PointDecl::of::<FileDelete>("Files are deleted.")
        .relayable(Codec::request::<FileDelete>()),
    PointDecl::of::<StepComputed>("A render step was computed.").journaled(),
];

// after
use bus_contract_crossing::{relayable_event, relayable_request};
use bus_contract_journal::JOURNALED;

const POINTS: &[PointDecl] = &[
    PointDecl::of::<LibraryImported>("Photos were imported.")
        .facets(&[relayable_event::<LibraryImported>()]),
    PointDecl::of::<FileDelete>("Files are deleted.")
        .facets(&[relayable_request::<FileDelete>()]),
    PointDecl::of::<StepComputed>("A render step was computed.").facets(&[JOURNALED]),
];
```

- `relayable_event::<P>()` carries `Codec::of::<P>()`,
  `relayable_request::<P>()` carries `Codec::request::<P>()`, and
  `relayable(&CODEC)` takes any `'static` codec (`const CODEC: Codec =
  …;`).
- `facets` takes a `&'static [Facet]`. Inside a `const` table the
  slice literal is `'static`; in a function, put the slice in a `const`
  first (`const CROSSES: &[Facet] = &[relayable_event::<P>()];`).
- A relayable event no longer sets `journaled` in the table: the bus
  journals it when it declares it, and a relayable request is
  fail-closed there (`bus_contract_crossing::applied`). The crossing
  contract requires the journal one.
- `PointDecl::required()` returns the contracts (`Vec<&'static
  Contract>`): those of `needs`, then those of the facets.

The same at run time, on `PointSpec`:

| Before | After |
|---|---|
| `PointSpec::new(d).journaled()` | `PointSpec::new(d).facet(bus_contract_journal::JOURNALED)` |
| `PointSpec::new(d).relayable(Codec::of::<P>())` | `PointSpec::new(d).facet(relayable_event::<P>())` |
| `PointSpec::new(d).relayable_request(Codec::request::<P>())` | `PointSpec::new(d).facet(relayable_request::<P>())` |
| `spec.journaled` | `bus_contract_journal::is_journaled(&spec.facets)` |
| `spec.relay` (`Option<Codec>`) | `bus_contract_crossing::codec(&spec.facets)` (`Option<&'static Codec>`) |
| `info.journaled` (`PointInfo`) | `bus_contract_journal::is_journaled(&info.facets)` |
| `info.relayable` (`PointInfo`) | `bus_contract_crossing::codec(&info.facets).is_some()` |

## Refusals

| Before | After |
|---|---|
| `Refused::Unsupported { point, capability: Capability::Journal }` | `Refused::Unsupported { point, contract: "journal" }` |
| `Refused::NotEvent { point }` | `Refused::Contract { point, contract: "journal", reason }` (`bus_contract_journal::not_event`) |
| `Refused::NotJournaled { point }` | `Refused::Contract { point, contract: "journal", reason }` (`bus_contract_journal::not_journaled`) |
| `Refused::NotCrossable { point }` | `Refused::Contract { point, contract: "crossing", reason }` |
| a relayable point with another point's codec: `Refused::TypeMismatch` from `declare_all` | `Refused::Contract { contract: "crossing", .. }` from the bus |
| (new) | `Refused::BadId { id, error: IdError }` |

```rust,ignore
// before
assert_eq!(refused, [Refused::NotEvent { point: "doc.close" }]);
// after
assert!(matches!(
    refused.as_slice(),
    [Refused::Contract { point: "doc.close", contract: "journal", .. }]
));
```

`declare_all` now checks only that the bus offers the contracts a point
needs; what a facet's contract refuses is refused by the bus's
`declare_point`.

## Implementing `Bus`

1. Remove the overrides of `defer`, `timings`, `journal` and `relay`;
   implement `capability`, one cast per capability:

   ```rust,ignore
   use std::any::{Any, TypeId};
   use bus_contract::{Cast, Provided};
   use bus_contract_journal::Journal;

   fn as_journal(bus: &dyn Any) -> Option<&(dyn Journal + 'static)> {
       bus.downcast_ref::<MyBus>().map(|b| b as _)
   }
   static AS_JOURNAL: Cast<dyn Journal> = as_journal;

   // in `impl Bus for MyBus`:
   fn capability(&self, contract: TypeId) -> Option<Provided<'_>> {
       (contract == TypeId::of::<dyn Journal>())
           .then(|| Provided::new::<dyn Journal>(self, &AS_JOURNAL))
   }
   ```

2. In `declare_point`, check the id and the facets of the contracts you
   implement, then apply crossing:

   ```rust,ignore
   fn declare_point(&self, point: PointKey, spec: PointSpec) -> Result<(), Refused> {
       bus_contract::check_id(point.id).map_err(|error| Refused::BadId {
           id: point.id.to_owned(),
           error,
       })?;
       bus_contract_journal::check(&point, &spec)?;
       bus_contract_crossing::check(&point, &spec)?;
       let spec = bus_contract_crossing::applied(&point, spec);
       // …as before, reading `is_journaled(&spec.facets)` and `codec(&spec.facets)`
   }
   ```

3. In `listen_any` and `listen_local_any`, refuse a bad listener id
   with nothing changed: `check_id(&id).map_err(|error| Refused::BadId
   { id: id.clone(), error })?`.
4. Fill `PointInfo::facets` from the spec (it replaces `journaled` and
   `relayable`).
5. A publish on a point not journaled: `Err(bus_contract_journal::not_journaled(point.id))`.
6. `clone_value::<V>` and `fresh_value::<V>` are public now, if you made
   copies of them.

`bus-kernel` 0.3.0 does all of this, and its conformance suite checks
the id refusals (`core.id.point-checked`, `core.id.listener-checked` in
[GUARANTEES.md](https://github.com/quazardous/bus-contract/blob/main/GUARANTEES.md)).
