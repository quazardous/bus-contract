//! The `Journal` and `Relay` capabilities: events kept with a sequence
//! number, read again from one, and served to another process; requests
//! answered on the other side of a process boundary (crossing).

use std::any::{Any, type_name};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::{
    AnyInput, AnyListener, CloneFn, FreshFn, HookError, Point, PointKey, Refused, clone_value,
    fresh_value,
};

/// Writes an event's input as text, its type erased; `None` when the
/// input is not of the point's type.
pub type EncodeFn = fn(&(dyn Any + Send + Sync)) -> Option<String>;

/// Reads an event's input back from text, its type erased.
pub type DecodeFn = fn(&str) -> Result<AnyInput, String>;

/// Writes a request's value as text, its type erased; `None` when the
/// value is not of the point's type.
pub type EncodeValueFn = fn(&(dyn Any + Send)) -> Option<String>;

/// Reads a request's value back from text into a value of the point's
/// type, its type erased, replacing it.
pub type DecodeValueFn = fn(&str, &mut (dyn Any + Send)) -> Result<(), String>;

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

/// A request point that can be answered by another process
/// ([`Relay::answer`]): its value, as well as its input, written as text
/// and read back.
///
/// The value goes out with the ask (what the asking side's listeners
/// left before the far side's turn) and comes back with the answer
/// (what the far side's listeners left). The same rules as
/// [`Relayed`] hold for versions: accept unknown fields, fail only on a
/// value that cannot be used.
pub trait RelayedRequest: Relayed {
    /// The value as text.
    fn encode_value(value: &Self::Value) -> String;
    /// The value read back from text, or why it cannot be.
    fn decode_value(text: &str) -> Result<Self::Value, String>;
}

/// How a relayable point's input is written and read
/// ([`Relayed`]), and a relayable request's value ([`RelayedRequest`]),
/// its types erased, for a contract table
/// ([`PointDecl::relayable`](crate::PointDecl::relayable)). Two codecs
/// are equal when they are for the same point.
///
/// Build one with [`Codec::of`] (an event) or [`Codec::request`] (a
/// request).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Codec {
    /// The point it is for ([`Point::ID`]): [`declare_all`](crate::declare_all)
    /// refuses a codec on another point ([`Refused::TypeMismatch`]).
    pub point: &'static str,
    /// Writes an input.
    pub encode: EncodeFn,
    /// Reads one back.
    pub decode: DecodeFn,
    /// Makes the value each listener fills when an event relayed in is
    /// delivered ([`Relay::ingest`]), and the value a request answered
    /// here starts from before its text is read into it.
    pub fresh: FreshFn,
    /// On a request's codec, writes its value; `None` on an event's.
    pub encode_value: Option<EncodeValueFn>,
    /// On a request's codec, reads its value back; `None` on an event's.
    pub decode_value: Option<DecodeValueFn>,
    /// On a request's codec, copies its value (the far side runs an
    /// alter emit, [`Bus::alter_any`](crate::Bus::alter_any)); `None` on
    /// an event's.
    pub clone_value: Option<CloneFn>,
}

impl Codec {
    /// The codec of the event point `P`.
    pub const fn of<P: Relayed>() -> Codec
    where
        P::Value: Default,
    {
        Codec {
            point: P::ID,
            encode: encode_erased::<P>,
            decode: decode_erased::<P>,
            fresh: fresh_value::<P::Value>,
            encode_value: None,
            decode_value: None,
            clone_value: None,
        }
    }

    /// The codec of the request point `P`: its input and its value.
    pub const fn request<P: RelayedRequest>() -> Codec
    where
        P::Value: Default + Clone,
    {
        Codec {
            point: P::ID,
            encode: encode_erased::<P>,
            decode: decode_erased::<P>,
            fresh: fresh_value::<P::Value>,
            encode_value: Some(encode_value_erased::<P>),
            decode_value: Some(decode_value_erased::<P>),
            clone_value: Some(clone_value::<P::Value>),
        }
    }

