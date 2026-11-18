//! The contract's types, shared by every implementation.

use std::fmt;
use std::time::Duration;

/// The deadline of an in-process listener when neither the listener nor
/// its point sets one.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(1);

/// The deadline meant for listeners that answer from another process (a
/// plugin over RPC): give it with [`ListenOpts::deadline`] or
/// [`PointSpec::deadline`].
pub const PLUGIN_DEADLINE: Duration = Duration::from_secs(5);

/// The `after` target that places a listener after every other listener
/// of its point ([`ListenOpts::last`]): one per point, a second is
/// refused as a [`Refused::Cycle`] (each would run after the other).
pub const LAST: &str = "*";

/// The name a host gives the fact it publishes for each [`Failure`] its
/// sinks receive ([`Bus::add_failure_sink`](crate::Bus::add_failure_sink)).
pub const FAILED_FACT: &str = "hook.failed";

/// The name a host gives the fact it publishes for each [`OverBudget`]
/// its budget sinks receive
/// ([`Timings::add_budget_sink`](crate::Timings::add_budget_sink)).
pub const OVER_BUDGET_FACT: &str = "bus.overBudget";

/// A hook point: one type per point, so its input and value are checked
/// at compile time. Declared by the crate that owns the result.
///
/// Every listener of a point has the same shape: it reads the emit's
/// [`Input`](Point::Input) and edits a [`Value`](Point::Value). An
/// [`alter`](crate::Bus::alter_any) emit passes one value through the listeners
/// in order; a [`collect`](crate::Bus::collect_any) emit gives each listener a
/// fresh value and gathers them in a list.
pub trait Point: 'static {
    /// The point's id, dotted by convention (`health.report`).
    const ID: &'static str;
    /// What every listener reads: shared, never changed by a listener.
    type Input: Send + Sync + 'static;
    /// What a listener edits: the value altered in turn, or one
    /// listener's contribution to a collect.
    type Value: Send + 'static;
    /// What the point is for ([`PointKind`]); [`PointKind::Alter`] unless
    /// the point says otherwise.
    const KIND: PointKind = PointKind::Alter;
}

/// What a point is for, declared for introspection and review. A request
/// is an alter emit over the request's input; only a request can be
/// refused by a listener ([`HookError::Refuse`]) or declared fail-closed
/// ([`PointSpec::fail_closed`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PointKind {
    /// A request (`<noun>.<verb>`): its listeners do the work in their
    /// declared order; the value is what each did.
    Request,
    /// An event's listeners: each event delivered once, in journal order.
    Event,
    /// One value passed through the listeners in turn.
    #[default]
    Alter,
    /// Each listener adds its own part.
    Collect,
}

impl PointKind {
    /// The kind as introspection writes it (`request`).
    pub fn as_str(self) -> &'static str {
        match self {
            PointKind::Request => "request",
            PointKind::Event => "event",
            PointKind::Alter => "alter",
            PointKind::Collect => "collect",
        }
    }
}

/// A point's declaration: its description and limits, for introspection.
///
/// New fields may be added in a minor release: build one with
/// [`PointSpec::new`] and the builder methods.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PointSpec {
    /// What the point builds, for people and agents reading
    /// [`Bus::points`](crate::Bus::points).
    pub description: String,
    /// The deadline of the point's listeners that set none themselves;
    /// `None` takes the kernel's default.
    pub deadline: Option<Duration>,
    /// On a [`PointKind::Request`] point, a listener that fails (an
    /// error, a panic, a missed deadline) refuses the request instead of
    /// having its contribution dropped: the emit stops there
    /// ([`Refusal::failed`]). Ignored on the other kinds.
    pub fail_closed: bool,
    /// The time one pass of the point's listeners should take on a
    /// thread ([`Timings`](crate::Timings)): a pass over it is reported
    /// ([`OverBudget`]), never failed. `None`: no budget.
    pub budget: Option<Duration>,
}

impl PointSpec {
    /// A point described by `description`, with the kernel's deadline.
    pub fn new(description: impl Into<String>) -> PointSpec {
        PointSpec {
            description: description.into(),
            deadline: None,
            fail_closed: false,
            budget: None,
        }
    }

    /// The deadline of the point's listeners that set none themselves.
    pub fn deadline(mut self, deadline: Duration) -> PointSpec {
        self.deadline = Some(deadline);
        self
    }

