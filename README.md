# bus-contract

[<img alt="github" src="https://img.shields.io/badge/github-quazardous/bus--contract-8da0cb?logo=github" height="20">](https://github.com/quazardous/bus-contract)
[![Latest version](https://img.shields.io/crates/v/bus-contract.svg)](https://crates.io/crates/bus-contract)
[![Documentation](https://docs.rs/bus-contract/badge.svg)](https://docs.rs/bus-contract)
[![Build Status](https://github.com/quazardous/bus-contract/actions/workflows/ci.yml/badge.svg)](https://github.com/quazardous/bus-contract/actions/workflows/ci.yml)
[![MSRV 1.95](https://img.shields.io/badge/MSRV-1.95-orange.svg)](https://github.com/quazardous/bus-contract/blob/main/Cargo.toml)
[![no unsafe](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance/)
[![no dependencies](https://img.shields.io/badge/dependencies-none-success.svg)](https://github.com/quazardous/bus-contract/blob/main/Cargo.toml)
[![MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/quazardous/bus-contract/blob/main/LICENSE-MIT)
[![Apache](https://img.shields.io/badge/license-Apache-blue.svg)](https://github.com/quazardous/bus-contract/blob/main/LICENSE-APACHE)

Traits and types for an in-process bus of hook points (requests,
events, alter, collect) with ordered listeners. A library declares its
points and listeners against the `Bus` trait; the application creates
the bus and passes it to each library.

## Guarantees in five lines

- **Ordered listeners:** a point's listeners run in their declared
  order (`after`, `before`, `last`, then priority).
- **Failure isolation:** a failing listener (error, panic, missed
  deadline) is isolated: its edit is dropped, the others run, and the
  failure is returned to the caller.
- **Veto without rollback:** a refusal stops a request, but **never
  rolls back** what the listeners before it did.
- **Fail-open or fail-closed:** a listener failure on a request is
  skipped (fail-open), unless the point is declared `fail_closed`
  (relayable requests always are): then it refuses the request.
- **No global:** the application owns the bus and hands it to each
  library; a library never constructs one.

Each property, with a stable id (`core.order.declared`,
`core.request.veto-stops`, …), what is not guaranteed (crossing is
**not a distributed transaction**, authorities are not a security
boundary, the journal is a window in memory) and the `bus-kernel`
conformance check that proves it, is in
[GUARANTEES.md](https://github.com/quazardous/bus-contract/blob/main/GUARANTEES.md),
also the `guarantees` page of the crate docs.

## Getting started

This crate has no implementation: add
[`bus-kernel`](https://github.com/quazardous/bus-kernel) (the
reference implementation, `Kernel`) to get started, or implement `Bus`
yourself (`bus-kernel`'s `conformance`
feature checks any implementation against the guarantees).

A point has one of four kinds. `alter` transforms a shared value: each
listener edits what the previous ones left, and the caller gets it
back. `collect` gathers every listener's answer, each in a fresh value,
and the failures. A `request` is an alter a listener may refuse; an
`event` says something happened. `PointKind` is not only a label:
`Request` enables refusals, `Event` gates the journal.

## Example

A library with one request point and one listener, and an application
that wires it. The whole block compiles as a doctest of the crate.

```rust
use std::sync::Arc;
use bus_contract::{Bus, ListenOpts, Point, PointDecl, PointId, PointKind, declare_all, order_dump};

/// The library's request: a document closes.
pub struct DocClose;
impl Point for DocClose {
    // A bad id here is a compile error: see "Naming points".
    const ID: PointId = PointId::new("doc.close");
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = Vec<String>;
}

/// The library's points, readable before anything is built.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<DocClose>("A document closes.")];

/// The library takes the bus; it never constructs one.
pub struct Documents(Arc<dyn Bus>);

impl Documents {
    pub fn new(bus: Arc<dyn Bus>) -> Documents {
        declare_all(&*bus, POINTS);
        let opts = ListenOpts::new().owner("docs");
        let _ = bus.listen::<DocClose>("docs.autosave", opts, |doc, did| {
            did.push(format!("saved {doc}"));
            Ok(())
        });
        Documents(bus)
    }

    /// Asks every listener to close `doc`; returns what each did.
    pub fn close(&self, doc: &str) -> Vec<String> {
        self.0.alter::<DocClose>(doc.to_owned(), Vec::new()).value
    }
}

/// The application creates the bus and passes it to each library.
/// `bus` is `Arc::new(bus_kernel::Kernel::new())`, or your own `Bus`.
pub fn app(bus: Arc<dyn Bus>) {
    let docs = Documents::new(bus.clone());
    assert_eq!(docs.close("notes.txt"), ["saved notes.txt"]);
    // "who runs when" on a point, for reviews and debug views:
    print!("{}", order_dump(&*bus, &["doc.close"]));
}
```

## What is in the workspace

Each crate is released and versioned on its own
([RELEASING.md](https://github.com/quazardous/bus-contract/blob/main/RELEASING.md)).
The core names none of the capabilities: each capability crate owns its
trait, its `CONTRACT` and its facets, and an extension trait gives its
getter on any bus through the core's typed lookup `Bus::capability`,
so a capability contract can change without a core release.

| Crate | What for | Trait, getter | Details |
|---|---|---|---|
| [`bus-contract`](https://crates.io/crates/bus-contract) | The core: points, order, alter and collect, requests, local listeners, introspection, the capability lookup | `Bus` | this page |
| [`bus-contract-defer`](https://crates.io/crates/bus-contract-defer) | Code that cannot be re-entered (an interface drawing): requests asked now, run at the next drain | `Defer`, `BusDefer` → `bus.defer()` | [defer](https://github.com/quazardous/bus-contract/blob/main/docs/defer.md) |
| [`bus-contract-timings`](https://crates.io/crates/bus-contract-timings) | Finding slow listeners: times per point and per listener, budgets reported | `Timings`, `BusTimings` → `bus.timings()` | [timings](https://github.com/quazardous/bus-contract/blob/main/docs/timings.md) |
| [`bus-contract-journal`](https://crates.io/crates/bus-contract-journal) | Publishers that must not wait: numbered events, read again, delivered off the publisher (facet `JOURNALED`) | `Journal`, `BusJournal` → `bus.journal()` | [journal](https://github.com/quazardous/bus-contract/blob/main/docs/journal-and-relay.md#journal) |
| [`bus-contract-crossing`](https://crates.io/crates/bus-contract-crossing) | One bus over several processes: what crosses, with origin and authorities (requires the journal; facets `relayable_event`, `relayable_request`) | `Relay`, `BusCrossing` → `bus.relay()` | [relay](https://github.com/quazardous/bus-contract/blob/main/docs/journal-and-relay.md#relay), [crossing](https://github.com/quazardous/bus-contract/blob/main/docs/crossing.md) |

`Bus` is an object-safe trait (`Arc<dyn Bus>`). The core also gives
typed calls on `dyn Bus` (`declare::<P>`,
`listen::<P>`, `listen_local::<P>`, `alter::<P>`, `collect::<P>`,
`on_failure`, `on_pending`), written once over the erased methods, so an
implementation writes only those; listeners with an owner (`forget`
removes them all), `unlisten`, failure sinks and introspection; a
contract table (`PointDecl`, a `const` declared with `declare_all`); and
tools on it: `order_dump` and `missing_targets` (an ordering constraint
naming a listener that is not there). No dependencies, no `unsafe`, no
async runtime.

Upgrading from 0.4: [MIGRATION-0.5.md](https://github.com/quazardous/bus-contract/blob/main/MIGRATION-0.5.md).

## Rules, one page each

- **[Naming points](https://github.com/quazardous/bus-contract/blob/main/docs/naming.md):**
  ids are `<crate>.<subject>.<moment>`, so libraries sharing a bus
  cannot collide; a bad `PointId` in a `const` is a compile error.
- **[Local listeners](https://github.com/quazardous/bus-contract/blob/main/docs/local-listeners.md):**
  `listen_local::<P>` for state that is not `Send`, bound to its
  thread, ordered with the others; an emit from another thread queues
  it for that thread's `run_pending()`.
- **[Refusals](https://github.com/quazardous/bus-contract/blob/main/docs/refusals.md):**
  a listener of a request returns `HookError::refuse(reason)` and the
  request stops there; a point declared `fail_closed` turns a listener
  failure into a refusal.
- **[Capabilities](https://github.com/quazardous/bus-contract/blob/main/docs/capabilities.md):**
  needing one (`needs`, facets: if you mark a point `JOURNALED`, the bus
  must provide `Journal`, or `declare_all` refuses it), doing without
  one, and offering one from an implementation.
- **[Crossing](https://github.com/quazardous/bus-contract/blob/main/docs/crossing.md):**
  what may cross a process boundary and what it means: deadlines that
  fail closed, vetoes with their reason, asks answered once, origin,
  authorities.
- **[Journal and Relay](https://github.com/quazardous/bus-contract/blob/main/docs/journal-and-relay.md):**
  how to publish, read again and relay events, and why they are shaped
  this way.

## Not included

- Transport across processes: a relay reads, takes in and answers; a
  bridge moves the text (framing, wire format, sockets, reconnection,
  discovery, back-pressure).

## License

MIT OR Apache-2.0, at your option.
