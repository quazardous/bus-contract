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

/// What a point is for: declared, for introspection and review, not
/// enforced (a request is an alter emit over the request's input).
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PointSpec {
    /// What the point builds, for people and agents reading
    /// [`Bus::points`](crate::Bus::points).
    pub description: String,
    /// The deadline of the point's listeners that set none themselves;
    /// `None` takes the kernel's default.
    pub deadline: Option<Duration>,
}

impl PointSpec {
    /// A point described by `description`, with the kernel's deadline.
    pub fn new(description: impl Into<String>) -> PointSpec {
        PointSpec {
            description: description.into(),
            deadline: None,
        }
    }

    /// The deadline of the point's listeners that set none themselves.
    pub fn deadline(mut self, deadline: Duration) -> PointSpec {
        self.deadline = Some(deadline);
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
    /// returns.
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

/// The error a listener returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookError(pub String);

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HookError {}

impl From<String> for HookError {
    fn from(message: String) -> HookError {
        HookError(message)
    }
}

impl From<&str> for HookError {
    fn from(message: &str) -> HookError {
        HookError(message.to_owned())
    }
}

/// What went wrong with one listener during an emit.
#[derive(Clone, Debug, PartialEq, Eq)]
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
/// [`FailureKind::MissingTarget`]), the other listeners ran.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Whether the failure dropped a contribution: everything but a
    /// missing ordering target. Only these reach the failure sinks.
    pub fn dropped(&self) -> bool {
        !matches!(self.kind, FailureKind::MissingTarget(_))
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "hook `{}`, listener `{}`: ", self.point, self.listener)?;
        match &self.kind {
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

/// Why a declaration or a listener was refused. Nothing was changed.
#[derive(Clone, Debug, PartialEq, Eq)]
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
        }
    }
}

impl std::error::Error for Refused {}

/// The result of an [`alter`](crate::Bus::alter_any) emit.
#[derive(Debug)]
pub struct Altered<V> {
    /// The value after every listener that succeeded.
    pub value: V,
    /// Every failure, in listener order.
    pub failures: Vec<Failure>,
}

/// The result of a [`collect`](crate::Bus::collect_any) emit.
#[derive(Debug)]
pub struct Collected<V> {
    /// Each successful listener's value, in listener order, with its id.
    pub parts: Vec<(String, V)>,
    /// Every failure, in listener order.
    pub failures: Vec<Failure>,
}

/// A listener as introspection shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// The `after`/`before` targets that are not registered.
    pub missing: Vec<String>,
    /// How many times it ran.
    pub runs: u64,
    /// How many of those runs failed (contribution dropped).
    pub failures: u64,
    /// The last failure, as text.
    pub last_failure: Option<String>,
}

/// A point as introspection shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Its listeners, in the order they run.
    pub listeners: Vec<ListenerInfo>,
}
