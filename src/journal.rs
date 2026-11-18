//! The `Journal` and `Relay` capabilities: events kept with a sequence
//! number, read again from one, and served to another process.

use std::any::{Any, type_name};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::{AnyInput, FreshFn, Point, PointKey, Refused, fresh_value};

/// Writes an event's input as text, its type erased; `None` when the
/// input is not of the point's type.
pub type EncodeFn = fn(&(dyn Any + Send + Sync)) -> Option<String>;

/// Reads an event's input back from text, its type erased.
pub type DecodeFn = fn(&str) -> Result<AnyInput, String>;

/// An event point whose input can cross a process: written as text by
/// one bus, read back by another ([`Relay`]).
///
/// The format is the application's (JSON, most often), the same on both
/// ends. Two processes of different versions read each other: a decoder
/// should accept fields it does not know and fill the ones it misses,
/// and fail only on a payload it cannot use.
pub trait Relayed: Point {
    /// The input as text.
    fn encode(input: &Self::Input) -> String;
    /// The input read back from text, or why it cannot be.
    fn decode(text: &str) -> Result<Self::Input, String>;
}

/// How a relayable point's input is written and read
/// ([`Relayed`]), its types erased, for a contract table
/// ([`PointDecl::relayable`](crate::PointDecl::relayable)). Two codecs
/// are equal when they are for the same point.
#[derive(Clone, Copy, Debug)]
pub struct Codec {
    /// The point it is for ([`Point::ID`]): [`declare_all`](crate::declare_all)
    /// refuses a codec on another point ([`Refused::TypeMismatch`]).
    pub point: &'static str,
    /// Writes an input.
    pub encode: EncodeFn,
    /// Reads one back.
    pub decode: DecodeFn,
    /// Makes the value each listener fills when an event relayed in is
    /// delivered ([`Relay::ingest`]).
    pub fresh: FreshFn,
}

impl Codec {
    /// The codec of the point `P`.
    pub const fn of<P: Relayed>() -> Codec
    where
        P::Value: Default,
    {
        Codec {
            point: P::ID,
            encode: encode_erased::<P>,
            decode: decode_erased::<P>,
            fresh: fresh_value::<P::Value>,
        }
    }
}

impl PartialEq for Codec {
    fn eq(&self, other: &Codec) -> bool {
        self.point == other.point
    }
}

impl Eq for Codec {}

fn encode_erased<P: Relayed>(input: &(dyn Any + Send + Sync)) -> Option<String> {
    input.downcast_ref::<P::Input>().map(P::encode)
}

fn decode_erased<P: Relayed>(text: &str) -> Result<AnyInput, String> {
    P::decode(text).map(|input| Arc::new(input) as AnyInput)
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
    /// or, for an entry relayed in ([`JournalEntry::source`] set), in
    /// its source's.
    pub cause: Option<u64>,
    /// Where it was published: `None` here, else the source a
    /// [`Relay::ingest`] named (another process).
    pub source: Option<String>,
    /// Its input, typed, when this process holds it: published here, or
    /// relayed and decoded with its point's codec.
    pub input: Option<AnyInput>,
    /// Its input as text, when it has one: relayed in, or written by a
    /// codec.
    pub text: Option<String>,
}

impl JournalEntry {
    /// The entry `seq` of `kind`, with no cause, no source and no input.
    pub fn new(seq: u64, kind: impl Into<String>) -> JournalEntry {
        JournalEntry {
            seq,
            kind: kind.into(),
            cause: None,
            source: None,
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
            .field("source", &self.source)
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

/// A capability of a bus ([`Bus::journal`](crate::Bus::journal)): events
/// kept in order with a number, read again from a number, and delivered
/// to their point's listeners after the publisher has moved on.
///
/// An event point is journaled when its declaration says so
/// ([`PointDecl::journaled`](crate::PointDecl::journaled),
/// [`PointSpec::journaled`](crate::PointSpec::journaled)). Its events are
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
///   [`Bus`](crate::Bus) holds: order, deadlines, failures, local
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
    /// is not declared journaled on this bus ([`Refused::NotJournaled`])
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
        let entry = self.wait(&[P::ID], seq, timeout)?;
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

/// Where a reader of a relay stands: the journal run it read and the
/// last number it took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cursor {
    /// The journal's run ([`Relay::cursor`]); it changes when the
    /// journal starts again (the process restarted) and numbers restart
    /// with it. 0: the reader has read nothing yet.
    pub run: u64,
    /// The last number taken in that run.
    pub seq: u64,
}

impl Cursor {
    /// A reader that has read nothing: [`Relay::resume`] from it returns
    /// what the journal still keeps, with nothing lost.
    pub const START: Cursor = Cursor { run: 0, seq: 0 };
}

/// An event as it crosses a process ([`Relay`]): its input as text.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RelayedEntry {
    /// Its number in the journal it was read from.
    pub seq: u64,
    /// Its point's id.
    pub kind: String,
    /// The entry that caused it, in the journal it was read from.
    pub cause: Option<u64>,
    /// Its input, written by its point's codec.
    pub text: String,
}

impl RelayedEntry {
    /// The entry `seq` of `kind`, its input `text`, with no cause.
    pub fn new(seq: u64, kind: impl Into<String>, text: impl Into<String>) -> RelayedEntry {
        RelayedEntry {
            seq,
            kind: kind.into(),
            cause: None,
            text: text.into(),
        }
    }
}

/// Why a reader of a relay missed entries ([`Resumed::lost`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Lost {
    /// Entries after the reader's number were dropped from the journal's
    /// window.
    Gap,
    /// The journal started again (another run): the reader's number
    /// means nothing in it.
    Restarted,
}

