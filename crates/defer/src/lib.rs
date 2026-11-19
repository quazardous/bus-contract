#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::sync::Arc;

use bus_contract::{
    AnyInput, AnyValue, Bus, CloneFn, Contract, Point, PointKey, capability, clone_value,
};
#[allow(unused_imports)] // named by the docs
use bus_contract::{ListenOpts, PendingRun};

/// The defer contract: a point that needs it names it in its table
/// (`.needs(&[&bus_contract_defer::CONTRACT])`); a bus offers it by
/// answering for `dyn Defer` in `Bus::capability`.
pub const CONTRACT: Contract = Contract::new::<dyn Defer>("defer", "0.2", &[]);

/// A capability of a bus ([`BusDefer::defer`]): a request asked now, whose
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

/// Reaches the [`Defer`] capability of any bus: `bus.defer()`.
///
/// Implemented for every [`Bus`], sized or not (`dyn Bus`, `Arc<dyn
/// Bus>`, a concrete bus): what the bus answers for `dyn Defer` through
/// the typed lookup `Bus::capability`.
pub trait BusDefer {
    /// The [`Defer`] capability of this bus; `None` when the bus does not
    /// offer it.
    fn defer(&self) -> Option<&dyn Defer>;
}

impl<B: Bus + ?Sized> BusDefer for B {
    fn defer(&self) -> Option<&dyn Defer> {
        capability::<dyn Defer, B>(self)
    }
}
