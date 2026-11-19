#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod journal;
mod tools;
mod types;

/// What an implementation guarantees and what it does not, contract by
/// contract, each property with a stable id and the conformance check
/// that proves it. Documentation only: this module has no items.
#[doc = include_str!("../GUARANTEES.md")]
pub mod guarantees {}

use std::any::{Any, TypeId, type_name};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub use journal::{
    Answer, Authority, Codec, Crossing, Cursor, DecodeFn, DecodeValueFn, EncodeFn, EncodeValueFn,
    IngestError, Journal, JournalEntry, Lost, Origin, Relay, Relayed, RelayedAnswer, RelayedAsk,
    RelayedEntry, RelayedRequest, Resumed, Since, ask_run, far_listener, kind_matches,
};
pub use tools::{declare_all, missing_targets, order_dump};
pub use types::*;

/// A point as the erased methods of [`Bus`] name it: its id, its kind and
/// its type, so an implementation refuses one id registered with two
/// types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointKey {
    /// The point's id ([`Point::ID`]).
    pub id: &'static str,
    /// What it is for ([`Point::KIND`]).
    pub kind: PointKind,
    /// The point type's id.
    pub type_id: TypeId,
    /// The point type's name, for messages.
    pub type_name: &'static str,
}

impl PointKey {
    /// The key of the point `P`.
    pub fn of<P: Point>() -> PointKey {
        PointKey {
            id: P::ID,
            kind: P::KIND,
            type_id: TypeId::of::<P>(),
            type_name: type_name::<P>(),
        }
    }
}

/// An emit's input, its type erased: a point's [`Point::Input`].
pub type AnyInput = Arc<dyn Any + Send + Sync>;

/// A value going through an emit, its type erased: a point's
/// [`Point::Value`].
pub type AnyValue = Box<dyn Any + Send>;

/// A listener, its types erased: it reads the input and edits the value
/// in place.
pub type AnyListener = Arc<
    dyn Fn(&(dyn Any + Send + Sync), &mut (dyn Any + Send)) -> Result<(), HookError> + Send + Sync,
>;

/// A local listener, its types erased: like [`AnyListener`], but neither
/// `Send` nor `Sync`. It is bound to the thread that registered it
/// ([`Bus::listen_local_any`]).
pub type AnyLocalListener =
    Rc<dyn Fn(&(dyn Any + Send + Sync), &mut (dyn Any + Send)) -> Result<(), HookError>>;

/// Called when local work is queued for a thread
/// ([`Bus::add_pending_waker`]), with that thread's id, on the emitting
/// thread.
pub type PendingWaker = Arc<dyn Fn(std::thread::ThreadId) + Send + Sync>;

/// Copies an erased value of a point's type (an alter keeps the value as
/// it was when a listener fails).
pub type CloneFn = fn(&(dyn Any + Send)) -> AnyValue;

/// Makes a fresh erased value of a point's type (each listener's part in
/// a collect).
pub type FreshFn = fn() -> AnyValue;

/// Receives each failure that dropped a contribution
/// ([`Failure::dropped`]).
pub type FailureSink = Arc<dyn Fn(&Failure) + Send + Sync>;

/// Receives each pass over a point's budget ([`OverBudget`]).
pub type BudgetSink = Arc<dyn Fn(&OverBudget) + Send + Sync>;