    /// A listener failure refuses the request ([`PointSpec::fail_closed`]).
    pub fn fail_closed(mut self) -> PointSpec {
        self.fail_closed = true;
        self
    }

    /// The time one pass of the point's listeners should take
    /// ([`PointSpec::budget`]).
    pub fn budget(mut self, budget: Duration) -> PointSpec {
        self.budget = Some(budget);
        self
    }
}

/// How a listener is placed and limited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenOpts {
    /// Listeners this one runs after.
    pub after: Vec<String>,
    /// Listeners this one runs before.
    pub before: Vec<String>,
    /// Among listeners `after`/`before` leave free, higher runs first;
    /// ties run in registration order.
    pub priority: i32,
    /// Overrides the point's deadline.
    pub deadline: Option<Duration>,
    /// Who registered it (a module, a plugin), for [`Bus::forget`](crate::Bus::forget) and
    /// introspection.
    pub owner: Option<String>,
    /// Runs on a thread of its own, so the emit stops waiting at the
    /// deadline. An inline listener (the default) runs on the emitting
    /// thread: it cannot be stopped, and only counts as late once it
    /// returns. A local listener runs inline on its own thread: it is
    /// refused with this set ([`Refused::Threaded`]).
    pub threaded: bool,
}

impl ListenOpts {
    /// No constraint, priority 0, the point's deadline, inline.
    pub fn new() -> ListenOpts {
        ListenOpts::default()
    }

    /// Runs after the listener `id` (a missing one is reported and
    /// ignored).
    pub fn after(mut self, id: impl Into<String>) -> ListenOpts {
        self.after.push(id.into());
        self
    }

    /// Runs before the listener `id` (a missing one is reported and
    /// ignored).
    pub fn before(mut self, id: impl Into<String>) -> ListenOpts {
        self.before.push(id.into());
        self
    }

    /// Runs after every other listener of its point, whatever they
    /// declare: an invariants check, a listener that reads what all the
    /// others left. One per point: a second one closes a cycle and is
    /// refused ([`Refused::Cycle`]). Kept in [`ListenOpts::after`] as
    /// [`LAST`].
    pub fn last(mut self) -> ListenOpts {
        if !self.after.iter().any(|a| a == LAST) {
            self.after.push(LAST.to_owned());
        }
        self
    }

    /// Whether the listener runs after every other one of its point
    /// ([`ListenOpts::last`]).
    pub fn is_last(&self) -> bool {
        self.after.iter().any(|a| a == LAST)
    }

    /// Higher runs first among listeners left free by `after`/`before`.
    pub fn priority(mut self, priority: i32) -> ListenOpts {
        self.priority = priority;
        self
    }

    /// Overrides the point's deadline.
    pub fn deadline(mut self, deadline: Duration) -> ListenOpts {
        self.deadline = Some(deadline);
        self
    }

    /// Who registered the listener.
    pub fn owner(mut self, owner: impl Into<String>) -> ListenOpts {
        self.owner = Some(owner.into());
        self
    }

    /// Runs on a thread of its own, so a slow listener cannot hold the
    /// emit past its deadline.
    pub fn threaded(mut self) -> ListenOpts {
        self.threaded = true;
        self
    }
}

/// What a listener returns instead of `Ok(())`: it failed, or it refuses
/// the request.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookError {
    /// The listener failed: its contribution is dropped and the other
    /// listeners run, unless its point is fail-closed
    /// ([`PointSpec::fail_closed`]).
    Failed(String),
    /// The listener refuses the request, with a reason. On a
    /// [`PointKind::Request`] point the emit stops: no later listener
    /// runs, and the result names the refusal ([`Refusal`]). On another
    /// kind it counts as [`HookError::Failed`].
    Refuse(String),
}

impl HookError {
    /// A failure with `message`.
    pub fn failed(message: impl Into<String>) -> HookError {
        HookError::Failed(message.into())
    }

    /// A refusal of the request, with `reason`.
    pub fn refuse(reason: impl Into<String>) -> HookError {
        HookError::Refuse(reason.into())
    }

    /// The failure's message or the refusal's reason.
    pub fn message(&self) -> &str {
        match self {
            HookError::Failed(m) | HookError::Refuse(m) => m,
        }
    }

