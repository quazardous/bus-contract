# Crossing

One bus may span several processes (a desktop and its node, a plugin,
an agent's session). The contract says **what** may cross a process
boundary and **what it means** when it does; it never says **how** it
crosses. Framing, wire format, transports, reconnection and discovery
belong to a bridge built on these traits. Whatever the bridge, a
listener cannot tell whether the far side is in its process or not:
that is the meaning a conformance suite checks.

## What may cross

A point crosses only when its table line says so, with the crossing facet carrying its codec (`relayable(&CODEC)`):

- an event point (`relayable_event::<P>()`, codec `Codec::of::<P>()`,
  `Relayed`): its events, as on [journal-and-relay.md](journal-and-relay.md#relay);
  the bus journals them;
- a request point (`relayable_request::<P>()`, codec
  `Codec::request::<P>()`, `RelayedRequest`: the value is written and
  read too): asks from another process. A relayable request is
  fail-closed.

Alter and collect points never cross (`Refused::Contract` from the
crossing contract). Each point has a version (`PointDecl::version`, from 1), raised when its
payload changes in a way an older reader cannot use; a field a reader
may ignore is not such a change. Two processes cross a point only at
the same version: another one is refused (`IngestError::Version`, or a
refused answer).

## A request answered on the other side

On the asking side the far side is one listener, `far_listener(codec, deadline, version, send)`,
registered on the point like any other: the asking side's order decides
when the far side takes its turn. `send` is the bridge's part: it moves
the ask (`RelayedAsk`: input and value so far as text, the deadline, a
number) to the far side's `Relay::answer`, and the answer back.

- **Deadline, fail-closed.** An ask carries its deadline. No answer in
  time (a link down, a far side gone, too slow) refuses the request:
  "no answer" means "no". The far side counts the deadline from when it
  got the ask, runs nothing when it has passed, and answers refused
  when its listeners end past it, so both sides hold the same outcome.
  If you order the far listener before your own listeners with
  effects, a dead link refuses the request before they run.
- **A veto carries its reason.** A far listener's refusal comes back as
  `Answer::Refused` with the listener and its reason, and refuses the
  request here with that reason; the far side's edit is dropped. A
  refusal by the far bus itself (an authority, a point it does not
  declare as a relayable request, another version, an input it cannot
  read) has no listener.
- **Answered once.** An ask is named by the asker's run and its number.
  The far side runs the listeners of an ask once per origin: a repeat
  gets the first answer again, or a refusal once it has been dropped.
  The asking side takes one answer, to its own ask, in time; anything
  else refuses.

Effects the far side's listeners made before the deadline ran out are
theirs to undo or to make idempotent: the contract makes both sides
agree on the outcome, not roll back work. If a far listener of a
relayable request has effects, make it idempotent or let it undo its
own work.

## Origin

Whatever crosses in is taken with an `Origin`: the process
on the far side of the link, and the agent it speaks for when the link
is an agent's session. The receiving side sets it from the link, never
from what the sender wrote. Every entry taken in keeps it
(`JournalEntry::origin`); every ask is checked against it.

## Authority

`Relay::add_authority` (typed: `authorize`) adds a check
shown each crossing (`Crossing`: origin, kind, version, ask or event)
before anything is decoded, recorded or run. Every authority is asked,
in order; one refusal refuses (`IngestError::Refused`, or a refused
answer); a panic refuses. Authorities guard a trusted process against
mistakes, not against an attacker (see
[GUARANTEES.md](../GUARANTEES.md), `crossing.authority.not-security`).

```rust,ignore
pub const POINTS: &[PointDecl] = &[PointDecl::of::<FileDelete>("Files are deleted.")
    .facets(&[relayable_request::<FileDelete>()])];

// the node: agents may not delete.
node.relay().unwrap().authorize(|c| match &c.origin.agent {
    Some(agent) if c.kind == "files.delete" => Err(format!("{agent} may not delete")),
    _ => Ok(()),
});
// its bridge answers each ask it reads off its link:
let answer = node.relay().unwrap().answer(&link_origin, ask);

// the desktop: the node takes its turn after the desktop's own listeners.
desktop.listen_any(
    PointKey::of::<FileDelete>(),
    "desktop.node".into(),
    ListenOpts::new().after("desktop.confirm").deadline(PLUGIN_DEADLINE).threaded(),
    far_listener(Codec::request::<FileDelete>(), PLUGIN_DEADLINE, 1, move |ask| link.ask(ask)),
)?;
let out = desktop.alter::<FileDelete>(path, Vec::new());
if let Some(refusal) = out.refused { /* the node said no, or said nothing in time */ }
```
