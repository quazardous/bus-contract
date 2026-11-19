//! The capability lookup (`Bus::capability`) and the getters agree both
//! ways: a bus written before the lookup, which overrides only the
//! getters, and a bus that implements only `capability`.

use std::any::{Any, TypeId};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bus_contract::{
    Altered, AnyInput, AnyListener, AnyLocalListener, AnyValue, Bus, Capability, Cast, CloneFn,
    Collected, Defer, FailureSink, FreshFn, Journal, ListenOpts, ListenerInfo, PendingRun,
    PendingWaker, Point, PointDecl, PointInfo, PointKey, PointSpec, Provided, Refused, Relay,
    Timings, capability, declare_all, offers,
};

/// The core methods, which these tests never call.
macro_rules! core_unused {
    () => {
        fn declare_point(&self, _: PointKey, _: PointSpec) -> Result<(), Refused> {
            Ok(())
        }
        fn listen_any(
            &self,
            _: PointKey,
            _: String,
            _: ListenOpts,
            _: AnyListener,
        ) -> Result<(), Refused> {
            unimplemented!()
        }
        fn listen_local_any(
            &self,
            _: PointKey,
            _: String,
            _: ListenOpts,
            _: AnyLocalListener,
        ) -> Result<(), Refused> {
            unimplemented!()
        }
        fn run_pending(&self) -> Vec<PendingRun> {
            Vec::new()
        }
        fn add_pending_waker(&self, _: PendingWaker) {}
        fn unlisten(&self, _: &str, _: &str) -> bool {
            false
        }
        fn forget(&self, _: &str) -> usize {
            0
        }
        fn alter_any(
            &self,
            _: PointKey,
            _: AnyInput,
            _: AnyValue,
            _: CloneFn,
        ) -> Altered<AnyValue> {
            unimplemented!()
        }
        fn collect_any(&self, _: PointKey, _: AnyInput, _: FreshFn) -> Collected<AnyValue> {
            unimplemented!()
        }
        fn add_failure_sink(&self, _: FailureSink) {}
        fn points(&self) -> Vec<PointInfo> {
            Vec::new()
        }
        fn listeners(&self, _: &str) -> Option<Vec<ListenerInfo>> {
            None
        }
    };
}

/// A `Defer` that only counts asks.
macro_rules! counting_defer {
    ($bus:ty) => {
        impl Defer for $bus {
            fn ask_any(&self, _: PointKey, _: AnyInput, _: AnyValue, _: CloneFn) -> u64 {
                self.asks.fetch_add(1, Ordering::SeqCst) + 1
            }
            fn drain(&self) -> Vec<PendingRun> {
                Vec::new()
            }
            fn waiting(&self) -> usize {
                0
            }
        }
    };
}

/// A bus written before the lookup: it overrides the `defer` getter.
#[derive(Default)]
struct GetterOnly {
    asks: AtomicU64,
}
counting_defer!(GetterOnly);
impl Bus for GetterOnly {
    core_unused!();
    fn defer(&self) -> Option<&dyn Defer> {
        Some(self)
    }
}

/// A bus written for the lookup: it implements `capability` only.
#[derive(Default)]
struct LookupOnly {
    asks: AtomicU64,
}
counting_defer!(LookupOnly);
fn lookup_as_defer(bus: &dyn Any) -> Option<&(dyn Defer + 'static)> {
    bus.downcast_ref::<LookupOnly>().map(|b| b as _)
}
static LOOKUP_AS_DEFER: Cast<dyn Defer> = lookup_as_defer;
impl Bus for LookupOnly {
    core_unused!();
    fn capability(&self, contract: TypeId) -> Option<Provided<'_>> {
        (contract == TypeId::of::<dyn Defer>())
            .then(|| Provided::new::<dyn Defer>(self, &LOOKUP_AS_DEFER))
    }
}

