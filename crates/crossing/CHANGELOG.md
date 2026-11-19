# Changelog

## 0.1.0

First release: the public name of the crossing contract, before the core
moves it here.

- Re-exports the `bus-contract` 0.4 core's crossing items (`Relay`, `Relayed`, `RelayedRequest`, `Codec`, `Cursor`, `RelayedEntry`, `Resumed`, `Lost`, `IngestError`, `Crossing`, `Authority`, `RelayedAsk`, `Answer`, `RelayedAnswer`, `Origin`, `EncodeFn`, `DecodeFn`, `EncodeValueFn`, `DecodeValueFn`, `ask_run`, `far_listener`) and, from `bus-contract-journal`, `BusJournal`, `Journal`, `JournalEntry` and `Since`.
  They are the core's own types, so code using either path works
  together.
- `BusCrossing`, an extension trait implemented for every `Bus` (sized or
  not), gives `bus.relay()`. It answers through the core's getter, which
  defaults to the typed lookup `Bus::capability`.