/// The bus contract: hook points, their listeners in a declared order,
/// emits and introspection. An application supplies one implementation
/// (its own, or one written for it, such as `bus-kernel`'s `Kernel`)
/// and hands it to each
/// library at construction; a library never constructs the bus it
/// publishes on.
///
/// The methods are object safe and their values erased, so a library
/// holds an `Arc<dyn Bus>`; the typed calls (`declare::<P>`,
/// `listen::<P>`, `alter::<P>`, `collect::<P>`, `on_failure`) are methods
/// of `dyn Bus`, written once here over these.
///
/// What an implementation promises:
///
/// - Listeners run ordered by `after` / `before` first (the target
///   [`LAST`] places one after every other of its point, one per point),
///   then by priority (higher first), then by registration. A cycle is
///   refused at registration ([`Refused::Cycle`]); a target not there is
///   reported ([`FailureKind::MissingTarget`]) and ignored.
/// - A listener's edit counts only when it returns `Ok` within its
///   deadline without panicking; otherwise it is dropped, the others
///   still run, and the failure is returned and sent to the sinks.
/// - The value an emit returns is of the point's type: the typed calls
///   panic otherwise, as a defect of the implementation.
/// - Listeners run with no lock held, so a listener may emit or listen.
///
/// Every property, with its stable id, what it does not guarantee and
/// the check that proves it: [`guarantees`].
///
/// Refusals, on a point declared [`PointKind::Request`]:
///
/// - A listener that returns [`HookError::Refuse`] stops the request: its
///   own edit is dropped, no later listener runs (on this thread or
///   another: nothing is queued), and the result names the refusal
///   ([`Altered::refused`], [`Collected::refused`]). It is not a failure
///   and does not reach the failure sinks.
/// - On a point declared fail-closed ([`PointSpec::fail_closed`]), a
///   listener that fails (error, panic, missed deadline) stops the
///   request the same way: the failure is reported as usual and the
///   result names a refusal with [`Refusal::failed`] set. A threaded
///   listener that misses its deadline stops it at once.
/// - On the other kinds, [`HookError::Refuse`] counts as a failure
///   ([`FailureKind::Error`] with the reason) and `fail_closed` is
///   ignored.
///
/// Capabilities: an implementation may offer more than this core, each
/// as a trait of its own found at run time through a provided method
/// that returns `None` by default ([`Bus::defer`], [`Bus::timings`],
/// [`Bus::journal`], [`Bus::relay`]). A library that needs
/// one names it in its contract table ([`PointDecl::needs`]);
/// [`declare_all`] then refuses the point on a bus without it
/// ([`Refused::Unsupported`]). A library that can do without checks the
/// method and falls back.
///
/// Local listeners ([`Bus::listen_local_any`]) are for state that is not
/// `Send` (`Rc`, `RefCell`), such as an interface's. Each is bound to the
/// thread that registered it:
///
/// - It takes its place in the same order as the other listeners of its
///   point (`after` / `before` / [`LAST`], priority, registration), keeps
///   its owner and deadline, and shows in introspection
///   ([`ListenerInfo::local`]).
/// - An emit on its thread runs it in that order, inline, with the
///   others.
/// - An emit on another thread runs every other listener in order and
///   queues the local ones for their thread; the result names them
///   ([`Altered::deferred`], [`Collected::deferred`]) and the wakers
///   ([`Bus::add_pending_waker`]) are called. The thread runs them when
///   it calls [`Bus::run_pending`], in their order, emits in the order
///   they were queued. A deferred alter's listeners edit a copy of the
///   value the emit returned; a deferred collect's listeners each fill a
///   fresh value. What they do is reported by [`Bus::run_pending`], not
///   to the emitter.
/// - A listener removed ([`Bus::unlisten`], [`Bus::forget`]), from any
///   thread, does not run again, queued or not.
pub trait Bus: Send + Sync {
    /// Declares a point: its description and deadline. Listeners may
    /// register before or after; declaring twice is refused.
    fn declare_point(&self, point: PointKey, spec: PointSpec) -> Result<(), Refused>;

    /// Registers a listener on a point under `id`. Refused, with nothing
    /// changed, when the id is taken on the point, when its constraints
    /// close a cycle, or when the point is registered with another type.
    fn listen_any(
        &self,
        point: PointKey,
        id: String,
        opts: ListenOpts,
        run: AnyListener,
    ) -> Result<(), Refused>;

    /// Registers a local listener on a point under `id`, bound to the
    /// calling thread: it runs only on that thread. Refused as
    /// [`Bus::listen_any`] is, and when `opts` asks for a thread of its
    /// own ([`Refused::Threaded`]).
    fn listen_local_any(
        &self,
        point: PointKey,
        id: String,
        opts: ListenOpts,
        run: AnyLocalListener,
    ) -> Result<(), Refused>;

    /// Runs the local listeners queued for the calling thread by emits
    /// on other threads, emit by emit in the order they were queued;
    /// what each emit's deferred part did. Empty when nothing waits.
    fn run_pending(&self) -> Vec<PendingRun>;

