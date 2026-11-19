# Changelog

## 0.4.4

Additive: nothing changes for existing implementations or callers.

- `Bus::capability(TypeId) -> Option<Provided<'_>>`, a typed capability
  lookup keyed by the `TypeId` of the capability's trait object type
  (`dyn Defer`, `dyn Timings`, `dyn Journal`, `dyn Relay`, or one the
  core does not name). It returns `None` by default. `Provided` holds
  the bus as `Any` and a `'static` cast (`Cast<C>`), so the lookup needs
  no `unsafe` and `Bus` stays object safe.
- `capability::<C, _>(&bus)` and `get::<C>()` on `dyn Bus` give
  `Option<&C>`. A bus answering one `TypeId` with another capability's
  cast gives `None`, never a wrong type.
- `defer()`, `timings()`, `journal()` and `relay()` default to the
  lookup, so a new implementation writes `capability` only. One that
  overrides the getters keeps working through them, `offers` and
  `declare_all`; its lookup answers `None` until it implements
  `capability`. `Arc<B>` forwards `capability`.
- `GUARANTEES.md`: `core.capability.lookup-agrees`, each capability
  reached through its getter and the lookup with the same answer,
  checked by `bus-kernel` 0.2.2 (`capability.getter_and_lookup_agree`).

## 0.4.3

Docs only: no change to the traits or their behaviour.

- `GUARANTEES.md`: the rows that were "not covered yet" name the
  `bus-kernel` 0.2.1 checks that now prove them: the panicking waker,
  failure sink and budget sink, the order of queued emits, listening on
  the emitting point, the defer wakers and a listener removed between
  ask and drain, the refusal that keeps earlier effects, journal numbers
  growing by one, the `since` gap, `wait` / `wait_for` and the cause,
  `Lost::Gap` and `Lost::Restarted`, no echo, and relay implying
  journal. Only local listeners queued from the journal's pump are
  still not covered.

## 0.4.2

Docs only: no change to the traits or their behaviour.

- `GUARANTEES.md`, also in the crate docs as the `guarantees` module and
  summarised in the README: for each contract (core, defer, timings,
  journal, crossing), every property with a stable id
  (`core.order.declared`, `core.request.veto-stops`,
  `crossing.deadline.fail-closed`, …), what is guaranteed, what is not,
  and the `bus-kernel` conformance check that proves it, or "not covered
  yet".
- Stated explicitly: a refusal never rolls back earlier listeners'
  effects; requests are fail-open unless declared `fail_closed`;
  `PointKind` is not purely declarative (`Request` changes semantics,
  `Event` gates the journal); crossing is not a distributed transaction;
  authorities are not a security boundary; no persistence or
  exactly-once across processes.
- Credits: the guarantees-page style and its "what we do not guarantee"
  framing come from
  [eventbus-contract](https://github.com/mapseekai/eventbus-contract)
  and [qubit-event-bus](https://github.com/qubit-ltd/rs-event-bus).

## 0.4.1

- Docs: the reference implementation is `bus-kernel`'s `Kernel` (it
  moved out of `egui-pop-hooks`), with a `conformance` feature that
  checks any implementation.

## 0.4.0

Crossing: what may cross a process boundary and what it means. The
transport stays out of the contract.

- Requests cross: `PointDecl::relayable` takes a request point with a
  request codec (`Codec::request::<P>()`, the `RelayedRequest` trait:
  the value written and read too) and makes it fail-closed;
  `PointSpec::relayable_request`. Alter and collect points never cross
  (`Refused::NotCrossable`).
- `Relay::answer(origin, ask)`: an ask (`RelayedAsk`: run, number,
  kind, version, deadline, input and value as text) answered by the far
  side's listeners (`RelayedAnswer`, `Answer::Done` / `Answer::Refused`
  with the far listener and its reason). Fail-closed on the deadline on
  both sides; answered once per origin.
- `far_listener(codec, deadline, version, send)`: the asking side's
  listener standing for the far side; no answer in time, a late answer,
  an answer to another ask or one it cannot read refuses. `ask_run()`.
