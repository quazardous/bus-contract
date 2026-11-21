# The Journal and Relay capabilities

How to use them ([Journal](#journal), [Relay](#relay)), then why they
are shaped this way ([Design note](#design-note)). What a relayable
request means across processes is on [crossing.md](crossing.md).

## Journal

`bus.journal()` (`use bus_contract_journal::BusJournal;`) returns
`Option<&dyn Journal>`. An event point declared with the `JOURNALED`
facet is published through the journal instead of emitted: the
publisher gets a number and moves on, and the listeners hear each event
once, in number order, on a thread of the implementation's (a pump). A
slow listener delays the next events, not the publisher.

- `publish::<P>(input)` / `publish_caused::<P>(input, cause)`: the
  entry's number, from 1. Refused on a point not declared journaled: if
  you publish to a point, declare it with the `JOURNALED` facet (or a
  relayable one).
- `since(seq, limit)`: the entries after a number, with `gap` set when
  some left the journal's window; `last_seq()`.
- `wait(patterns, seq, timeout)` / `wait_for::<P>`: the next entry of a
  kind (`vault.*`, see `kind_matches`), at once if already there.
- `flush(timeout)`: every entry recorded so far delivered (before an
  exit, in tests).

## Relay

`bus.relay()` (`use bus_contract_crossing::BusCrossing;`) returns
`Option<&dyn Relay>`, on a bus that offers the journal. An event point
declared with the `relayable_event::<P>()` facet (codec
`Codec::of::<P>()`) has its input written as text and read back by
another process (`Relayed`: the format is the application's). The
transport is not part of the contract (see [crossing.md](crossing.md)).

- `cursor()`: where a reader starting now stands (the journal's run and
  last number). `resume(cursor, limit)`: the relayable entries after
  it, and `lost` when the reader missed some (`Gap`: the window;
  `Restarted`: the process started again and numbers with it). A
  reader that lost entries resyncs, then applies the ones returned.
- `ingest(origin, entry)`: an entry relayed in is recorded with its
  origin and delivered to its point's listeners like a local one. A
  kind not declared here is kept as text and heard by no one; a point
  declared here but not relayable, or at another version, is refused.
  Entries taken in are not served again.

```rust,ignore
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<LibraryImported>("Photos were imported.")
        .facets(&[relayable_event::<LibraryImported>()]),
    PointDecl::of::<StepComputed>("A render step was computed.").facets(&[JOURNALED]),
];

// the node publishes and moves on:
bus.journal().expect("declared journaled").publish::<LibraryImported>(imported)?;
// its server answers a desktop reader:
let read = bus.relay().expect("declared relayable").resume(cursor, 256);
// the desktop takes the entries in; its listeners hear them in order:
for entry in read.entries { desktop.relay().unwrap().ingest(&Origin::process("node"), entry)?; }
```

## Design note

The core contract delivers an emit to its listeners inline, on the
emitting thread, in a declared order. Applications that already run a
fact bus (past-tense events with a sequence number, a window read again
from a number, events relayed between processes) had to keep it beside
the contract: two vocabularies, anonymous subscribers draining channels.
`Journal` and `Relay` let such a bus implement the contract's missing
parts instead, as optional capabilities shaped like `Defer`.

### Decisions

**Optional, found at run time.** The journal and the relay are
capability contracts of their own (`bus-contract-journal`,
`bus-contract-crossing`), reached through the core's typed lookup
(`bus.journal()`, `bus.relay()` with the crates' extension traits),
`None` when the bus does not offer them. A point that needs one says so
in its declaration with a facet (`JOURNALED`, `relayable(&CODEC)`) and
`declare_all` refuses it on a bus that cannot serve it
(`Refused::Unsupported`), as `needs` does for `Defer`.
`PointDecl::required()` lists the contracts a point needs, named and
from its facets.

**Journaled is a property of an event point.** Only `PointKind::Event`
points are journaled (the journal contract refuses the others, `Refused::Contract`): requests and
alters return values to their caller, which a deferred delivery cannot.
A journaled point's events are published through the journal, not
emitted; the same point type is used to listen, so a listener does not
know whether its events were published here or relayed in.

**Publishing never runs a listener.** Fact publishers (renderers, file
watchers, database writers) must not block on a listener. The journal
records the entry and returns its number; a pump of the
implementation's delivers entries in number order as collect emits,
with every rule of the core (order, deadlines, failures, local
listeners queued for their thread). The cost is head-of-line blocking
on the pump: a slow listener delays the next entries. Deadlines bound
it; an implementation may run one pump per bus.

**Numbers per journal, a run per process.** Numbers start at 1 and are
never reused while the process runs. A restart starts numbers again, so
a relay cursor carries the journal's run: a cursor of another run is
answered with `Lost::Restarted`, an evicted range with `Lost::Gap`. A
reader that lost entries resyncs from the current state, then applies
what it was given. `Since::gap` says the same for in-process readers.

**Codecs as function pointers.** The crate has no dependencies, so it
does not pick a format. A relayable point implements `Relayed` (text in,
text out) and its table line carries `Codec::of::<P>()`: three function
pointers built in a `const`. Two processes of different versions must
read each other, so decoders are asked to accept unknown fields and
fail only on a payload they cannot use (`IngestError::Decode`).

**What crosses, and what does not.** Only relayable points are served
by `resume`, so internal, high-rate events stay in their process. An
entry relayed in keeps its source and the sender's cause number; it is
not served again, so two processes relaying to each other do not echo.
A kind the receiver does not declare is kept as text, delivered to no
one, readable with `since`. A point declared on the receiver but not
relayable is refused: a remote process may not publish it.

**No transport.** A relay reads (`resume`), takes in (`ingest`) and
answers (`answer`); a bridge moves the text over its own socket or RPC.

**Requests cross as asks (0.4).** A journal cannot carry a request: its
caller waits for a value. A relayable request instead crosses as an
ask answered by the far side's `Relay::answer`, and on the asking side
the far side is one listener (`far_listener`) in the point's order. The
contract fixes what a bridge may not choose: the ask carries a
deadline, no answer in time is a refusal (the point is fail-closed), a
veto carries its reason, and each ask is run once per origin. Without
it, each transport would give timeouts and vetoes its own meaning and
one bus would quietly become several.

**Origin and authority (0.4).** The receiver names the origin from the
link, never from the payload, so a sender cannot claim another one.
Authorities see each crossing before it is decoded: a refused one
leaves no trace but the refusal.

**Versions (0.4).** Decoders already accept unknown fields, which
covers additive changes. A version on the point covers the rest: the
same version on both sides, or the crossing is refused, rather than
read wrongly.

### Not decided here

- Whether `Journal` entries of kinds with no point (an implementation's
  own untyped facts) get a contract type: the journal keeps them as
  text, which is enough to read and relay them by hand.
- Back-pressure on the pump (a bounded queue, dropping, or blocking the
  publisher past a size): left to implementations for now.
- Reading an older version of a point (a range of versions a codec
  reads): additive later; for now the versions must be equal.
- Point aliases (a point renamed across versions).