    /// Adds a waker called when an emit queues local listeners for a
    /// thread, with that thread's id: where the host asks that thread to
    /// call [`Bus::run_pending`] (a repaint request, a message to its
    /// loop). A waker that panics is ignored.
    fn add_pending_waker(&self, wake: PendingWaker);

    /// Removes the listener `id` of the point `point`; whether it was
    /// there.
    fn unlisten(&self, point: &str, id: &str) -> bool;

    /// Removes every listener of `owner`, on every point; how many.
    fn forget(&self, owner: &str) -> usize;

    /// Passes `value` through the point's listeners in order, each
    /// editing what the previous ones left; `clone` copies it so a
    /// failing listener leaves it as it found it.
    fn alter_any(
        &self,
        point: PointKey,
        input: AnyInput,
        value: AnyValue,
        clone: CloneFn,
    ) -> Altered<AnyValue>;

    /// Gives each of the point's listeners, in order, a value from
    /// `fresh` to fill, and gathers those that succeed.
    fn collect_any(&self, point: PointKey, input: AnyInput, fresh: FreshFn) -> Collected<AnyValue>;

    /// Adds a sink called with each failure that dropped a contribution,
    /// as it happens.
    fn add_failure_sink(&self, sink: FailureSink);

    /// Every point, in id order, with its listeners in the order they
    /// run.
    fn points(&self) -> Vec<PointInfo>;

    /// The listeners of the point `point`, in the order they run; `None`
    /// for a point nobody declared or listened to.
    fn listeners(&self, point: &str) -> Option<Vec<ListenerInfo>>;

    /// The [`Defer`] capability: requests asked now, heard at the next
    /// drain on the same thread. `None` (the default) when the
    /// implementation does not offer it.
    fn defer(&self) -> Option<&dyn Defer> {
        None
    }

    /// The [`Timings`] capability: how long listeners take, per point and
    /// listener, and passes over a point's budget reported. `None` (the
    /// default) when the implementation does not offer it.
    fn timings(&self) -> Option<&dyn Timings> {
        None
    }

    /// The [`Journal`] capability: events of journaled points kept with a
    /// number, read again from one, delivered after the publisher has
    /// moved on. `None` (the default) when the implementation does not
    /// offer it.
    fn journal(&self) -> Option<&dyn Journal> {
        None
    }

    /// The [`Relay`] capability: events of relayable points served to
    /// another process from a cursor and taken in from one, asks of
    /// relayable requests answered for one. `None` (the default) when
    /// the implementation does not offer it; a bus that offers it offers
    /// [`Bus::journal`].
    fn relay(&self) -> Option<&dyn Relay> {
        None
    }
}

/// Whether `bus` offers `capability`.
pub fn offers(bus: &dyn Bus, capability: Capability) -> bool {
    match capability {
        Capability::Defer => bus.defer().is_some(),
        Capability::Timings => bus.timings().is_some(),
        Capability::Journal => bus.journal().is_some(),
        Capability::Relay => bus.relay().is_some() && bus.journal().is_some(),
    }
}

/// A capability of a bus ([`Bus::timings`]): how long listeners take,
/// per point and per listener, and the passes over a point's budget
/// ([`PointSpec::budget`]) reported as they happen.
///
/// A pass is what the listeners of one emit ran on one thread: the
/// inline part of an alter or collect emit, one emit's local listeners
/// run by [`Bus::run_pending`], or one ask run by [`Defer::drain`]. Its
/// time is its listeners' times added up.
///
/// What an implementation promises:
///
/// - Every listener run it makes is timed, whatever its outcome (a
///   threaded listener that misses its deadline counts the time the
///   emit waited), and counted in the listener's and the pass's
///   [`TimingStats`] until [`Timings::reset`].
/// - A pass of a point with a budget that takes longer is reported to
///   the budget sinks, on the thread that ran it, after the pass, with
///   each listener's time ([`OverBudget`]). The pass's results are kept:
///   a budget is not a deadline, and a pass over it is not a failure.
/// - Quantiles may be approximate; counts, totals and maxima are exact.
/// - Timing costs little next to a listener: clock reads and counters,
///   no allocation per run.
pub trait Timings: Send + Sync {
    /// Every point that ran a pass since the last reset, in id order.
    fn points(&self) -> Vec<PointTimings>;