- `Origin` (process, agent): set by the receiver from the link.
  `JournalEntry::origin`.
- Authority: `Relay::add_authority` (typed `authorize`), `Authority`,
  `Crossing`; checked before anything crossing in is decoded, recorded
  or run. `IngestError::Refused`.
- Point versions: `PointDecl::version`, `PointSpec::version`,
  `PointInfo::version`, `RelayedEntry::version`;
  `IngestError::Version`.
- `PointInfo::relayable` is set on relayable requests too.

Breaking, and why:

- `Relay::ingest` takes `&Origin` instead of `&str`: the origin is
  structured (process and agent) so an authority can decide on it.
- `JournalEntry::source: Option<String>` becomes
  `JournalEntry::origin: Option<Origin>`, for the same reason.
- `Relay` has two new required methods, `answer` and `add_authority`:
  a default would either drop the authority (fail-open) or refuse every
  ask silently.
- `Codec` gains fields (`encode_value`, `decode_value`, `clone_value`)
  and is now `#[non_exhaustive]`: build it with `Codec::of` or
  `Codec::request`.
- `PointDecl::relayable` on a request point used to be refused
  (`Refused::NotEvent`, since it also made the point journaled); it now
  declares a crossing request (fail-closed, not journaled), or is
  refused as `Refused::NotCrossable` with an event codec. On an alter or
  collect point, `Refused::NotEvent` becomes `Refused::NotCrossable`.

Migration from 0.3:

- `relay.ingest("node", entry)` becomes
  `relay.ingest(&Origin::process("node"), entry)`; `entry.source`
  becomes `entry.origin` (`.process` holds the old string).
- An implementation of `Relay` adds `answer` (see the promises on
  `Relay`) and `add_authority`, checks authorities in `ingest`, refuses
  an entry at another version, and fills `RelayedEntry::version` in
  `resume`. Its `declare_point` accepts a relayable request and refuses
  `NotCrossable` points.

## 0.3.3

- Docs: point ids start with the declaring crate's prefix,
  `<crate>.<subject>.<moment>`. README: "Naming points", so libraries
  sharing one bus cannot collide.

## 0.3.2

- The `Journal` capability: `Bus::journal()` (provided, `None` by
  default) returns `Option<&dyn Journal>`: `last_seq`, `since` (with
  `Since::gap`), `wait` / typed `wait_for::<P>`, `publish_any` / typed
  `publish::<P>` and `publish_caused::<P>`, `flush`. A journaled event
  point's events are recorded with a number and delivered to its
  listeners in number order, never on the publisher's thread.
- The `Relay` capability: `Bus::relay()` returns `Option<&dyn Relay>`:
  `cursor`, `resume` (`Resumed`, `Lost::Gap`, `Lost::Restarted`),
  `ingest` (`IngestError`). `Cursor`, `RelayedEntry`, `JournalEntry`.
- Contract tables: `PointDecl::journaled`, `PointDecl::relayable(codec)`,
  `PointDecl::required`; `PointSpec::journaled` / `PointSpec::relay`;
  `PointInfo::journaled` / `PointInfo::relayable`. `Relayed` and
  `Codec::of::<P>()` write and read a relayable point's input as text.
- `declare_all` refuses a journaled or relayable point on a bus without
  the capability (`Refused::Unsupported`), on a point that is not an
  event (`Refused::NotEvent`), and with another point's codec
  (`Refused::TypeMismatch`). `Refused::NotJournaled` refuses a publish.
- `Capability::Journal`, `Capability::Relay`; `kind_matches` reads the
  event kind patterns every implementation shares (`vault.*`).
- A design note: `docs/journal-and-relay.md`.

Not breaking: the types gaining a field or variant are
`#[non_exhaustive]` (`PointSpec`, `PointInfo`, `Refused`, `Capability`)
or have a private field (`PointDecl`); an implementation of `Bus` needs
no new method.

## 0.3.1

