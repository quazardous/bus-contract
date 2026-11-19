# Changelog

## 0.2.0

Breaking, with `bus-contract` 0.5.0: this crate owns its contract.
See [MIGRATION-0.5.md](https://github.com/quazardous/bus-contract/blob/main/MIGRATION-0.5.md).

- The contract's items are defined here, no longer re-exported from the
  core: `Relay` (and `authorize`), `Codec`, `Relayed`, `RelayedRequest`, `Answer`, `Authority`, `Crossing`, `Cursor`, `IngestError`, `Lost`, `Resumed`, `RelayedEntry`, `RelayedAsk`, `RelayedAnswer`, the codec function types, `far_listener`, `ask_run`.
- `CONTRACT`, the contract as data (`bus_contract::Contract`): what a
  point names in `PointDecl::needs` and what `offers` checks. It
  replaces `Capability`.
- `CONTRACT` requires the journal contract.
- The crossing facet carrying a point's codec: `relayable(&CODEC)`,
  `relayable_event::<P>()`, `relayable_request::<P>()`, and
  `codec(&facets)`. They replace `PointDecl::relayable`,
  `PointSpec::relayable` / `relayable_request`, `PointSpec::relay` and
  `PointInfo::relayable`.
- For implementations: `check(key, spec)` refuses a point that cannot
  cross or carries another point's codec (`Refused::Contract`, replacing
  `Refused::NotCrossable`), and `applied(key, spec)` journals a
  relayable event and makes a relayable request fail-closed.
- Re-exports the journal items it sits on (`BusJournal`, `Journal`,
  `JournalEntry`, `Since`, `Origin`, `JOURNALED`, `is_journaled`).
- `BusCrossing` answers through the typed lookup `Bus::capability` only:
  the core's getter is gone, and so is the E0034 name clash.

## 0.1.0

First release: the public name of the crossing contract, before the core
moves it here.

- Re-exports the `bus-contract` 0.4 core's crossing items (`Relay`, `Relayed`, `RelayedRequest`, `Codec`, `Cursor`, `RelayedEntry`, `Resumed`, `Lost`, `IngestError`, `Crossing`, `Authority`, `RelayedAsk`, `Answer`, `RelayedAnswer`, `Origin`, `EncodeFn`, `DecodeFn`, `EncodeValueFn`, `DecodeValueFn`, `ask_run`, `far_listener`) and, from `bus-contract-journal`, `BusJournal`, `Journal`, `JournalEntry` and `Since`.
  They are the core's own types, so code using either path works
  together.
- `BusCrossing`, an extension trait implemented for every `Bus` (sized or
  not), gives `bus.relay()`. It answers through the core's getter, which
  defaults to the typed lookup `Bus::capability`.