    /// The timings of the point `point`; `None` when it ran no pass
    /// since the last reset.
    fn point(&self, point: &str) -> Option<PointTimings>;

    /// Starts a new window: every count back to zero (after a start-up
    /// phase, before a measured run).
    fn reset(&self);

    /// Adds a sink called with each pass over its point's budget, on the
    /// thread that ran it: where the host publishes its
    /// [`OVER_BUDGET_FACT`] fact or logs. A sink that panics is ignored.
    fn add_budget_sink(&self, sink: BudgetSink);
}

impl<'a> dyn Timings + 'a {
    /// Adds a sink called with each pass over its point's budget
    /// ([`Timings::add_budget_sink`]).
    pub fn on_over_budget(&self, sink: impl Fn(&OverBudget) + Send + Sync + 'static) {
        self.add_budget_sink(Arc::new(sink));
    }
}

/// A capability of a bus ([`Bus::defer`]): a request asked now, whose
/// listeners run at the bus's next drain on the same thread.
///
/// It is for code that cannot be re-entered while it runs, such as an
/// interface drawing a panel with its state borrowed: the panel asks,
/// and the listeners run after the drawing, when the host calls
/// [`Defer::drain`].
///
/// What an implementation promises:
///
/// - An ask is queued for the calling thread and returns at once with
///   its number, increasing in ask order across the bus. The wakers
///   ([`Bus::add_pending_waker`]) are called with the thread's id.
/// - [`Defer::drain`] runs the asks queued for the calling thread, in
///   ask order, each as an alter emit of its point with the listeners
///   registered when it runs (one removed since the ask does not run;
///   one added since does). Every rule of [`Bus`] holds, refusals and
///   fail-closed points included; a local listener of another thread is
///   queued for that thread's [`Bus::run_pending`] as usual.
/// - Each ask's outcome is reported by the drain ([`PendingRun`], with
///   [`PendingRun::ask`] set): the listeners that ran, their failures,
///   a refusal. The asker gets nothing back; a listener placed last
///   ([`ListenOpts::last`]) reads the final value.
/// - An ask made during a drain (by a listener) waits for the next one.
/// - An immediate emit made after an ask, before the drain, runs before
///   it.
pub trait Defer: Send + Sync {
    /// Queues an alter emit of `point` with `input` and `value` for the
    /// calling thread's next [`Defer::drain`]; the ask's number.
    fn ask_any(&self, point: PointKey, input: AnyInput, value: AnyValue, clone: CloneFn) -> u64;

    /// Runs the asks queued for the calling thread, in ask order; what
    /// each did. Empty when nothing waits.
    fn drain(&self) -> Vec<PendingRun>;

    /// How many asks wait for the calling thread's drain (zero before a
    /// clean exit).
    fn waiting(&self) -> usize;
}

impl<'a> dyn Defer + 'a {
    /// Asks the request `P` now: its listeners run at the calling
    /// thread's next [`Defer::drain`]. The ask's number.
    pub fn ask<P: Point>(&self, input: P::Input, value: P::Value) -> u64
    where
        P::Value: Clone,
    {
        self.ask_any(
            PointKey::of::<P>(),
            Arc::new(input),
            Box::new(value),
            clone_value::<P::Value>,
        )
    }
}

/// A shared bus is a bus: `&Arc<dyn Bus>` goes where `&dyn Bus` is
/// asked, and an application may hand out `Arc<MyBus>` as it is.
impl<B: Bus + ?Sized> Bus for Arc<B> {
    fn declare_point(&self, point: PointKey, spec: PointSpec) -> Result<(), Refused> {
        (**self).declare_point(point, spec)
    }

    fn listen_any(
        &self,
        point: PointKey,
        id: String,
        opts: ListenOpts,
        run: AnyListener,
    ) -> Result<(), Refused> {
        (**self).listen_any(point, id, opts, run)
    }

    fn listen_local_any(
        &self,
        point: PointKey,
        id: String,
        opts: ListenOpts,
        run: AnyLocalListener,
    ) -> Result<(), Refused> {
        (**self).listen_local_any(point, id, opts, run)
    }

    fn run_pending(&self) -> Vec<PendingRun> {
        (**self).run_pending()
    }

    fn add_pending_waker(&self, wake: PendingWaker) {
        (**self).add_pending_waker(wake);
    }