- The `Timings` capability: `Bus::timings()` (provided, `None` by
  default) returns `Option<&dyn Timings>`: `points`, `point`, `reset`,
  `add_budget_sink` (typed: `on_over_budget`). Per point and per
  listener, `TimingStats` (count, total, max, p50, p95);
  `PointTimings::slowest`.
- Budgets: `PointDecl::budget` / `PointSpec::budget`, shown in
  `PointInfo::budget`. A pass over its point's budget is reported as an
  `OverBudget` (each listener's time, `slowest`), never failed;
  `OVER_BUDGET_FACT` (`bus.overBudget`) names the fact a host publishes.
- `Capability::Timings`; `BudgetSink`.

Not breaking: the types gaining a field or variant are
`#[non_exhaustive]` (`PointSpec`, `PointInfo`, `Capability`) or have a
private field (`PointDecl`); an implementation of `Bus` needs no new
method.

## 0.3.0

- Refusals: a listener returns `HookError::Refuse` (`HookError::refuse`)
  and a request stops there; the result names it (`Refusal`, in
  `Altered::refused`, `Collected::refused`, `PendingRun::refused`).
- Fail-closed points: `PointSpec::fail_closed` / `PointDecl::fail_closed`
  turn a listener failure on a request into a refusal;
  `PointInfo::fail_closed` shows it.
- Optional capabilities: `Bus::defer()` (provided, `None` by default)
  returns the `Defer` capability: `ask_any` / typed `ask::<P>`, `drain`,
  `waiting`. `PendingRun::ask` numbers a deferred request's run.
- `Capability`, `offers`, `PointDecl::needs`; `declare_all` refuses a
  point needing a capability the bus lacks (`Refused::Unsupported`).
- Constructors for implementations: `Altered::new`, `Collected::new`,
  `PendingRun::new`, `ListenerInfo::new`, `PointInfo::new`,
  `Failure::new`, `Refusal::new`, `Refusal::from_failure`.
- `FailureKind` implements `Display` (the part of a failure's message
  after the listener).

Breaking, and why: these types are now `#[non_exhaustive]`, so the next
additions to them are not breaking: `PointSpec`, `PendingRun`,
`ListenerInfo`, `PointInfo`, `Failure`, `Altered`, `Collected`,
`Refusal`, `HookError`, `FailureKind`, `Refused`, `Capability`.
`HookError` is now an enum (`Failed`, `Refuse`).

Migration from 0.2:

- Build the types above with their constructor, then set the public
  fields: `PendingRun { point, ran, failures }` becomes
  `let mut run = PendingRun::new(point); run.ran = ran; run.failures = failures;`.
- `HookError(message)` becomes `HookError::failed(message)` (or
  `message.into()`); `error.0` becomes `error.message()`.
- A `match` on `FailureKind` or `Refused` outside this crate needs a
  `_` arm.
- An implementation of `Bus` needs no new method. It builds its results
  with the constructors, honours refusals and `fail_closed` on request
  points (see `Bus`), and may offer `Defer` by overriding `Bus::defer`.

## 0.2.0

- Local listeners: `Bus::listen_local_any` and the typed
  `listen_local::<P>` register a listener that is not `Send`, bound to
  the thread that registered it and ordered with the other listeners.
  An emit from another thread queues it for that thread's
  `Bus::run_pending`; `Bus::add_pending_waker` (typed: `on_pending`)
  is called when work is queued.
- `Altered` and `Collected` gain `deferred`: the local listeners an
  emit queued for another thread.
- `PendingRun`: what one emit's queued listeners did.
- `ListenerInfo` gains `local`; `order_dump` marks local listeners.
- `Refused::Threaded`: a local listener asked for a thread of its own.
- `erase_local`, `AnyLocalListener`, `PendingWaker`.
- Breaking: `Bus` has three new required methods; `Altered`,
  `Collected`, `ListenerInfo` and `Refused` have new fields or variants.

## 0.1.0

- First release: typed hook points, ordered listeners with owners and
  deadlines, alter and collect emits, failure sinks, introspection, the
  contract table and the tools on it.