    /// Whether the listener refuses the request.
    pub fn is_refusal(&self) -> bool {
        matches!(self, HookError::Refuse(_))
    }
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookError::Failed(m) => f.write_str(m),
            HookError::Refuse(r) => write!(f, "refused: {r}"),
        }
    }
}

impl std::error::Error for HookError {}

/// A failure with this message.
impl From<String> for HookError {
    fn from(message: String) -> HookError {
        HookError::Failed(message)
    }
}

/// A failure with this message.
impl From<&str> for HookError {
    fn from(message: &str) -> HookError {
        HookError::Failed(message.to_owned())
    }
}

/// What went wrong with one listener during an emit.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailureKind {
    /// The listener returned an error.
    Error(String),
    /// The listener panicked; the panic's message.
    Panic(String),
    /// The listener missed its deadline. `took` is its time when it
    /// returned late (inline); `None` when the emit stopped waiting
    /// (threaded).
    Late {
        /// The deadline it had.
        deadline: Duration,
        /// How long it ran, when it returned.
        took: Option<Duration>,
    },
    /// An `after` or `before` constraint names a listener that is not
    /// registered. The listener still ran and its contribution counts;
    /// only the constraint was ignored. Not sent to the failure sinks.
    MissingTarget(String),
    /// The point id is registered with another [`Point`] type; no
    /// listener ran. `listener` is `*`.
    TypeMismatch,
}

/// One listener's failure: its contribution was dropped (except for a
/// [`FailureKind::MissingTarget`]); the other listeners ran, unless the
/// point is fail-closed ([`PointSpec::fail_closed`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Failure {
    /// The point emitted.
    pub point: &'static str,
    /// The listener's id.
    pub listener: String,
    /// The listener's owner.
    pub owner: Option<String>,
    /// What went wrong.
    pub kind: FailureKind,
}

impl Failure {
    /// The failure of the listener `listener` of `point`.
    pub fn new(
        point: &'static str,
        listener: impl Into<String>,
        owner: Option<String>,
        kind: FailureKind,
    ) -> Failure {
        Failure {
            point,
            listener: listener.into(),
            owner,
            kind,
        }
    }

    /// Whether the failure dropped a contribution: everything but a
    /// missing ordering target. Only these reach the failure sinks.
    pub fn dropped(&self) -> bool {
        !matches!(self.kind, FailureKind::MissingTarget(_))
    }
}

impl fmt::Display for FailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailureKind::Error(m) => write!(f, "failed: {m}"),
            FailureKind::Panic(m) => write!(f, "panicked: {m}"),
            FailureKind::Late {
                deadline,
                took: Some(t),
            } => write!(f, "took {t:?}, past its deadline of {deadline:?}"),
            FailureKind::Late { deadline, .. } => {
                write!(f, "no answer within its deadline of {deadline:?}")
            }
            FailureKind::MissingTarget(t) => {
                write!(f, "orders itself against `{t}`, which is not there")
            }
            FailureKind::TypeMismatch => write!(f, "the point is registered with another type"),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "hook `{}`, listener `{}`: {}",
            self.point, self.listener, self.kind
        )
    }
}

/// Why a declaration or a listener was refused. Nothing was changed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refused {
    /// The point already has a listener with this id.
    Duplicate {
        /// The point.
        point: &'static str,
        /// The id taken.
        listener: String,
    },
    /// The listener's `after`/`before` would close a cycle: these
    /// listeners, each to run before the next, the first again at the
    /// end.
    Cycle {
        /// The point.
        point: &'static str,
        /// The listeners in the cycle.
        path: Vec<String>,
    },
    /// The point id is already registered with another [`Point`] type.
    TypeMismatch {
        /// The point.
        point: &'static str,
    },
    /// The point was already declared.
    AlreadyDeclared {
        /// The point.
        point: &'static str,
    },
    /// A local listener asked for a thread of its own
    /// ([`ListenOpts::threaded`]); it runs on the thread that registered
    /// it.
    Threaded {
        /// The point.
        point: &'static str,
        /// The listener.
        listener: String,
    },
    /// The point needs a capability the bus does not offer
    /// ([`PointDecl::needs`](crate::PointDecl::needs)); it was not
    /// declared.
    Unsupported {
        /// The point.
        point: &'static str,
        /// The capability missing.
        capability: Capability,
    },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Duplicate { point, listener } => {
                write!(f, "hook `{point}` already has a listener `{listener}`")
            }
            Refused::Cycle { point, path } => {
                write!(
                    f,
                    "hook `{point}`: listeners order each other in a cycle: {}",
                    path.join(" -> ")
                )
            }
            Refused::TypeMismatch { point } => {
                write!(f, "hook `{point}` is registered with another type")
            }
            Refused::AlreadyDeclared { point } => write!(f, "hook `{point}` is already declared"),
            Refused::Threaded { point, listener } => write!(
                f,
                "hook `{point}`: the local listener `{listener}` runs on its own thread, not a new one"
            ),
            Refused::Unsupported { point, capability } => write!(
                f,
                "hook `{point}` needs the `{}` capability, which this bus does not offer",
                capability.as_str()
            ),
        }
    }
}

