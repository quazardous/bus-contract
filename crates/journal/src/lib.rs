#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::any::type_name;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use bus_contract::{
    AnyInput, Bus, Contract, Facet, FreshFn, Point, PointKey, PointKind, PointSpec, Refused,
    capability, fresh_value,
};

/// The journal contract: a bus offers it by answering for `dyn Journal`
/// in `Bus::capability`. A point that is journaled says so with the
/// [`JOURNALED`] facet, which needs this contract.
pub const CONTRACT: Contract = Contract::new::<dyn Journal>("journal", "0.2", &[]);

/// The data of the [`JOURNALED`] facet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Journaled;

/// The facet of a journaled event point: its events go through the
/// bus's journal, published with a number and delivered to the
/// listeners in journal order after the publisher has moved on
/// (`PointDecl::of::<P>(…).facets(&[JOURNALED])`, or
/// `PointSpec::new(…).facet(JOURNALED)`). Only an event point is
/// journaled ([`check`]).
pub const JOURNALED: Facet = Facet::new(&CONTRACT, &Journaled);

/// Whether `facets` (`PointSpec::facets`, `PointInfo::facets`) mark the
/// point journaled.
pub fn is_journaled(facets: &[Facet]) -> bool {
    Facet::find::<Journaled>(facets, &CONTRACT).is_some()
}

/// What the journal contract refuses in a declaration, for an
/// implementation's `declare_point`: a journaled point that is not an
/// event ([`Refused::Contract`]).
pub fn check(point: &PointKey, spec: &PointSpec) -> Result<(), Refused> {
    if is_journaled(&spec.facets) && point.kind != PointKind::Event {
        return Err(not_event(point.id));
    }
    Ok(())
}

/// The refusal of a journaled point that is not an event.
pub fn not_event(point: &'static str) -> Refused {
    Refused::Contract {
        point,
        contract: CONTRACT.name,
        reason: "only an event point is journaled".into(),
    }
}

/// The refusal of a publish on a point not declared journaled on this
/// bus ([`Journal::publish_any`]); nothing was recorded.
pub fn not_journaled(point: &'static str) -> Refused {
    Refused::Contract {
        point,
        contract: CONTRACT.name,
        reason: "the point is not declared journaled on this bus".into(),
    }
}

/// Who sent what crosses a process boundary: the far side of a link, as
/// the receiving side knows it.
///
/// The receiving side sets it from the link it read the entry or the ask
/// on (the link's own identity: a process, or an agent's session), never
/// from what the sender wrote: a sender cannot claim another origin.
/// Every entry taken in carries one ([`JournalEntry::origin`]); every ask
/// answered is checked against one (the crossing contract's `Relay::answer`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Origin {
    /// The process on the far side of the link (`node`, `plugin:geo`).
    pub process: String,
    /// The agent the link speaks for, when the link is an agent's
    /// session (the agent socket, an MCP client); `None` for a process.
    pub agent: Option<String>,
}

impl Origin {
    /// The process `name`, no agent.
    pub fn process(name: impl Into<String>) -> Origin {
        Origin {
            process: name.into(),
            agent: None,
        }
    }

    /// The same origin, speaking for the agent `name`.
    pub fn agent(mut self, name: impl Into<String>) -> Origin {
        self.agent = Some(name.into());
        self
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.agent {
            Some(agent) => write!(f, "{} (agent {agent})", self.process),
            None => f.write_str(&self.process),
        }
    }
}

/// Whether the event kind `kind` matches `pattern`: the same id, or a
/// prefix ending in `.*` (`vault.*` matches `vault.changed`), or `*` for
/// every kind. Every implementation reads patterns this way.
pub fn kind_matches(pattern: &str, kind: &str) -> bool {
    if pattern == "*" || pattern == kind {
        return true;
    }
    match pattern.strip_suffix(".*") {
        Some(prefix) => kind
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.')),
        None => false,
    }
}

/// One event of a journal: what happened, its number, what caused it.
#[derive(Clone)]
#[non_exhaustive]
pub struct JournalEntry {
    /// Its number in this journal: from 1, increasing by one, never
    /// reused while the process runs.
    pub seq: u64,
    /// Its kind: the event point's id, or a kind the journal keeps with
    /// no point (a fact recorded by the implementation, an event relayed
    /// from a process whose point this one does not declare).
    pub kind: String,
    /// The entry that caused it, if one did: a number in this journal,
    /// or, for an entry relayed in ([`JournalEntry::origin`] set), in
    /// its origin's.
    pub cause: Option<u64>,
    /// Where it was published: `None` here, else the origin
    /// the crossing contract's `Relay::ingest` was given (another process, an agent's
    /// session). Every entry taken in has one.
    pub origin: Option<Origin>,
    /// Its input, typed, when this process holds it: published here, or
    /// relayed and decoded with its point's codec.
    pub input: Option<AnyInput>,
    /// Its input as text, when it has one: relayed in, or written by a
    /// codec.
    pub text: Option<String>,
}

impl JournalEntry {
    /// The entry `seq` of `kind`, with no cause, no origin and no input.
    pub fn new(seq: u64, kind: impl Into<String>) -> JournalEntry {
        JournalEntry {
            seq,
            kind: kind.into(),
            cause: None,
            origin: None,
            input: None,
            text: None,
        }
    }

    /// The input of the event point `P`, when the entry is one of its
    /// events and holds it typed.
    pub fn read<P: Point>(&self) -> Option<&P::Input> {
        if self.kind != P::ID {
            return None;
        }
        self.input.as_deref()?.downcast_ref::<P::Input>()
    }
}

