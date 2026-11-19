# Changelog

## 0.2.0

Breaking, with `bus-contract` 0.5.0: this crate owns its contract.
See [MIGRATION-0.5.md](https://github.com/quazardous/bus-contract/blob/main/MIGRATION-0.5.md).

- The contract's items are defined here, no longer re-exported from the
  core: `Journal` (and its typed `publish`, `publish_caused`, `wait_for`), `JournalEntry`, `Since`, `Origin`, `kind_matches`.
- `CONTRACT`, the contract as data (`bus_contract::Contract`): what a
  point names in `PointDecl::needs` and what `offers` checks. It
  replaces `Capability`.
- `JOURNALED`, the facet of a journaled event point (data `Journaled`),
  and `is_journaled(&facets)`. They replace `PointDecl::journaled()`,
  `PointSpec::journaled` and `PointInfo::journaled`.
- For implementations: `check(key, spec)` refuses a journaled point
  that is not an event, `not_event` and `not_journaled` build the
  refusals (`Refused::Contract`), replacing `Refused::NotEvent` and
  `Refused::NotJournaled`.
- `BusJournal` answers through the typed lookup `Bus::capability` only:
  the core's getter is gone, and so is the E0034 name clash.

## 0.1.0

First release: the public name of the journal contract, before the core
moves it here.

- Re-exports the `bus-contract` 0.4 core's journal items (`Journal`, `JournalEntry`, `Since`, `Origin`, `kind_matches`).
  They are the core's own types, so code using either path works
  together.
- `BusJournal`, an extension trait implemented for every `Bus` (sized or
  not), gives `bus.journal()`. It answers through the core's getter, which
  defaults to the typed lookup `Bus::capability`.
