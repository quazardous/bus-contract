//! The journaled facet: usable in a `const` table, needing the journal
//! contract, refused on a point that is not an event.

use std::sync::Arc;

use bus_contract::{
    Bus, Point, PointDecl, PointId, PointKey, PointKind, PointSpec, Refused, declare_all,
};
use bus_contract_journal::{CONTRACT, JOURNALED, check, is_journaled, not_journaled};
use bus_contract_testkit::{LookupBus, Offers};

struct Mounted;
impl Point for Mounted {
    const ID: PointId = PointId::new("test.volume.mounted");
    const KIND: PointKind = PointKind::Event;
    type Input = String;
    type Value = ();
}

struct Close;
impl Point for Close {
    const ID: PointId = PointId::new("test.doc.close");
    const KIND: PointKind = PointKind::Request;
    type Input = ();
    type Value = ();
}

const TABLE: &[PointDecl] =
    &[PointDecl::of::<Mounted>("A volume was mounted.").facets(&[JOURNALED])];

#[test]
fn a_journaled_point_needs_the_journal_contract() {
    assert_eq!(TABLE[0].required(), [&CONTRACT]);
    assert!(is_journaled(&TABLE[0].spec().facets));
    assert!(!is_journaled(&PointSpec::new("plain").facets));
    let without: Arc<dyn Bus> = Arc::new(LookupBus::new(Offers::NONE));
    assert_eq!(
        declare_all(&*without, TABLE),
        [Refused::Unsupported {
            point: "test.volume.mounted",
            contract: "journal"
        }]
    );
    let with: Arc<dyn Bus> = Arc::new(LookupBus::new(Offers {
        journal: true,
        ..Offers::NONE
    }));
    assert!(declare_all(&*with, TABLE).is_empty());
}

#[test]
fn only_an_event_point_is_journaled() {
    let spec = PointSpec::new("x").facet(JOURNALED);
    assert!(check(&PointKey::of::<Mounted>(), &spec).is_ok());
    let err = check(&PointKey::of::<Close>(), &spec).unwrap_err();
    assert!(matches!(
        err,
        Refused::Contract {
            point: "test.doc.close",
            contract: "journal",
            ..
        }
    ));
    assert!(
        err.to_string().contains("only an event point is journaled"),
        "{err}"
    );
    assert!(matches!(
        not_journaled("test.doc.close"),
        Refused::Contract {
            contract: "journal",
            ..
        }
    ));
}