/// A faulty bus: it answers the `Journal` id with its `Defer` cast.
struct Mismatched(LookupOnly);
impl Bus for Mismatched {
    core_unused!();
    fn capability(&self, contract: TypeId) -> Option<Provided<'_>> {
        (contract == TypeId::of::<dyn Journal>())
            .then(|| Provided::new::<dyn Defer>(&self.0, &LOOKUP_AS_DEFER))
    }
}

struct Write;
impl Point for Write {
    const ID: &'static str = "test.write.asked";
    type Input = ();
    type Value = u32;
}

const NEEDS_DEFER: &[PointDecl] =
    &[PointDecl::of::<Write>("A write asked for later.").needs(&[Capability::Defer])];

/// The address of the object a reference points at, to compare two
/// trait objects found by different routes.
fn addr<T: ?Sized>(r: &T) -> *const () {
    r as *const T as *const ()
}

#[test]
fn a_bus_overriding_only_the_getters_keeps_working() {
    let bus: Arc<dyn Bus> = Arc::new(GetterOnly::default());
    let defer = bus.defer().expect("the getter answers");
    assert_eq!(defer.ask::<Write>((), 1), 1);
    assert!(offers(&*bus, Capability::Defer));
    assert!(!offers(&*bus, Capability::Journal));
    assert!(declare_all(&*bus, NEEDS_DEFER).is_empty());
    // The other getters keep their `None` default.
    assert!(bus.timings().is_none() && bus.journal().is_none() && bus.relay().is_none());
    // The lookup knows nothing it was not told.
    assert!(bus.get::<dyn Defer>().is_none());
}

#[test]
fn a_bus_implementing_only_the_lookup_answers_through_the_getters() {
    let bus: Arc<dyn Bus> = Arc::new(LookupOnly::default());
    let by_getter = bus.defer().expect("the getter defaults to the lookup");
    let by_lookup = bus.get::<dyn Defer>().expect("the lookup answers");
    assert_eq!(addr(by_getter), addr(by_lookup));
    assert_eq!(by_getter.ask::<Write>((), 1), 1);
    assert_eq!(by_lookup.ask::<Write>((), 1), 2);
    assert!(offers(&*bus, Capability::Defer));
    assert!(!offers(&*bus, Capability::Timings));
    assert!(declare_all(&*bus, NEEDS_DEFER).is_empty());
    assert!(bus.timings().is_none() && bus.journal().is_none() && bus.relay().is_none());
    assert!(bus.get::<dyn Timings>().is_none());
    assert!(bus.get::<dyn Relay>().is_none());
}

#[test]
fn a_bus_without_the_capability_refuses_the_point() {
    let bus: Arc<dyn Bus> = Arc::new(Mismatched(LookupOnly::default()));
    let refused = declare_all(&*bus, NEEDS_DEFER);
    assert!(matches!(refused.as_slice(), [Refused::Unsupported { .. }]));
}

#[test]
fn the_typed_lookup_works_on_concrete_and_shared_buses() {
    let concrete = LookupOnly::default();
    assert!(capability::<dyn Defer, _>(&concrete).is_some());
    assert!(concrete.defer().is_some());
    let shared = Arc::new(LookupOnly::default());
    assert!(capability::<dyn Defer, _>(&shared).is_some());
    assert!(shared.defer().is_some());
    // An `Arc` of an `Arc<dyn Bus>` forwards the lookup too.
    let inner: Arc<dyn Bus> = Arc::new(LookupOnly::default());
    let wrapped: Arc<dyn Bus> = Arc::new(inner.clone());
    assert_eq!(
        addr(wrapped.get::<dyn Defer>().unwrap()),
        addr(inner.get::<dyn Defer>().unwrap())
    );
}

#[test]
fn a_cast_of_another_capability_gives_none_not_a_wrong_type() {
    let bus: Arc<dyn Bus> = Arc::new(Mismatched(LookupOnly::default()));
    assert!(bus.capability(TypeId::of::<dyn Journal>()).is_some());
    assert!(bus.get::<dyn Journal>().is_none());
    assert!(bus.journal().is_none());
    assert!(!offers(&*bus, Capability::Journal));
}