impl fmt::Debug for JournalEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalEntry")
            .field("seq", &self.seq)
            .field("kind", &self.kind)
            .field("cause", &self.cause)
            .field("origin", &self.origin)
            .field("input", &self.input.as_ref().map(|_| ".."))
            .field("text", &self.text)
            .finish()
    }
}

/// What a read of a journal after a number found ([`Journal::since`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Since {
    /// The entries after the number asked, oldest first.
    pub entries: Vec<JournalEntry>,
    /// Entries after the number asked were dropped before this read (a
    /// journal keeps a window): the reader missed them, and rebuilds
    /// what it follows from the current state.
    pub gap: bool,
    /// The journal's last number when it was read.
    pub last_seq: u64,
}

impl Since {
    /// A read that found `entries`, with no gap, the journal at
    /// `last_seq`.
    pub fn new(entries: Vec<JournalEntry>, last_seq: u64) -> Since {
        Since {
            entries,
            gap: false,
            last_seq,
        }
    }
}

/// A capability of a bus ([`BusJournal::journal`]): events
/// kept in order with a number, read again from a number, and delivered
/// to their point's listeners after the publisher has moved on.
///
/// An event point is journaled when its declaration says so
/// ([`JOURNALED`],
/// [`is_journaled`]). Its events are
/// published through the journal ([`Journal::publish_any`], typed
/// `publish::<P>`), not emitted.
///
/// What an implementation promises:
///
/// - Numbers start at 1 and increase by one with each entry; the
///   journal is ordered by them. An implementation may keep entries
///   that are not events of a point (its own facts, events relayed in
///   from a kind not declared here).
/// - Publishing never runs a listener and never waits on one: it
///   records the entry and returns its number.
/// - Each entry of a journaled point is delivered to the point's
///   listeners once, entries in number order, each as a collect emit of
///   the point with the entry's input (every rule of
///   [`Bus`] holds: order, deadlines, failures, local
///   listeners queued for their thread). Delivery runs on a thread of
///   the implementation's (a pump), so a slow listener delays the
///   following entries, never the publisher; deadlines bound it.
/// - The journal keeps a window: [`Journal::since`] reports a gap when
///   entries after the number asked were dropped.
pub trait Journal: Send + Sync {
    /// The number of the last entry; 0 before the first.
    fn last_seq(&self) -> u64;

    /// The entries after `seq`, oldest first, at most `limit`; a gap when
    /// some were dropped before this read.
    fn since(&self, seq: u64, limit: usize) -> Since;

    /// The first entry after `seq` whose kind matches one of `patterns`
    /// ([`kind_matches`]; empty matches every kind), waiting for it up
    /// to `timeout`; `None` when none came. An entry already in the
    /// journal is returned at once.
    fn wait(&self, patterns: &[&str], seq: u64, timeout: Duration) -> Option<JournalEntry>;

    /// Records an event of the journaled point `point`, caused by the
    /// entry `cause`; its number. `fresh` makes the value each listener
    /// fills at delivery. Refused, with nothing recorded, when the point
    /// is not declared journaled on this bus ([`not_journaled`])
    /// or is declared with another type ([`Refused::TypeMismatch`]).
    fn publish_any(
        &self,
        point: PointKey,
        input: AnyInput,
        fresh: FreshFn,
        cause: Option<u64>,
    ) -> Result<u64, Refused>;

    /// Waits until every entry recorded before the call has been
    /// delivered to its listeners, up to `timeout`; whether it was. For
    /// a host before it exits and for tests.
    fn flush(&self, timeout: Duration) -> bool;
}

impl<'a> dyn Journal + 'a {
    /// Publishes an event of the journaled point `P`: recorded now,
    /// delivered to its listeners later, in journal order. Its number.
    pub fn publish<P: Point>(&self, input: P::Input) -> Result<u64, Refused>
    where
        P::Value: Default,
    {
        self.publish_any(
            PointKey::of::<P>(),
            Arc::new(input),
            fresh_value::<P::Value>,
            None,
        )
    }

    /// Publishes an event of `P` caused by the entry `cause`.
    pub fn publish_caused<P: Point>(&self, input: P::Input, cause: u64) -> Result<u64, Refused>
    where
        P::Value: Default,
    {
        self.publish_any(
            PointKey::of::<P>(),
            Arc::new(input),
            fresh_value::<P::Value>,
            Some(cause),
        )
    }

    /// The next event of `P` after `seq`, typed, waiting up to
    /// `timeout`.
    pub fn wait_for<P: Point>(&self, seq: u64, timeout: Duration) -> Option<(u64, P::Input)>
    where
        P::Input: Clone,
    {
        let entry = self.wait(&[P::ID.as_str()], seq, timeout)?;
        let input = entry.read::<P>().cloned().unwrap_or_else(|| {
            panic!(
                "the journal returned a `{}` entry with no `{}` input",
                P::ID,
                type_name::<P::Input>()
            )
        });
        Some((entry.seq, input))
    }
}

/// Reaches the [`Journal`] capability of any bus: `bus.journal()`.
///
/// Implemented for every [`Bus`], sized or not (`dyn Bus`, `Arc<dyn
/// Bus>`, a concrete bus): what the bus answers for `dyn Journal`
/// through the typed lookup `Bus::capability`.
pub trait BusJournal {
    /// The [`Journal`] capability of this bus; `None` when the bus does
    /// not offer it.
    fn journal(&self) -> Option<&dyn Journal>;
}

impl<B: Bus + ?Sized> BusJournal for B {
    fn journal(&self) -> Option<&dyn Journal> {
        capability::<dyn Journal, B>(self)
    }
}
