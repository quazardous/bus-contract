//! `BusJournal` reaches the capability on every kind of bus through the
//! lookup, and the contract is what `offers` and `declare_all` check.

use std::sync::Arc;

use bus_contract_journal::BusJournal;
use bus_contract_testkit::{LookupBus, Offers, addr};

const OFFERS: Offers = Offers {
    journal: true,
    ..Offers::NONE
};

/// Calls the capability once, through `BusJournal`.
fn exercise<B: BusJournal + ?Sized>(bus: &B) -> u64 {
    bus.journal().unwrap().last_seq()
}

#[test]
fn reached_on_a_bus_offering_it_through_the_lookup() {
    let shared: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let by_ext = shared.journal().expect("the extension finds it");
    let by_lookup = shared
        .get::<dyn bus_contract_journal::Journal>()
        .expect("the lookup finds it");
    assert_eq!(addr(by_ext), addr(by_lookup));
    exercise(&shared);
    exercise(&*shared);
    exercise(&LookupBus::new(OFFERS));
    exercise(&Arc::new(LookupBus::new(OFFERS)));
}

#[test]
fn none_on_a_bus_without_it() {
    let everything_else = Offers {
        journal: false,
        ..Offers::ALL
    };
    let lookup: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(everything_else));
    assert!(lookup.journal().is_none());
}

#[test]
fn the_contract_is_offered_by_a_bus_that_answers_the_lookup() {
    let with: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(OFFERS));
    let without: Arc<dyn bus_contract::Bus> = Arc::new(LookupBus::new(Offers::NONE));
    assert!(bus_contract::offers(
        &*with,
        &bus_contract_journal::CONTRACT
    ));
    assert!(!bus_contract::offers(
        &*without,
        &bus_contract_journal::CONTRACT
    ));
    assert_eq!(
        bus_contract_journal::CONTRACT.type_id(),
        std::any::TypeId::of::<dyn bus_contract_journal::Journal>()
    );
    assert_eq!(bus_contract_journal::CONTRACT.version, "0.2");
}