impl std::error::Error for Refused {}

/// A capability a bus may offer beyond the core of [`Bus`](crate::Bus),
/// found at run time ([`Bus::defer`](crate::Bus::defer)). A library names
/// the ones its points need in its contract table
/// ([`PointDecl::needs`](crate::PointDecl::needs)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Capability {
    /// Requests asked now and heard at the bus's next drain on the same
    /// thread ([`Defer`](crate::Defer)).
    Defer,
    /// How long listeners take, per point and listener, and passes over
    /// a point's budget reported ([`Timings`](crate::Timings)).
    Timings,
}

impl Capability {
    /// The capability as messages write it (`defer`).
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Defer => "defer",
            Capability::Timings => "timings",
        }
    }
}

/// A request stopped by one of its listeners: the listener refused it
/// ([`HookError::Refuse`]), or failed on a fail-closed point
/// ([`PointSpec::fail_closed`]). The listeners after it did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Refusal {
    /// The point emitted.
    pub point: &'static str,
    /// The listener that stopped the request.
    pub listener: String,
    /// Its owner.
    pub owner: Option<String>,
    /// The reason it gave, or its failure as text.
    pub reason: String,
    /// Whether the listener failed (its [`Failure`] is in the result's
    /// failures) rather than refused.
    pub failed: bool,
}

impl Refusal {
    /// The refusal of `point` by the listener `listener`, for `reason`.
    pub fn new(
        point: &'static str,
        listener: impl Into<String>,
        owner: Option<String>,
        reason: impl Into<String>,
    ) -> Refusal {
        Refusal {
            point,
            listener: listener.into(),
            owner,
            reason: reason.into(),
            failed: false,
        }
    }

    /// The refusal a listener's failure makes on a fail-closed point.
    pub fn from_failure(failure: &Failure) -> Refusal {
        let reason = match &failure.kind {
            FailureKind::Error(m) | FailureKind::Panic(m) => m.clone(),
            kind => kind.to_string(),
        };
        Refusal {
            point: failure.point,
            listener: failure.listener.clone(),
            owner: failure.owner.clone(),
            reason,
            failed: true,
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.failed {
            write!(
                f,
                "hook `{}`: refused, `{}` failed: {}",
                self.point, self.listener, self.reason
            )
        } else {
            write!(
                f,
                "hook `{}`: refused by `{}`: {}",
                self.point, self.listener, self.reason
            )
        }
    }
}

/// The result of an [`alter`](crate::Bus::alter_any) emit.
#[derive(Debug)]
#[non_exhaustive]
pub struct Altered<V> {
    /// The value after every listener that succeeded; when the request
    /// was refused, as the listeners before the refusal left it.
    pub value: V,
    /// Every failure, in listener order.
    pub failures: Vec<Failure>,
    /// The local listeners bound to another thread, in order: queued for
    /// that thread's [`run_pending`](crate::Bus::run_pending), not run
    /// by this emit. Empty when the request was refused.
    pub deferred: Vec<String>,
    /// The listener that stopped the request, if one did.
    pub refused: Option<Refusal>,
}

impl<V> Altered<V> {
    /// An emit that left `value`, with no failure, nothing deferred and
    /// no refusal.
    pub fn new(value: V) -> Altered<V> {
        Altered {
            value,
            failures: Vec::new(),
            deferred: Vec::new(),
            refused: None,
        }
    }
}

/// The result of a [`collect`](crate::Bus::collect_any) emit.
#[derive(Debug)]
#[non_exhaustive]
pub struct Collected<V> {
    /// Each successful listener's value, in listener order, with its id.
    pub parts: Vec<(String, V)>,
    /// Every failure, in listener order.
    pub failures: Vec<Failure>,
    /// The local listeners bound to another thread, in order: queued for
    /// that thread's [`run_pending`](crate::Bus::run_pending), not run
    /// by this emit. Empty when the request was refused.
    pub deferred: Vec<String>,
    /// The listener that stopped the request, if one did.
    pub refused: Option<Refusal>,
}

impl<V> Collected<V> {
    /// An emit with no part, no failure, nothing deferred and no
    /// refusal.
    pub fn new() -> Collected<V> {
        Collected {
            parts: Vec::new(),
            failures: Vec::new(),
            deferred: Vec::new(),
            refused: None,
        }
    }
}

impl<V> Default for Collected<V> {
    fn default() -> Collected<V> {
        Collected::new()
    }
}

/// What listeners run later did: one emit's local listeners queued for
/// their thread ([`Bus::run_pending`](crate::Bus::run_pending)), or one
/// deferred request ([`Defer::drain`](crate::Defer::drain)).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingRun {
    /// The point emitted.
    pub point: &'static str,
    /// The listeners that ran, in order (a listener removed since the
    /// emit is not run).
    pub ran: Vec<String>,
    /// Their failures, in listener order.
    pub failures: Vec<Failure>,
    /// The listener that stopped the request, if one did.
    pub refused: Option<Refusal>,
    /// For a deferred request, the number [`Defer::ask_any`](crate::Defer::ask_any)
    /// returned when it was asked; `None` for local listeners run by
    /// [`Bus::run_pending`](crate::Bus::run_pending).
    pub ask: Option<u64>,
}

