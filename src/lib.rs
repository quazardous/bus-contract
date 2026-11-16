#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod tools;
mod types;

use std::any::{Any, TypeId, type_name};
use std::sync::Arc;
use std::time::Duration;

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

/// Copies an erased value of a point's type (an alter keeps the value as
/// it was when a listener fails).
pub type CloneFn = fn(&(dyn Any + Send)) -> AnyValue;

/// Makes a fresh erased value of a point's type (each listener's part in
/// a collect).
pub type FreshFn = fn() -> AnyValue;

/// Receives each failure that dropped a contribution
/// ([`Failure::dropped`]).
pub type FailureSink = Arc<dyn Fn(&Failure) + Send + Sync>;

/// The bus contract: hook points, their listeners in a declared order,
/// emits and introspection. An application supplies one implementation
/// (its own, or one written for it, such as `egui-pop-hooks`'s `Kernel`)
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
            return Err(HookError(format!(
                "`{}`: the input is not a `{}`",
                P::ID,
                type_name::<P::Input>()
            )));
        };
        let Some(value) = value.downcast_mut::<P::Value>() else {
            return Err(HookError(format!(
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
            key: PointKey::of::<P>,
        }
    }

    /// The deadline of its listeners that set none.
    pub const fn deadline(mut self, deadline: Duration) -> PointDecl {
        self.deadline = Some(deadline);
        self
    }

    /// The point's key.
    pub fn key(&self) -> PointKey {
        (self.key)()
    }

    /// The point's spec, as [`Bus::declare_point`] takes it.
    pub fn spec(&self) -> PointSpec {
        let spec = PointSpec::new(self.description);
        match self.deadline {
            Some(d) => spec.deadline(d),
            None => spec,
        }
    }
}