    /// Whether it writes and reads a value too ([`Codec::request`]): a
    /// request point needs one to cross.
    pub const fn answers(&self) -> bool {
        self.encode_value.is_some() && self.decode_value.is_some() && self.clone_value.is_some()
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

fn encode_value_erased<P: RelayedRequest>(value: &(dyn Any + Send)) -> Option<String> {
    value.downcast_ref::<P::Value>().map(P::encode_value)
}

fn decode_value_erased<P: RelayedRequest>(
    text: &str,
    into: &mut (dyn Any + Send),
) -> Result<(), String> {
    let Some(slot) = into.downcast_mut::<P::Value>() else {
        return Err(format!("the value is not a `{}`", type_name::<P::Value>()));
    };
    *slot = P::decode_value(text)?;
    Ok(())
}

/// Who sent what crosses a process boundary: the far side of a link, as
/// the receiving side knows it.
///
/// The receiving side sets it from the link it read the entry or the ask
/// on (the link's own identity: a process, or an agent's session), never
/// from what the sender wrote: a sender cannot claim another origin.
/// Every entry taken in carries one ([`JournalEntry::origin`]); every ask
/// answered is checked against one ([`Relay::answer`]).
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
    /// [`Relay::ingest`] was given (another process, an agent's
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
    /// Its point's version on the sending side
    /// ([`PointDecl::version`](crate::PointDecl::version)): a receiver
    /// that declares the point with another version refuses it
    /// ([`IngestError::Version`]).
    pub version: u32,
    /// Its input, written by its point's codec.
    pub text: String,
}

impl RelayedEntry {
    /// The entry `seq` of `kind`, its input `text`, with no cause, at
    /// version 1.
    pub fn new(seq: u64, kind: impl Into<String>, text: impl Into<String>) -> RelayedEntry {
        RelayedEntry {
            seq,
            kind: kind.into(),
            cause: None,
            version: 1,
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
    /// An authority of this bus refused it ([`Relay::add_authority`]).
    /// Nothing was recorded.
    Refused {
        /// The point.
        kind: String,
        /// What the authority said.
        reason: String,
    },
    /// Its point is declared here with another version: the sender and
    /// this process do not mean the same payload by it. Nothing was
    /// recorded.
    Version {
        /// The point.
        kind: String,
        /// The sender's version.
        sent: u32,
        /// This process's version.
        here: u32,
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
            IngestError::Refused { kind, reason } => {
                write!(f, "relayed `{kind}`: refused: {reason}")
            }
            IngestError::Version { kind, sent, here } => write!(
                f,
                "relayed `{kind}`: sent at version {sent}, declared here at version {here}"
            ),
        }
    }
}

impl std::error::Error for IngestError {}

/// What an authority of a bus is shown of something crossing in
/// ([`Relay::add_authority`]), before anything is decoded or recorded.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Crossing<'a> {
    /// Who sent it: the link it came on.
    pub origin: &'a Origin,
    /// Its point's id (or a kind this process does not declare).
    pub kind: &'a str,
    /// Its point's version on the sending side.
    pub version: u32,
    /// An ask to answer ([`Relay::answer`]) rather than an event taken in
    /// ([`Relay::ingest`]).
    pub ask: bool,
}

impl<'a> Crossing<'a> {
    /// An event of `kind` at `version` from `origin`, or an ask when
    /// `ask` is set.
    pub fn new(origin: &'a Origin, kind: &'a str, version: u32, ask: bool) -> Crossing<'a> {
        Crossing {
            origin,
            kind,
            version,
            ask,
        }
    }
}

/// Decides whether what crosses in may be taken ([`Relay::add_authority`]):
/// `Err(reason)` refuses it.
pub type Authority = Arc<dyn Fn(&Crossing<'_>) -> Result<(), String> + Send + Sync>;

/// A request asked of another process ([`Relay::answer`]): its input and
/// the value so far, as text, with the time the asker waits.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RelayedAsk {
    /// The asking process's run: it changes when that process starts
    /// again. With [`RelayedAsk::id`], names the ask.
    pub run: u64,
    /// The ask's number in its run; never reused while the run lasts.
    pub id: u64,
    /// Its point's id.
    pub kind: String,
    /// Its point's version on the asking side.
    pub version: u32,
    /// How long the asker waits for the answer, counted from when it
    /// sent the ask: the far side counts it from when it got the ask
    /// (clocks of two machines are not compared). Past it, both sides
    /// hold the ask refused.
    pub deadline: Duration,
    /// Its input, written by its point's codec.
    pub input: String,
    /// The value the asking side's listeners left before the far side's
    /// turn, written by its point's codec.
    pub value: String,
}

impl RelayedAsk {
    /// The ask `id` of the run `run` on `kind`, at version 1.
    pub fn new(
        run: u64,
        id: u64,
        kind: impl Into<String>,
        deadline: Duration,
        input: impl Into<String>,
        value: impl Into<String>,
    ) -> RelayedAsk {
        RelayedAsk {
            run,
            id,
            kind: kind.into(),
            version: 1,
            deadline,
            input: input.into(),
            value: value.into(),
        }
    }
}