    fn unlisten(&self, point: &str, id: &str) -> bool {
        (**self).unlisten(point, id)
    }

    fn forget(&self, owner: &str) -> usize {
        (**self).forget(owner)
    }

    fn alter_any(
        &self,
        point: PointKey,
        input: AnyInput,
        value: AnyValue,
        clone: CloneFn,
    ) -> Altered<AnyValue> {
        (**self).alter_any(point, input, value, clone)
    }

    fn collect_any(&self, point: PointKey, input: AnyInput, fresh: FreshFn) -> Collected<AnyValue> {
        (**self).collect_any(point, input, fresh)
    }

    fn add_failure_sink(&self, sink: FailureSink) {
        (**self).add_failure_sink(sink);
    }

    fn points(&self) -> Vec<PointInfo> {
        (**self).points()
    }

    fn listeners(&self, point: &str) -> Option<Vec<ListenerInfo>> {
        (**self).listeners(point)
    }

    fn defer(&self) -> Option<&dyn Defer> {
        (**self).defer()
    }

    fn timings(&self) -> Option<&dyn Timings> {
        (**self).timings()
    }

    fn journal(&self) -> Option<&dyn Journal> {
        (**self).journal()
    }

    fn relay(&self) -> Option<&dyn Relay> {
        (**self).relay()
    }
}

/// Copies an erased value of type `V`.
fn clone_value<V: Clone + Send + 'static>(value: &(dyn Any + Send)) -> AnyValue {
    match value.downcast_ref::<V>() {
        Some(v) => Box::new(v.clone()),
        None => panic!(
            "a bus implementation passed a value that is not a `{}`",
            type_name::<V>()
        ),
    }
}

/// A fresh erased value of type `V`.
fn fresh_value<V: Default + Send + 'static>() -> AnyValue {
    Box::new(V::default())
}

/// Takes the typed value back from what an implementation returned.
fn typed<P: Point>(value: AnyValue) -> P::Value {
    match value.downcast::<P::Value>() {
        Ok(v) => *v,
        Err(_) => panic!(
            "the bus returned a value of another type than `{}`'s ({})",
            P::ID,
            type_name::<P::Value>()
        ),
    }
}

impl<'a> dyn Bus + 'a {
    /// Declares the point `P`: its description and deadline. Listeners
    /// may register before or after; declaring twice is refused.
    pub fn declare<P: Point>(&self, spec: PointSpec) -> Result<(), Refused> {
        self.declare_point(PointKey::of::<P>(), spec)
    }

    /// Registers a listener on `P` under `id`. It reads the emit's input
    /// and edits the value; its edit counts only when it returns `Ok`
    /// within its deadline without panicking.
    ///
    /// Refused, with nothing changed, when the id is taken on the point
    /// or when its `after`/`before` close a cycle. A constraint naming a
    /// listener not (yet) registered is accepted, reported by each emit
    /// and by introspection, and ignored.
    pub fn listen<P: Point>(
        &self,
        id: impl Into<String>,
        opts: ListenOpts,
        run: impl Fn(&P::Input, &mut P::Value) -> Result<(), HookError> + Send + Sync + 'static,
    ) -> Result<(), Refused> {
        self.listen_any(PointKey::of::<P>(), id.into(), opts, erase::<P>(run))
    }

    /// Registers a local listener on `P` under `id`, bound to the calling
    /// thread: it may hold state that is not `Send` (`Rc<RefCell<…>>`).
    /// It takes its place in the point's order with the other listeners;
    /// an emit from another thread queues it for this thread's next
    /// [`Bus::run_pending`]. See [`Bus`] for the guarantees.
    pub fn listen_local<P: Point>(
        &self,
        id: impl Into<String>,
        opts: ListenOpts,
        run: impl Fn(&P::Input, &mut P::Value) -> Result<(), HookError> + 'static,
    ) -> Result<(), Refused> {
        self.listen_local_any(PointKey::of::<P>(), id.into(), opts, erase_local::<P>(run))
    }

    /// Adds a waker called, with the thread's id, when an emit queues
    /// local listeners for that thread. A waker that panics is ignored.
    pub fn on_pending(&self, wake: impl Fn(std::thread::ThreadId) + Send + Sync + 'static) {
        self.add_pending_waker(Arc::new(wake));
    }