impl PendingRun {
    /// A run of `point` where nothing ran yet.
    pub fn new(point: &'static str) -> PendingRun {
        PendingRun {
            point,
            ran: Vec::new(),
            failures: Vec::new(),
            refused: None,
            ask: None,
        }
    }
}

/// A listener as introspection shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListenerInfo {
    /// Its id.
    pub id: String,
    /// Its owner.
    pub owner: Option<String>,
    /// The listeners it runs after.
    pub after: Vec<String>,
    /// The listeners it runs before.
    pub before: Vec<String>,
    /// Its priority.
    pub priority: i32,
    /// The deadline it gets: its own, else its point's, else the kernel's.
    pub deadline: Duration,
    /// Whether it runs on a thread of its own.
    pub threaded: bool,
    /// Whether it is a local listener, bound to the thread that
    /// registered it.
    pub local: bool,
    /// The `after`/`before` targets that are not registered.
    pub missing: Vec<String>,
    /// How many times it ran.
    pub runs: u64,
    /// How many of those runs failed (contribution dropped).
    pub failures: u64,
    /// The last failure, as text.
    pub last_failure: Option<String>,
}

impl ListenerInfo {
    /// The listener `id` with no owner, no constraint, priority 0, the
    /// default deadline, inline, never run.
    pub fn new(id: impl Into<String>) -> ListenerInfo {
        ListenerInfo {
            id: id.into(),
            owner: None,
            after: Vec::new(),
            before: Vec::new(),
            priority: 0,
            deadline: DEFAULT_DEADLINE,
            threaded: false,
            local: false,
            missing: Vec::new(),
            runs: 0,
            failures: 0,
            last_failure: None,
        }
    }
}

/// A point as introspection shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PointInfo {
    /// Its id.
    pub id: &'static str,
    /// What it is for.
    pub kind: PointKind,
    /// Its description; empty until declared.
    pub description: String,
    /// Whether its owner declared it ([`Bus::declare_point`](crate::Bus::declare_point)); a point
    /// listened to before that is not.
    pub declared: bool,
    /// The deadline of its listeners that set none.
    pub deadline: Duration,
    /// Whether a listener failure refuses the request
    /// ([`PointSpec::fail_closed`]).
    pub fail_closed: bool,
    /// The time one pass of its listeners should take
    /// ([`PointSpec::budget`]).
    pub budget: Option<Duration>,
    /// Its listeners, in the order they run.
    pub listeners: Vec<ListenerInfo>,
}

