//! The relayable facet: usable in a `const` table with its codec, needing
//! the crossing contract, which requires the journal one.

use std::sync::Arc;

use bus_contract::{
    Bus, Point, PointDecl, PointId, PointKey, PointKind, PointSpec, Refused, declare_all, offers,
};
use bus_contract_crossing::{
    CONTRACT, Codec, JOURNALED, Relayed, RelayedRequest, applied, check, codec, is_journaled,
    relayable,
};
use bus_contract_testkit::{LookupBus, Offers};

struct Imported;
impl Point for Imported {
    const ID: PointId = PointId::new("test.library.imported");
    const KIND: PointKind = PointKind::Event;
    type Input = String;
    type Value = ();
}
impl Relayed for Imported {
    fn encode(input: &String) -> String {
        input.clone()
    }
    fn decode(text: &str) -> Result<String, String> {
        Ok(text.to_owned())
    }
}

struct Delete;
impl Point for Delete {
    const ID: PointId = PointId::new("test.files.delete");
    const KIND: PointKind = PointKind::Request;
    type Input = String;
    type Value = u32;
}
impl Relayed for Delete {
    fn encode(input: &String) -> String {
        input.clone()
    }
    fn decode(text: &str) -> Result<String, String> {
        Ok(text.to_owned())
    }
}
impl RelayedRequest for Delete {
    fn encode_value(value: &u32) -> String {
        value.to_string()
    }
    fn decode_value(text: &str) -> Result<u32, String> {
        text.parse().map_err(|e| format!("{e}"))
    }
}

const IMPORTED: Codec = Codec::of::<Imported>();
const DELETE: Codec = Codec::request::<Delete>();

const TABLE: &[PointDecl] = &[
    PointDecl::of::<Imported>("A library was imported.").facets(&[relayable(&IMPORTED)]),
    PointDecl::of::<Delete>("Files are deleted.").facets(&[relayable(&DELETE)]),
];

#[test]
fn crossing_requires_the_journal() {
    let relay_only: Arc<dyn Bus> = Arc::new(LookupBus::new(Offers {
        relay: true,
        ..Offers::NONE
    }));
    assert!(!offers(&*relay_only, &CONTRACT));
    assert_eq!(
        declare_all(&*relay_only, TABLE)[0],
        Refused::Unsupported {
            point: "test.library.imported",
            contract: "crossing"
        }
    );
    let both: Arc<dyn Bus> = Arc::new(LookupBus::new(Offers {
        relay: true,
        journal: true,
        ..Offers::NONE
    }));
    assert!(offers(&*both, &CONTRACT));
    assert!(declare_all(&*both, TABLE).is_empty());
    assert_eq!(CONTRACT.requires, [&bus_contract_journal::CONTRACT]);
}

#[test]
fn the_codec_is_read_back_from_the_facets() {
    assert_eq!(codec(&TABLE[0].spec().facets), Some(&IMPORTED));
    assert_eq!(TABLE[0].required(), [&CONTRACT]);
    assert_eq!(codec(&PointSpec::new("x").facets), None);
}

#[test]
fn a_bus_applies_a_relayable_point() {
    let event = applied(&PointKey::of::<Imported>(), TABLE[0].spec());
    assert!(is_journaled(&event.facets) && !event.fail_closed);
    let request = applied(&PointKey::of::<Delete>(), TABLE[1].spec());
    assert!(request.fail_closed && !is_journaled(&request.facets));
    // Applying twice adds nothing.
    let again = applied(&PointKey::of::<Imported>(), event.clone());
    assert_eq!(again.facets.len(), event.facets.len());
    let _ = JOURNALED;
}

#[test]
fn check_refuses_what_cannot_cross() {
    assert!(check(&PointKey::of::<Imported>(), &TABLE[0].spec()).is_ok());
    assert!(check(&PointKey::of::<Delete>(), &TABLE[1].spec()).is_ok());
    // Another point's codec.
    let other = PointSpec::new("x").facet(relayable(&IMPORTED));
    let err = check(&PointKey::of::<Delete>(), &other).unwrap_err();
    assert!(err.to_string().contains("another point's"), "{err}");
    // A request whose codec writes no value.
    const NO_VALUE: Codec = Codec::of::<Delete>();
    let err = check(
        &PointKey::of::<Delete>(),
        &PointSpec::new("x").facet(relayable(&NO_VALUE)),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        Refused::Contract {
            contract: "crossing",
            ..
        }
    ));
}
