# Changelog

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