impl PointInfo {
    /// The point `id` of `kind`, not declared, with the default deadline
    /// and no listener.
    pub fn new(id: &'static str, kind: PointKind) -> PointInfo {
        PointInfo {
            id,
            kind,
            description: String::new(),
            declared: false,
            deadline: DEFAULT_DEADLINE,
            fail_closed: false,
            budget: None,
            listeners: Vec::new(),
        }
    }
}

/// How long something took over a window of runs
/// ([`Timings`](crate::Timings)): a listener's runs, or a point's passes.
/// The quantiles may be approximate (an implementation may read them
/// from a histogram); the count, total and max are exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TimingStats {
    /// How many runs.
    pub count: u64,
    /// Their time added up.
    pub total: Duration,
    /// The longest.
    pub max: Duration,
    /// The median.
    pub p50: Duration,
    /// The 95th percentile.
    pub p95: Duration,
}

impl TimingStats {
    /// No run.
    pub fn new() -> TimingStats {
        TimingStats::default()
    }

    /// The mean time of a run; zero with no run.
    pub fn mean(&self) -> Duration {
        match u32::try_from(self.count) {
            Ok(0) => Duration::ZERO,
            Ok(n) => self.total / n,
            Err(_) => Duration::from_secs_f64(self.total.as_secs_f64() / self.count as f64),
        }
    }
}

/// One listener's time on a point ([`PointTimings::listeners`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListenerTimings {
    /// Its id.
    pub id: String,
    /// Its owner.
    pub owner: Option<String>,
    /// Its runs.
    pub stats: TimingStats,
}

impl ListenerTimings {
    /// The listener `id`, no run.
    pub fn new(id: impl Into<String>) -> ListenerTimings {
        ListenerTimings {
            id: id.into(),
            owner: None,
            stats: TimingStats::new(),
        }
    }
}

/// A point's time ([`Timings::points`](crate::Timings::points)): its
/// passes, its listeners, its budget and how often a pass went over.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PointTimings {
    /// The point.
    pub point: &'static str,
    /// Its budget ([`PointSpec::budget`]).
    pub budget: Option<Duration>,
    /// Its passes: each the time of the listeners one emit, queued run
    /// or drained ask ran on one thread.
    pub passes: TimingStats,
    /// How many passes went over the budget.
    pub over_budget: u64,
    /// Its listeners that ran, in the order they run.
    pub listeners: Vec<ListenerTimings>,
}

impl PointTimings {
    /// The point `point`, no budget, no run.
    pub fn new(point: &'static str) -> PointTimings {
        PointTimings {
            point,
            budget: None,
            passes: TimingStats::new(),
            over_budget: 0,
            listeners: Vec::new(),
        }
    }

    /// Its listeners, the slowest first by their 95th percentile, then
    /// by their max.
    pub fn slowest(&self) -> Vec<&ListenerTimings> {
        let mut out: Vec<&ListenerTimings> = self.listeners.iter().collect();
        out.sort_by(|a, b| {
            (b.stats.p95, b.stats.max)
                .cmp(&(a.stats.p95, a.stats.max))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }
}

/// A pass of a point's listeners that went over its budget
/// ([`PointSpec::budget`]): reported to the budget sinks
/// ([`Timings::add_budget_sink`](crate::Timings::add_budget_sink)). The
/// pass's results were kept: a budget is not a deadline.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OverBudget {
    /// The point.
    pub point: &'static str,
    /// The pass's time: its listeners' times added up.
    pub took: Duration,
    /// The point's budget.
    pub budget: Duration,
    /// Each listener that ran in the pass, in order, with its time.
    pub listeners: Vec<(String, Duration)>,
}

impl OverBudget {
    /// A pass of `point` that took `took` against `budget`.
    pub fn new(point: &'static str, took: Duration, budget: Duration) -> OverBudget {
        OverBudget {
            point,
            took,
            budget,
            listeners: Vec::new(),
        }
    }

    /// The listener that took longest in the pass, with its time.
    pub fn slowest(&self) -> Option<(&str, Duration)> {
        self.listeners
            .iter()
            .max_by_key(|(_, t)| *t)
            .map(|(id, t)| (id.as_str(), *t))
    }
}

impl fmt::Display for OverBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "hook `{}`: a pass took {:?}, over its budget of {:?}",
            self.point, self.took, self.budget
        )?;
        if let Some((id, t)) = self.slowest() {
            write!(f, " (slowest `{id}`: {t:?})")?;
        }
        Ok(())
    }
}