/// What a relay read found ([`Relay::resume`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Resumed {
    /// The relayable entries after the reader's cursor, oldest first.
    pub entries: Vec<RelayedEntry>,
    /// Whether the reader missed entries before these: it rebuilds what
    /// it follows from the current state (a resync), then applies them.
    pub lost: Option<Lost>,
    /// Where the reader resumes next time.
    pub next: Cursor,
}

impl Resumed {
    /// A read that found `entries`, nothing lost, resuming at `next`.
    pub fn new(entries: Vec<RelayedEntry>, next: Cursor) -> Resumed {
        Resumed {
            entries,
            lost: None,
            next,
        }
    }
}

/// Why an entry relayed in was not taken ([`Relay::ingest`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum IngestError {
    /// Its point is declared here and its codec could not read the text
    /// (a sender of another version, a broken payload). Nothing was
    /// recorded.
    Decode {
        /// The point.
        kind: String,
        /// What the codec said.
        reason: String,
    },
    /// Its point is declared here but not relayable: a process may not
    /// publish it on this one. Nothing was recorded.
    NotRelayable {
        /// The point.
        kind: String,
    },
}

impl fmt::Display for IngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IngestError::Decode { kind, reason } => {
                write!(f, "relayed `{kind}`: cannot read its input: {reason}")
            }
            IngestError::NotRelayable { kind } => {
                write!(f, "relayed `{kind}`: the point is not relayable here")
            }
        }
    }
}

impl std::error::Error for IngestError {}

/// A capability of a bus ([`Bus::relay`](crate::Bus::relay)), on top of
/// its [`Journal`]: the events of relayable points served to another
/// process from a cursor, and the events another process serves taken
/// in as if published here.
///
/// A point is relayable when its declaration gives it a codec
/// ([`PointDecl::relayable`](crate::PointDecl::relayable)); only its
/// events cross. The transport (a socket, a pipe, an RPC) is the
/// application's: a relay reads and takes entries, it does not move
/// them.
///
/// What an implementation promises:
///
/// - A bus that offers `Relay` offers [`Journal`].
/// - [`Relay::resume`] returns the relayable entries after the cursor,
///   in number order, each written by its point's codec, and the cursor
///   to resume from. A cursor of another run returns the run's entries
///   from its start with [`Lost::Restarted`]; a cursor whose next
///   entries left the window returns the kept ones with [`Lost::Gap`].
///   [`Cursor::START`] loses nothing.
/// - [`Relay::ingest`] records a relayed entry in this journal with its
///   source: a point declared here is decoded with its codec and
///   delivered to its listeners like an entry published here; a kind
///   not declared here is kept with its text only and delivered to no
///   one. The cause is the sender's number, kept as it came.
/// - Entries taken in are not served again by [`Relay::resume`]: two
///   processes relaying to each other do not echo.
pub trait Relay: Send + Sync {
    /// Where a reader that starts now stands: this journal's run and its
    /// last number.
    fn cursor(&self) -> Cursor;

    /// The relayable entries after `from`, at most `limit`.
    fn resume(&self, from: Cursor, limit: usize) -> Resumed;

    /// Takes in an entry relayed from `source`; its number in this
    /// journal.
    fn ingest(&self, source: &str, entry: RelayedEntry) -> Result<u64, IngestError>;
}