/// How an ask was answered ([`RelayedAnswer::answer`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Answer {
    /// The far side's listeners ran and none refused: the value they
    /// left, as text.
    Done {
        /// The value, written by the point's codec.
        value: String,
    },
    /// The ask was refused: by a listener of the far side (a veto, or a
    /// failure: the point is fail-closed), or by the far bus itself (an
    /// authority, a point it does not declare, relayable or at this
    /// version, an input it cannot read, the deadline gone).
    Refused {
        /// The far listener that refused it; `None` when the far bus
        /// refused it before any listener ran.
        listener: Option<String>,
        /// That listener's owner.
        owner: Option<String>,
        /// Why: the listener's reason, or the bus's.
        reason: String,
        /// Whether a listener failed rather than refused
        /// ([`Refusal::failed`](crate::Refusal::failed)).
        failed: bool,
    },
}

impl Answer {
    /// A refusal by the far bus itself, before any listener ran.
    pub fn refused(reason: impl Into<String>) -> Answer {
        Answer::Refused {
            listener: None,
            owner: None,
            reason: reason.into(),
            failed: false,
        }
    }
}

/// The answer to one ask ([`Relay::answer`]), naming it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RelayedAnswer {
    /// The ask's run ([`RelayedAsk::run`]).
    pub run: u64,
    /// The ask's number ([`RelayedAsk::id`]).
    pub id: u64,
    /// What it got.
    pub answer: Answer,
}

impl RelayedAnswer {
    /// The answer `answer` to `ask`.
    pub fn to(ask: &RelayedAsk, answer: Answer) -> RelayedAnswer {
        RelayedAnswer {
            run: ask.run,
            id: ask.id,
            answer,
        }
    }
}

/// A capability of a bus ([`Bus::relay`](crate::Bus::relay)), on top of
/// its [`Journal`]: what crosses a process boundary. The events of
/// relayable points served to another process from a cursor and taken
/// in from one as if published here; the asks of relayable request
/// points answered here for another process.
///
/// A point is relayable when its declaration gives it a codec
/// ([`PointDecl::relayable`](crate::PointDecl::relayable)); only events
/// and requests cross. The transport (a socket, a pipe, an RPC, a
/// channel in the same process) is not part of the contract: a relay
/// reads, takes in and answers; something else moves the text. Whatever
/// moves it, the meaning is the one written here, so a listener cannot
/// tell whether the far side is in this process or another.
///
/// What an implementation promises:
///
/// - A bus that offers `Relay` offers [`Journal`].
/// - [`Relay::resume`] returns the relayable entries after the cursor,
///   in number order, each written by its point's codec with its point's
///   version, and the cursor to resume from. A cursor of another run
///   returns the run's entries from its start with [`Lost::Restarted`];
///   a cursor whose next entries left the window returns the kept ones
///   with [`Lost::Gap`]. [`Cursor::START`] loses nothing.
/// - Authority first: whatever comes in ([`Relay::ingest`],
///   [`Relay::answer`]) is shown to every authority added
///   ([`Relay::add_authority`]), in the order they were added, before
///   anything is decoded, recorded or run. One refusal refuses it. With
///   no authority added, everything relayable is taken.
/// - [`Relay::ingest`] records a relayed entry in this journal with its
///   origin: a point declared here is decoded with its codec and
///   delivered to its listeners like an entry published here; a kind
///   not declared here is kept with its text only and delivered to no
///   one. The cause is the sender's number, kept as it came. A point
///   declared here but not relayable, or at another version, is refused.
/// - Entries taken in are not served again by [`Relay::resume`]: two
///   processes relaying to each other do not echo.
/// - [`Relay::answer`] runs a relayable request's listeners here, as an
///   alter emit of the point with the ask's input and value read by its
///   codec, and answers with the value they left or the refusal. Every
///   rule of [`Bus`](crate::Bus) holds; a relayable request point is
///   fail-closed. It refuses, running no listener, an ask on a point not
///   declared here, not relayable, at another version, whose input or
///   value it cannot read, or whose deadline has passed when it would
///   run. When the listeners end past the ask's deadline, the answer is
///   a refusal (failed) whatever they did: the asker has stopped
///   waiting and holds it refused, and both sides record the same.
/// - Each ask is answered once: an ask whose run and number this bus has
///   already answered for the same origin process runs no listener; it
///   gets the first answer again while the bus still keeps it (at least
///   as long as the ask's deadline), else a refusal.
pub trait Relay: Send + Sync {
    /// Where a reader that starts now stands: this journal's run and its
    /// last number.
    fn cursor(&self) -> Cursor;