    /// Passes `value` through `P`'s listeners in order: each edits the
    /// value the previous ones left. A listener that fails leaves it as
    /// it found it.
    pub fn alter<P: Point>(&self, input: P::Input, value: P::Value) -> Altered<P::Value>
    where
        P::Value: Clone,
    {
        let out = self.alter_any(
            PointKey::of::<P>(),
            Arc::new(input),
            Box::new(value),
            clone_value::<P::Value>,
        );
        Altered {
            value: typed::<P>(out.value),
            failures: out.failures,
            deferred: out.deferred,
            refused: out.refused,
        }
    }

    /// Gives each of `P`'s listeners, in order, a fresh default value to
    /// fill, and gathers the values of those that succeed.
    pub fn collect<P: Point>(&self, input: P::Input) -> Collected<P::Value>
    where
        P::Value: Default,
    {
        let out = self.collect_any(
            PointKey::of::<P>(),
            Arc::new(input),
            fresh_value::<P::Value>,
        );
        Collected {
            parts: out
                .parts
                .into_iter()
                .map(|(id, v)| (id, typed::<P>(v)))
                .collect(),
            failures: out.failures,
            deferred: out.deferred,
            refused: out.refused,
        }
    }

    /// Adds a sink called with each failure that dropped a contribution
    /// ([`Failure::dropped`]), as it happens: where the host publishes
    /// its [`FAILED_FACT`] fact or logs. A sink that panics is ignored.
    pub fn on_failure(&self, sink: impl Fn(&Failure) + Send + Sync + 'static) {
        self.add_failure_sink(Arc::new(sink));
    }
}

/// Erases a typed listener of `P`.
pub fn erase<P: Point>(
    run: impl Fn(&P::Input, &mut P::Value) -> Result<(), HookError> + Send + Sync + 'static,
) -> AnyListener {
    Arc::new(move |input, value| {
        let Some(input) = input.downcast_ref::<P::Input>() else {
            return Err(HookError::Failed(format!(
                "`{}`: the input is not a `{}`",
                P::ID,
                type_name::<P::Input>()
            )));
        };
        let Some(value) = value.downcast_mut::<P::Value>() else {
            return Err(HookError::Failed(format!(
                "`{}`: the value is not a `{}`",
                P::ID,
                type_name::<P::Value>()
            )));
        };
        run(input, value)
    })
}

/// Erases a typed local listener of `P`.
pub fn erase_local<P: Point>(
    run: impl Fn(&P::Input, &mut P::Value) -> Result<(), HookError> + 'static,
) -> AnyLocalListener {
    Rc::new(move |input, value| {
        let Some(input) = input.downcast_ref::<P::Input>() else {
            return Err(HookError::Failed(format!(
                "`{}`: the input is not a `{}`",
                P::ID,
                type_name::<P::Input>()
            )));
        };
        let Some(value) = value.downcast_mut::<P::Value>() else {
            return Err(HookError::Failed(format!(
                "`{}`: the value is not a `{}`",
                P::ID,
                type_name::<P::Value>()
            )));
        };
        run(input, value)
    })
}

/// One line of a library's contract table: a point it declares, built
/// from the point's type in a `const` (`PointDecl::of::<DocClose>(…)`),
/// so the application can read what a library will host before building
/// anything. [`declare_all`] declares a table on a bus.
#[derive(Clone, Copy, Debug)]
pub struct PointDecl {
    /// The point's id.
    pub id: &'static str,
    /// What it is for.
    pub kind: PointKind,
    /// What it does, for people and agents reading introspection.
    pub description: &'static str,
    /// The deadline of its listeners that set none; `None` takes the
    /// bus's.
    pub deadline: Option<Duration>,
    /// Whether a listener failure refuses the request
    /// ([`PointSpec::fail_closed`]).
    pub fail_closed: bool,
    /// The capabilities the point needs from the bus: [`declare_all`]
    /// refuses it on a bus without them.
    pub needs: &'static [Capability],
    /// The time one pass of its listeners should take
    /// ([`PointSpec::budget`]).
    pub budget: Option<Duration>,
    /// Its events go through the bus's journal ([`PointSpec::journaled`]).
    pub journaled: bool,
    /// How it crosses a process, when it is relayable
    /// ([`PointSpec::relay`]).
    pub relay: Option<Codec>,
    /// Its version, from 1 ([`PointSpec::version`]).
    pub version: u32,
    key: fn() -> PointKey,
}

