#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::any::{Any, type_name};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bus_contract::{
    AnyInput, AnyListener, Bus, CloneFn, Contract, Facet, FreshFn, HookError, Point, PointKey,
    PointKind, PointSpec, Refused, capability, clone_value, fresh_value,
};

/// The journal the relay sits on: a bus that offers [`Relay`] offers
/// [`Journal`] too.
pub use bus_contract_journal::{
    BusJournal, JOURNALED, Journal, JournalEntry, Origin, Since, is_journaled,
};

/// The crossing contract: a bus offers it by answering for `dyn Relay`
/// in `Bus::capability`, and offers the journal contract too. A
/// relayable point says so with the [`relayable`] facet, which needs
/// this contract.
pub const CONTRACT: Contract =
    Contract::new::<dyn Relay>("crossing", "0.2", &[&bus_contract_journal::CONTRACT]);

/// The facet of a relayable point, carrying its codec: the point
/// crosses processes ([`Relay`]):
///
/// - an event point's events, their input written and read by `codec`
///   ([`Codec::of::<P>()`](Codec::of)); the bus journals them too;
/// - a request point's asks, from another process, their input and value
///   written and read by `codec` ([`Codec::request::<P>()`](Codec::request));
///   the bus treats it as fail-closed too, so no answer in time is a
///   refusal.
///
/// In a `const` table, the codec is a `const` of its own:
///
/// ```ignore
/// const CODEC: Codec = Codec::of::<DocChanged>();
/// PointDecl::of::<DocChanged>("…").facets(&[relayable(&CODEC)])
/// ```
///
/// [`check`] refuses it on an alter or collect point, on a request whose
/// codec has no value, and with a codec of another point.
pub const fn relayable(codec: &'static Codec) -> Facet {
    Facet::new(&CONTRACT, codec)
}

/// The [`relayable`] facet of the event point `P`, its codec
/// [`Codec::of::<P>()`](Codec::of): usable in a `const` table and at run
/// time alike (`.facets(&[relayable_event::<DocChanged>()])`,
/// `PointSpec::new(…).facet(relayable_event::<DocChanged>())`).
pub const fn relayable_event<P: Relayed>() -> Facet
where
    P::Value: Default,
{
    Facet::new(&CONTRACT, &CodecOf::<P>::EVENT)
}

/// The [`relayable`] facet of the request point `P`, its codec
/// [`Codec::request::<P>()`](Codec::request): usable in a `const` table
/// and at run time alike.
pub const fn relayable_request<P: RelayedRequest>() -> Facet
where
    P::Value: Default + Clone,
{
    Facet::new(&CONTRACT, &CodecOf::<P>::REQUEST)
}

/// The codecs of `P` as constants, so a facet can borrow them for
/// `'static`.
struct CodecOf<P>(std::marker::PhantomData<P>);

impl<P: Relayed> CodecOf<P>
where
    P::Value: Default,
{
    const EVENT: Codec = Codec::of::<P>();
}

impl<P: RelayedRequest> CodecOf<P>
where
    P::Value: Default + Clone,
{
    const REQUEST: Codec = Codec::request::<P>();
}

/// The codec of a relayable point, from its facets (`PointSpec::facets`,
/// `PointInfo::facets`); `None` when it is not relayable.
pub fn codec(facets: &[Facet]) -> Option<&'static Codec> {
    Facet::find::<Codec>(facets, &CONTRACT)
}

/// What the crossing contract refuses in a declaration, for an
/// implementation's `declare_point` ([`Refused::Contract`]): a relayable
/// point with another point's codec, an alter or collect point, a
/// request whose codec writes no value.
pub fn check(point: &PointKey, spec: &PointSpec) -> Result<(), Refused> {
    let Some(codec) = codec(&spec.facets) else {
        return Ok(());
    };
    let refuse = |reason: &str| Refused::Contract {
        point: point.id,
        contract: CONTRACT.name,
        reason: reason.into(),
    };
    if codec.point != point.id {
        return Err(refuse("the codec is another point's"));
    }
    let crosses = match point.kind {
        PointKind::Event => true,
        PointKind::Request => codec.answers(),
        _ => false,
    };
    if !crosses {
        return Err(refuse(
            "it cannot cross: only an event, or a request with a value codec, crosses",
        ));
    }
    Ok(())
}

/// The spec as a bus that implements crossing applies it: a relayable
/// event point is journaled ([`JOURNALED`] added), a relayable request is
/// fail-closed.
pub fn applied(point: &PointKey, mut spec: PointSpec) -> PointSpec {
    if codec(&spec.facets).is_some() {
        match point.kind {
            PointKind::Event if !is_journaled(&spec.facets) => spec.facets.push(JOURNALED),
            PointKind::Request => spec.fail_closed = true,
            _ => {}
        }
    }
    spec
}

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
/// ([`relayable`]). Two codecs
/// are equal when they are for the same point.
///
/// Build one with [`Codec::of`] (an event) or [`Codec::request`] (a
/// request).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Codec {
    /// The point it is for ([`Point::ID`]): [`declare_all`](bus_contract::declare_all)
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
    /// alter emit, [`Bus::alter_any`]); `None` on
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
            point: P::ID.as_str(),
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
            point: P::ID.as_str(),
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
    /// ([`PointDecl::version`](bus_contract::PointDecl::version)): a receiver
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
        /// ([`Refusal::failed`](bus_contract::Refusal::failed)).
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

/// A capability of a bus ([`BusCrossing::relay`]), on top of
/// its [`Journal`]: what crosses a process boundary. The events of
/// relayable points served to another process from a cursor and taken
/// in from one as if published here; the asks of relayable request
/// points answered here for another process.
///
/// A point is relayable when its declaration gives it a codec
/// ([`relayable`]); only events
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
///   rule of [`Bus`] holds; a relayable request point is
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

/// Reaches the [`Relay`] capability of any bus: `bus.relay()`.
///
/// Implemented for every [`Bus`], sized or not (`dyn Bus`, `Arc<dyn
/// Bus>`, a concrete bus): what the bus answers for `dyn Relay` through
/// the typed lookup `Bus::capability`.
pub trait BusCrossing {
    /// The [`Relay`] capability of this bus; `None` when the bus does not
    /// offer it.
    fn relay(&self) -> Option<&dyn Relay>;
}

impl<B: Bus + ?Sized> BusCrossing for B {
    fn relay(&self) -> Option<&dyn Relay> {
        capability::<dyn Relay, B>(self)
    }
}
