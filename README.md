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
events, alter, collect) with ordered listeners.

A library declares its points and listeners against the `Bus` trait.
The application creates the bus and passes it to each library at
construction. There is no global.

## Crates

This repository is a Cargo workspace. Each crate is released and
versioned on its own ([RELEASING.md](https://github.com/quazardous/bus-contract/blob/main/RELEASING.md)):

| Crate | Contract |
|---|---|
| [`bus-contract`](https://crates.io/crates/bus-contract) | the core: points, order, alter and collect, requests, local listeners, introspection, and the capability lookup |
| [`bus-contract-defer`](https://crates.io/crates/bus-contract-defer) | `Defer`: requests asked now, run at the next drain |
| [`bus-contract-timings`](https://crates.io/crates/bus-contract-timings) | `Timings`: listener times and budgets |
| [`bus-contract-journal`](https://crates.io/crates/bus-contract-journal) | `Journal`: numbered events, read again, delivered off the publisher |
| [`bus-contract-crossing`](https://crates.io/crates/bus-contract-crossing) | `Relay`: what crosses a process boundary, on top of the journal |

Since `bus-contract` 0.5 each capability crate owns its trait and types,
its `CONTRACT` and its facets, and adds an extension trait
(`BusDefer`, `BusTimings`, `BusJournal`, `BusCrossing`) that gives
`bus.defer()`, `bus.timings()`, `bus.journal()` and `bus.relay()` on any
bus through the core's typed lookup. The core names none of them, so a
capability contract can change without a core release.

Upgrading from 0.4: [MIGRATION-0.5.md](https://github.com/quazardous/bus-contract/blob/main/MIGRATION-0.5.md).

## Contents

- **Points are types** (`Point`): an id (`PointId`, `doc.close`, checked
  when made), the input every listener reads, the value a listener
  edits, and a declared kind (`request`, `event`, `alter`, `collect`).
- **`Bus`**, an object-safe trait (`Arc<dyn Bus>`): declare a point;
  listen under an id and an owner, ordered by `after` / `before` /
  `last`, then priority; unlisten; forget an owner; `alter` and
  `collect` emits that return every listener's failure (error, panic,
  missed deadline); failure sinks; introspection.
- **Local listeners** (`listen_local::<P>`) for state that is not
  `Send` (`Rc<RefCell<…>>`): bound to the thread that registered them,
  in the same order as the other listeners. An emit from another
  thread queues them for their thread's `run_pending()`.
- **Refusals on requests**: a listener returns `HookError::refuse(reason)`
  and the request stops there; a point declared `fail_closed` turns a
  listener failure into a refusal.
- **Optional capabilities**, found at run time through the typed lookup
  `Bus::capability`, each defined in its own crate: `Defer` (requests
  asked now, heard at the next drain on the same thread), `Timings`
  (per-point and per-listener times, passes over a point's budget
  reported), `Journal` (events with a number, read again from one,
  delivered after the publisher has moved on) and `Relay` (what crosses
  a process boundary: events of relayable points served to another
  process and taken in from one, asks of relayable requests answered
  for one, each with its origin, behind the receiver's authorities).
- **Typed calls on `dyn Bus`** (`declare::<P>`, `listen::<P>`,
  `listen_local::<P>`, `alter::<P>`, `collect::<P>`, `on_failure`,
  `on_pending`), written once over the erased methods: an
  implementation writes only those.
- **A contract table** (`PointDecl`): a library's points as a `const`,
  readable before anything is built, declared with `declare_all`.
- **Tools on the contract**: `order_dump` ("who runs when", for reviewed
  files and debug views) and `missing_targets` (an ordering constraint
  naming a listener that is not there).
- No dependencies, no `unsafe`, no async runtime.

This crate has no implementation. The reference one is `Kernel` in
[`bus-kernel`](https://github.com/quazardous/bus-kernel), whose
`conformance` feature checks any implementation against the guarantees
documented on `Bus`.

## Guarantees

[GUARANTEES.md](https://github.com/quazardous/bus-contract/blob/main/GUARANTEES.md)
(also the `guarantees` page of the crate docs) lists, contract by
contract (core, defer, timings, journal, crossing), each property with a
stable id such as `core.order.declared`, `core.request.veto-stops` or
`crossing.deadline.fail-closed`: what is guaranteed, what is not, and
the `bus-kernel` conformance check that proves it. In short:

- listeners run in their declared order; a failing listener is
  isolated, its edit dropped, the failure returned;
- a veto stops a request, but **never rolls back** what the listeners
  before it did;
- requests are fail-open unless declared `fail_closed` (relayable
  requests always are); `PointKind` is not only a label: `Request`
  enables refusals, `Event` gates the journal;
- crossing is **not a distributed transaction**: both sides agree on
  the outcome, effects made before a deadline stay, so listeners of
  relayable requests should be idempotent;
- authorities guard a trusted process against mistakes, not against an
  attacker; the journal is a window in memory, with no persistence and
  no exactly-once across processes.

## Example

```rust
use std::sync::Arc;
use bus_contract::{Bus, ListenOpts, Point, PointDecl, PointId, PointKind, declare_all};

/// The library's request: a document closes.
pub struct DocClose;
impl Point for DocClose {
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

    pub fn close(&self, doc: &str) -> Vec<String> {
        self.0.alter::<DocClose>(doc.to_owned(), Vec::new()).value
    }
}
```

The application creates the bus and passes it to each library:

```rust,ignore
let bus: Arc<dyn Bus> = Arc::new(bus_kernel::Kernel::new()); // or its own
let docs = Documents::new(bus.clone());
let search = Search::new(bus.clone());
print!("{}", bus_contract::order_dump(&*bus, &["doc.close"]));
```

## Naming points

Several libraries share one bus, so a point's id starts with a prefix
that names the crate declaring it, then the subject and the moment:
`<crate>.<subject>.<moment>`.

- A library uses one short prefix of its own, written in its docs:
  `stack.layout.changed`, `stack.window.closed` for a docking crate
  called `egui_stack`; `doc.close` for the example above.
- An event names what happened, in the past (`…changed`, `…closed`).
  A request names what is asked (`…close`, `…drop`).
- A library never declares a point under another crate's prefix, and
  never asks another crate to rename its points to free a bare name.
- Listener ids follow the same rule (`docs.autosave`), so the
  `order_dump` of a point shows who each listener belongs to.

Two libraries that keep to this cannot collide, whatever the
application puts on its bus. A clash is refused at declaration time
(`Refused::TypeMismatch`): a point id is registered with one `Point`
type.

The rule is checked (`check_id`): at least two dot-separated segments,
none empty; the prefix is `[a-z][a-z0-9_-]*`, the other segments
`[A-Za-z0-9_-]+`.

- A point's id is a `PointId`. `PointId::new` is a `const fn` that
  panics on a bad id, so `const ID: PointId = PointId::new("close");`
  does not compile. It reads as a `&str` (`Deref`, `as_str`, comparisons
  with `str`).
- A listener's id may be a `ListenerId` (`ListenerId::new` in a `const`,
  `ListenerId::try_new` at run time), or any string: the bus checks it
  when the listener registers and refuses a bad one
  (`Refused::BadId`). An implementation also refuses a point key whose
  id breaks the rule.

Newtype ids checked at construction come from
[eventbus-contract](https://github.com/mapseekai/eventbus-contract)
(`Topic::new`, `ConsumerGroup::new`); the `const` form that fails at
compile time from [qubit-event-bus](https://github.com/qubit-ltd/rs-event-bus)
(`Topic::new_static`).

## Local listeners

A listener registered with `listen_local::<P>` may hold state that is
not `Send`, such as an interface's `Rc<RefCell<…>>`. It is bound to the
thread that registered it.

- It is ordered with the other listeners of its point (`after`,
  `before`, `last`, priority), keeps its owner and deadline, and shows
  in introspection as local.
- An emit on its thread runs it in that order with the others.
- An emit on another thread runs the other listeners, queues the local
  ones and names them in the result (`deferred`). The wakers added with
  `on_pending` are called with the thread's id. The thread runs the
  queued listeners, in order, when it calls `run_pending()`, typically
  once per pass of its loop.
- `unlisten` and `forget` remove local listeners too, queued runs
  included.

```rust,ignore
let panel = Rc::new(RefCell::new(Panel::default()));
let mine = panel.clone();
bus.listen_local::<DocClose>("ui.panel", ListenOpts::new().after("docs.autosave"), move |doc, _| {
    mine.borrow_mut().closed(doc);
    Ok(())
})?;
bus.on_pending(move |_| ctx.request_repaint());
// each pass of the interface's loop:
bus.run_pending();
```

## Refusals

On a point declared `PointKind::Request`, a listener may refuse the
request with a reason. The request stops: no later listener runs, the
refusing listener's edit is dropped, and the result names the refusal
(`Altered::refused`: point, listener, owner, reason). A refusal is not
a failure and does not reach the failure sinks.

By default a failing listener (error, panic, missed deadline) has its
contribution dropped and the others run. On a point declared
`fail_closed`, the failure refuses the request instead: the failure is
reported as usual and `refused` is set with `failed: true`. A policy
listener placed first on a fail-closed point therefore never lets the
handler run when the policy itself breaks.

```rust,ignore
pub const POINTS: &[PointDecl] =
    &[PointDecl::of::<CommandRun>("A command runs.").fail_closed()];

bus.listen::<CommandRun>("app.policy", ListenOpts::new().priority(1000), |run, _| {
    if run.origin.is_agent() && run.id == "files.delete" {
        return Err(HookError::refuse("agents may not delete files"));
    }
    Ok(())
})?;
let out = bus.alter::<CommandRun>(run, Outcome::default());
if let Some(refusal) = out.refused {
    // "hook `command.run`: refused by `app.policy`: agents may not delete files"
}
```

## Capabilities

The `Bus` trait is the core every implementation offers. An
implementation may offer more, each a contract of its own, defined in a
crate of its own and found at run time through the typed lookup
`Bus::capability`, which returns `None` by default. One `dyn Bus` type
works everywhere, whatever the implementation offers.

| Contract | Crate | Trait | Extension trait | `CONTRACT` | Facets |
|---|---|---|---|---|---|
| defer | `bus-contract-defer` | `Defer` | `BusDefer` (`bus.defer()`) | `bus_contract_defer::CONTRACT` | |
| timings | `bus-contract-timings` | `Timings` | `BusTimings` (`bus.timings()`) | `bus_contract_timings::CONTRACT` | |
| journal | `bus-contract-journal` | `Journal` | `BusJournal` (`bus.journal()`) | `bus_contract_journal::CONTRACT` | `JOURNALED` |
| crossing | `bus-contract-crossing` | `Relay` | `BusCrossing` (`bus.relay()`) | `bus_contract_crossing::CONTRACT` (requires journal) | `relayable(&CODEC)`, `relayable_event::<P>()`, `relayable_request::<P>()` |

- A library that needs a capability names its contract in its table
  (`PointDecl::needs(&[&bus_contract_defer::CONTRACT])`), or gives the
  point a facet of it (`PointDecl::facets(&[JOURNALED])`): what the
  declaration says for that contract, data only that contract's crate
  reads. `declare_all` refuses such a point on a bus without the
  contract (`Refused::Unsupported`) and leaves it undeclared.
- A library that can do without one calls the extension trait
  (`use bus_contract_defer::BusDefer;` then `bus.defer()`) and falls
  back when it gets `None`.
- `offers(bus, &bus_contract_defer::CONTRACT)` tells whether a bus has
  one, and the contracts it requires.

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
build.

### Defer

`bus.defer()` (`use bus_contract_defer::BusDefer;`) returns
`Option<&dyn Defer>`. A request asked through it
returns at once; its listeners run at the next `drain()` on the same
thread. It is for code that cannot be re-entered while it runs, such as
an interface drawing a panel with its state borrowed.

- Asks are per thread and run in ask order, each as an alter emit with
  the listeners registered at the drain. Refusals and fail-closed
  points apply.
- `drain()` returns what each ask did (`PendingRun` with `ask` set: the
  listeners that ran, failures, refusal). The asker gets nothing back.
- An ask made during a drain waits for the next drain. An immediate
  emit made after an ask runs before it.
- `waiting()` counts the asks queued for the calling thread, so a host
  can drain before it exits.

```rust,ignore
pub const POINTS: &[PointDecl] = &[PointDecl::of::<LibraryWrite>("A library write.")
    .needs(&[&bus_contract_defer::CONTRACT])];

// while drawing, with the library borrowed:
bus.defer().expect("declared with needs").ask::<LibraryWrite>(write, Vec::new());
// after drawing, where nothing is borrowed:
for run in bus.defer().map(|d| d.drain()).unwrap_or_default() {
    if let Some(refusal) = run.refused { log::warn!("{refusal}"); }
}
```

### Timings

`bus.timings()` (`use bus_contract_timings::BusTimings;`) returns
`Option<&dyn Timings>`: how long listeners
take, per point and per listener (`TimingStats`: count, total, max,
p50, p95), since the last `reset()`.

- A pass is what one emit's listeners ran on one thread (an inline
  emit, a `run_pending` run, a drained ask); its time is its listeners'
  times added up.
- A point may declare a budget for a pass (`PointDecl::budget`,
  `PointSpec::budget`). A pass over it goes to the budget sinks
  (`on_over_budget`, `OverBudget` with each listener's time), on the
  thread that ran it. Nothing fails: a budget is not a deadline. A host
  publishes it as the `bus.overBudget` fact (`OVER_BUDGET_FACT`) or logs.
- A budget needs no capability: on a bus without `Timings` it is only
  shown in introspection (`PointInfo::budget`).

```rust,ignore
pub const POINTS: &[PointDecl] = &[PointDecl::of::<ImageOpened>("An image was opened.")
    .budget(Duration::from_millis(4))];

if let Some(timings) = bus.timings() {
    timings.on_over_budget(|over| log::warn!("{over}"));
    for point in timings.points() {
        for l in point.slowest().iter().take(3) {
            println!("{} {} p95 {:?}", point.point, l.id, l.stats.p95);
        }
    }
}
```

### Journal

`bus.journal()` (`use bus_contract_journal::BusJournal;`) returns
`Option<&dyn Journal>`. An event point declared with the `JOURNALED`
facet is published through the journal instead of
emitted: the publisher gets a number and moves on, and the listeners
hear each event once, in number order, on a thread of the
implementation's (a pump). A slow listener delays the next events, not
the publisher.

- `publish::<P>(input)` / `publish_caused::<P>(input, cause)`: the
  entry's number, from 1. Refused on a point not declared journaled.
- `since(seq, limit)`: the entries after a number, with `gap` set when
  some left the journal's window; `last_seq()`.
- `wait(patterns, seq, timeout)` / `wait_for::<P>`: the next entry of a
  kind (`vault.*`, see `kind_matches`), at once if already there.
- `flush(timeout)`: every entry recorded so far delivered (before an
  exit, in tests).

### Relay

`bus.relay()` (`use bus_contract_crossing::BusCrossing;`) returns
`Option<&dyn Relay>`, on a bus that offers the journal. An event point
declared with the `relayable_event::<P>()` facet (codec
`Codec::of::<P>()`) has its
input written as text and read back by another process (`Relayed`: the
format is the application's). The transport is not part of the
contract (see [Crossing](#crossing)).

- `cursor()`: where a reader starting now stands (the journal's run and
  last number). `resume(cursor, limit)`: the relayable entries after
  it, and `lost` when the reader missed some (`Gap`: the window;
  `Restarted`: the process started again and numbers with it). A
  reader that lost entries resyncs, then applies the ones returned.
- `ingest(origin, entry)`: an entry relayed in is recorded with its
  origin and delivered to its point's listeners like a local one. A
  kind not declared here is kept as text and heard by no one; a point
  declared here but not relayable, or at another version, is refused.
  Entries taken in are not served again.

```rust,ignore
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<LibraryImported>("Photos were imported.")
        .facets(&[relayable_event::<LibraryImported>()]),
    PointDecl::of::<StepComputed>("A render step was computed.").facets(&[JOURNALED]),
];

// the node publishes and moves on:
bus.journal().expect("declared journaled").publish::<LibraryImported>(imported)?;
// its server answers a desktop reader:
let read = bus.relay().expect("declared relayable").resume(cursor, 256);
// the desktop takes the entries in; its listeners hear them in order:
for entry in read.entries { desktop.relay().unwrap().ingest(&Origin::process("node"), entry)?; }
```

Why it is shaped this way: [docs/journal-and-relay.md](https://github.com/quazardous/bus-contract/blob/main/docs/journal-and-relay.md).

### Crossing

One bus may span several processes (a desktop and its node, a plugin,
an agent's session). The contract says **what** may cross a process
boundary and **what it means** when it does; it never says **how** it
crosses. Framing, wire format, transports, reconnection and discovery
belong to a bridge built on these traits. Whatever the bridge, a
listener cannot tell whether the far side is in its process or not:
that is the meaning a conformance suite checks.

**What may cross.** A point crosses only when its table line says so,
with the crossing facet carrying its codec (`relayable(&CODEC)`):

- an event point (`relayable_event::<P>()`, codec `Codec::of::<P>()`,
  `Relayed`): its events, as above; the bus journals them;
- a request point (`relayable_request::<P>()`, codec
  `Codec::request::<P>()`, `RelayedRequest`: the value is written and
  read too): asks from another process. A relayable request is
  fail-closed.

Alter and collect points never cross (`Refused::Contract` from the
crossing contract). Each
point has a version (`PointDecl::version`, from 1), raised when its
payload changes in a way an older reader cannot use; a field a reader
may ignore is not such a change. Two processes cross a point only at
the same version: another one is refused (`IngestError::Version`, or a
refused answer).

**A request answered on the other side.** On the asking side the far
side is one listener, `far_listener(codec, deadline, version, send)`,
registered on the point like any other: the asking side's order decides
when the far side takes its turn. `send` is the bridge's part: it moves
the ask (`RelayedAsk`: input and value so far as text, the deadline, a
number) to the far side's `Relay::answer`, and the answer back.

- **Deadline, fail-closed.** An ask carries its deadline. No answer in
  time (a link down, a far side gone, too slow) refuses the request:
  "no answer" means "no". The far side counts the deadline from when it
  got the ask, runs nothing when it has passed, and answers refused
  when its listeners end past it, so both sides hold the same outcome.
- **A veto carries its reason.** A far listener's refusal comes back as
  `Answer::Refused` with the listener and its reason, and refuses the
  request here with that reason; the far side's edit is dropped. A
  refusal by the far bus itself (an authority, a point it does not
  declare as a relayable request, another version, an input it cannot
  read) has no listener.
- **Answered once.** An ask is named by the asker's run and its number.
  The far side runs the listeners of an ask once per origin: a repeat
  gets the first answer again, or a refusal once it has been dropped.
  The asking side takes one answer, to its own ask, in time; anything
  else refuses.

Effects the far side's listeners made before the deadline ran out are
theirs to undo or to make idempotent: the contract makes both sides
agree on the outcome, not roll back work.

**Origin.** Whatever crosses in is taken with an `Origin`: the process
on the far side of the link, and the agent it speaks for when the link
is an agent's session. The receiving side sets it from the link, never
from what the sender wrote. Every entry taken in keeps it
(`JournalEntry::origin`); every ask is checked against it.

**Authority.** `Relay::add_authority` (typed: `authorize`) adds a check
shown each crossing (`Crossing`: origin, kind, version, ask or event)
before anything is decoded, recorded or run. Every authority is asked,
in order; one refusal refuses (`IngestError::Refused`, or a refused
answer); a panic refuses.

```rust,ignore
pub const POINTS: &[PointDecl] = &[PointDecl::of::<FileDelete>("Files are deleted.")
    .facets(&[relayable_request::<FileDelete>()])];

// the node: agents may not delete.
node.relay().unwrap().authorize(|c| match &c.origin.agent {
    Some(agent) if c.kind == "files.delete" => Err(format!("{agent} may not delete")),
    _ => Ok(()),
});
// its bridge answers each ask it reads off its link:
let answer = node.relay().unwrap().answer(&link_origin, ask);

// the desktop: the node takes its turn after the desktop's own listeners.
desktop.listen_any(
    PointKey::of::<FileDelete>(),
    "desktop.node".into(),
    ListenOpts::new().after("desktop.confirm").deadline(PLUGIN_DEADLINE).threaded(),
    far_listener(Codec::request::<FileDelete>(), PLUGIN_DEADLINE, 1, move |ask| link.ask(ask)),
)?;
let out = desktop.alter::<FileDelete>(path, Vec::new());
if let Some(refusal) = out.refused { /* the node said no, or said nothing in time */ }
```

## Not included

- Transport across processes: a relay reads, takes in and answers; a
  bridge moves the text (framing, wire format, sockets, reconnection,
  discovery, back-pressure).

## License

MIT OR Apache-2.0, at your option.