impl PointDecl {
    /// The declaration of the point `P`, described by `description`.
    pub const fn of<P: Point>(description: &'static str) -> PointDecl {
        PointDecl {
            id: P::ID,
            kind: P::KIND,
            description,
            deadline: None,
            fail_closed: false,
            needs: &[],
            budget: None,
            journaled: false,
            relay: None,
            version: 1,
            key: PointKey::of::<P>,
        }
    }

    /// The deadline of its listeners that set none.
    pub const fn deadline(mut self, deadline: Duration) -> PointDecl {
        self.deadline = Some(deadline);
        self
    }

    /// A listener failure refuses the request
    /// ([`PointSpec::fail_closed`]).
    pub const fn fail_closed(mut self) -> PointDecl {
        self.fail_closed = true;
        self
    }

    /// The capabilities the point needs from the bus
    /// (`.needs(&[Capability::Defer])`).
    pub const fn needs(mut self, needs: &'static [Capability]) -> PointDecl {
        self.needs = needs;
        self
    }

    /// The time one pass of its listeners should take
    /// ([`PointSpec::budget`]); reported when over, never failed. It
    /// needs no capability: on a bus without [`Timings`] it is only
    /// shown.
    pub const fn budget(mut self, budget: Duration) -> PointDecl {
        self.budget = Some(budget);
        self
    }

    /// Its events go through the bus's journal: published with a number,
    /// delivered after the publisher has moved on ([`Journal`]). For an
    /// event point; [`declare_all`] refuses it on a bus without the
    /// journal ([`Refused::Unsupported`]) and on another kind
    /// ([`Refused::NotEvent`]).
    pub const fn journaled(mut self) -> PointDecl {
        self.journaled = true;
        self
    }

    /// The point crosses processes ([`Relay`]):
    ///
    /// - an event point's events, their input written and read by
    ///   `codec` ([`Codec::of::<P>()`](Codec::of)); journaled too;
    /// - a request point's asks, from another process, their input and
    ///   value written and read by `codec`
    ///   ([`Codec::request::<P>()`](Codec::request)); fail-closed too, so
    ///   no answer in time is a refusal.
    ///
    /// [`declare_all`] refuses it on a bus without the relay
    /// ([`Refused::Unsupported`]), on an alter or collect point and on a
    /// request whose codec has no value ([`Refused::NotCrossable`]), and
    /// with a codec of another point ([`Refused::TypeMismatch`]).
    pub const fn relayable(mut self, codec: Codec) -> PointDecl {
        match self.kind {
            PointKind::Event => self.journaled = true,
            PointKind::Request => self.fail_closed = true,
            _ => {}
        }
        self.relay = Some(codec);
        self
    }

    /// Its version, from 1 ([`PointSpec::version`]): raise it when the
    /// payload changes in a way an older reader cannot use. Two
    /// processes cross the point only at the same version.
    pub const fn version(mut self, version: u32) -> PointDecl {
        self.version = version;
        self
    }

    /// The capabilities the point needs: those it names
    /// ([`PointDecl::needs`]), then [`Capability::Journal`] when it is
    /// journaled and [`Capability::Relay`] when it is relayable.
    pub fn required(&self) -> Vec<Capability> {
        let mut all = self.needs.to_vec();
        if self.journaled && !all.contains(&Capability::Journal) {
            all.push(Capability::Journal);
        }
        if self.relay.is_some() && !all.contains(&Capability::Relay) {
            all.push(Capability::Relay);
        }
        all
    }

    /// The point's key.
    pub fn key(&self) -> PointKey {
        (self.key)()
    }

    /// The point's spec, as [`Bus::declare_point`] takes it.
    pub fn spec(&self) -> PointSpec {
        let mut spec = PointSpec::new(self.description);
        spec.deadline = self.deadline;
        spec.fail_closed = self.fail_closed;
        spec.budget = self.budget;
        spec.journaled = self.journaled;
        spec.relay = self.relay;
        spec.version = self.version;
        spec
    }
}
