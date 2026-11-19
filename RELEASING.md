# Releasing

This repository is a Cargo workspace of crates released one by one,
each with its own version, CHANGELOG and tag.

| Crate | Path | Tag |
|---|---|---|
| `bus-contract` | `.` (the root) | `v<version>` |
| `bus-contract-defer` | `crates/defer` | `bus-contract-defer-v<version>` |
| `bus-contract-timings` | `crates/timings` | `bus-contract-timings-v<version>` |
| `bus-contract-journal` | `crates/journal` | `bus-contract-journal-v<version>` |
| `bus-contract-crossing` | `crates/crossing` | `bus-contract-crossing-v<version>` |

`crates/testkit` holds test buses for the workspace's tests and is
never published.

The core stays at the root so its published path, its history and its
existing `v<version>` tags do not move; its package excludes the other
crates.

## Versions

- Each crate follows semver on its own. A change to one capability
  contract bumps that crate only, plus the crates that depend on it
  when they re-export what changed.
- A crate's CHANGELOG gets its entry in the change that makes it, under
  the version it will ship as.
- Dependencies inside the workspace carry both a `path` and a
  `version`: the path builds the workspace, the version is what
  crates.io sees. Raise the version requirement when a crate starts
  needing a newer release of another.

## Publish order

Dependencies first. Skip a crate whose version did not change.

1. `bus-contract`
2. `bus-contract-defer`, `bus-contract-timings`, `bus-contract-journal`
   (each depends on the core only)
3. `bus-contract-crossing` (depends on the core and on
   `bus-contract-journal`)

```sh
cargo publish -p bus-contract
cargo publish -p bus-contract-defer
cargo publish -p bus-contract-timings
cargo publish -p bus-contract-journal
cargo publish -p bus-contract-crossing
```

Then tag each published crate on the merged commit with the tag above,
push the tags, and create one GitHub release per tag with the crate's
CHANGELOG entry as notes.
