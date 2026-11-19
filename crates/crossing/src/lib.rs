#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bus_contract::Bus;

/// The items of the crossing contract. In this release they are the
/// `bus-contract` core's own items, re-exported: the same types, so code
/// importing them from here and code importing them from the core work
/// together.
pub use bus_contract::{
    Answer, Authority, Codec, Crossing, Cursor, DecodeFn, DecodeValueFn, EncodeFn, EncodeValueFn,
    IngestError, Lost, Origin, Relay, Relayed, RelayedAnswer, RelayedAsk, RelayedEntry,
    RelayedRequest, Resumed, ask_run, far_listener,
};

/// The journal the relay sits on: a bus that offers [`Relay`] offers
/// [`Journal`] too.
pub use bus_contract_journal::{BusJournal, Journal, JournalEntry, Since};

/// Reaches the [`Relay`] capability of any bus: `bus.relay()`.
///
/// Implemented for every [`Bus`], sized or not (`dyn Bus`, `Arc<dyn
/// Bus>`, a concrete bus). It answers what the bus answers for
/// `dyn Relay`: in this release, through the core's `Bus::relay` getter,
/// which defaults to the typed lookup `Bus::capability`. From
/// `bus-contract` 0.5 the getter leaves the core and this trait is the
/// way to reach the capability, through the lookup only.
pub trait BusCrossing {
    /// The [`Relay`] capability of this bus; `None` when the bus does not
    /// offer it.
    fn relay(&self) -> Option<&dyn Relay>;
}

impl<B: Bus + ?Sized> BusCrossing for B {
    fn relay(&self) -> Option<&dyn Relay> {
        Bus::relay(self)
    }
}