    /// The relayable entries after `from`, at most `limit`.
    fn resume(&self, from: Cursor, limit: usize) -> Resumed;

    /// Takes in an entry relayed from `origin`; its number in this
    /// journal.
    fn ingest(&self, origin: &Origin, entry: RelayedEntry) -> Result<u64, IngestError>;

    /// Answers an ask relayed from `origin`: runs the request's listeners
    /// here and says what they did. Blocks while they run.
    fn answer(&self, origin: &Origin, ask: RelayedAsk) -> RelayedAnswer;

    /// Adds an authority shown everything that crosses in before it is
    /// taken: an event to ingest, an ask to answer. An authority that
    /// panics refuses.
    fn add_authority(&self, authority: Authority);
}

impl<'a> dyn Relay + 'a {
    /// Adds an authority ([`Relay::add_authority`]): `Err(reason)`
    /// refuses what crosses in.
    pub fn authorize(
        &self,
        authority: impl Fn(&Crossing<'_>) -> Result<(), String> + Send + Sync + 'static,
    ) {
        self.add_authority(Arc::new(authority));
    }
}

/// This process's run, for the asks it sends ([`RelayedAsk::run`]):
/// fixed while the process runs, another one after a restart.
pub fn ask_run() -> u64 {
    static RUN: OnceLock<u64> = OnceLock::new();
    *RUN.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        (nanos ^ (u64::from(std::process::id()) << 32)).max(1)
    })
}

/// The next ask number of this process ([`RelayedAsk::id`]).
fn next_ask_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The listener that stands, on the asking side, for the far side of a
/// relayable request point: register it on the point here (with the
/// point's deadline and threaded, so the emit stops waiting at the
/// deadline), and the far side's listeners take their turn in this
/// side's order, at its place.
///
/// When it runs, it writes the input and the value so far with `codec`
/// ([`Codec::request`]), numbers the ask ([`ask_run`], a number of this
/// process), and hands it to `send`, which moves it to the far side's
/// [`Relay::answer`] and returns the answer, or `None` when none came
/// within [`RelayedAsk::deadline`] (a link down, a far side gone, too
/// slow). Then:
///
/// - an answer [`Answer::Done`] in time replaces the value with the far
///   side's;
/// - no answer, an answer after `deadline`, an answer to another ask, or
///   a value it cannot read refuses the request ("no answer is a no");
/// - an [`Answer::Refused`] refuses the request with the far side's
///   reason, naming the far listener.
///
/// `send` is the transport's part; this listener is the contract's, so
/// every transport gives a crossing request the same meaning.
pub fn far_listener(
    codec: Codec,
    deadline: Duration,
    version: u32,
    send: impl Fn(RelayedAsk) -> Option<RelayedAnswer> + Send + Sync + 'static,
) -> AnyListener {
    Arc::new(move |input, value| {
        let kind = codec.point;
        let (Some(encode_value), Some(decode_value)) = (codec.encode_value, codec.decode_value)
        else {
            return Err(HookError::refuse(format!(
                "`{kind}` has no request codec: it cannot cross"
            )));
        };
        let Some(input) = (codec.encode)(input) else {
            return Err(HookError::refuse(format!(
                "`{kind}`: the input is not the point's"
            )));
        };
        let Some(text) = encode_value(&*value) else {
            return Err(HookError::refuse(format!(
                "`{kind}`: the value is not the point's"
            )));
        };
        let mut ask = RelayedAsk::new(ask_run(), next_ask_id(), kind, deadline, input, text);
        ask.version = version;
        let (run, id) = (ask.run, ask.id);
        let sent = Instant::now();
        let Some(answer) = send(ask) else {
            return Err(HookError::refuse(format!(
                "`{kind}`: no answer within {deadline:?}"
            )));
        };
        if sent.elapsed() > deadline {
            return Err(HookError::refuse(format!(
                "`{kind}`: the answer came after {deadline:?}"
            )));
        }
        if (answer.run, answer.id) != (run, id) {
            return Err(HookError::refuse(format!(
                "`{kind}`: the answer is to another ask"
            )));
        }
        match answer.answer {
            // The value is replaced only when the answer reads.
            Answer::Done { value: text } => decode_value(&text, value).map_err(|reason| {
                HookError::refuse(format!("`{kind}`: cannot read the answer: {reason}"))
            }),
            Answer::Refused {
                listener, reason, ..
            } => Err(HookError::refuse(match listener {
                Some(l) => format!("refused on the far side by `{l}`: {reason}"),
                None => format!("refused on the far side: {reason}"),
            })),
        }
    })
}
