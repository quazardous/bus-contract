//! Test buses for the workspace's tests: a bus that offers the
//! capabilities it is told to through `Bus::capability`
//! ([`LookupBus`]). Its capability methods do the least that lets a
//! test tell which object answered.

use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bus_contract::{
    Altered, AnyInput, AnyListener, AnyLocalListener, AnyValue, Bus, Cast, CloneFn, Collected,
    FailureSink, FreshFn, ListenOpts, ListenerInfo, PendingRun, PendingWaker, PointInfo, PointKey,
    PointSpec, Provided, Refused,
};
use bus_contract_crossing::{
    Authority, Cursor, IngestError, Origin, Relay, RelayedAnswer, RelayedAsk, RelayedEntry, Resumed,
};
use bus_contract_defer::Defer;
use bus_contract_journal::{Journal, JournalEntry, Since};
use bus_contract_timings::{BudgetSink, PointTimings, Timings};

/// Which capabilities a test bus offers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Offers {
    /// `Defer`.
    pub defer: bool,
    /// `Timings`.
    pub timings: bool,
    /// `Journal`.
    pub journal: bool,
    /// `Relay` (a bus offering it should offer `Journal` too).
    pub relay: bool,
}

impl Offers {
    /// Every capability.
    pub const ALL: Offers = Offers {
        defer: true,
        timings: true,
        journal: true,
        relay: true,
    };
    /// None.
    pub const NONE: Offers = Offers {
        defer: false,
        timings: false,
        journal: false,
        relay: false,
    };
}

/// The capability methods: each counts its calls
/// in `calls`, so a test can see the object answering is this bus.
#[derive(Default)]
pub struct Parts {
    /// Calls made to any capability method.
    pub calls: AtomicU64,
}

impl Parts {
    fn hit(&self) -> u64 {
        self.calls.fetch_add(1, Ordering::SeqCst) + 1
    }
}

impl Defer for Parts {
    fn ask_any(&self, _: PointKey, _: AnyInput, _: AnyValue, _: CloneFn) -> u64 {
        self.hit()
    }
    fn drain(&self) -> Vec<PendingRun> {
        Vec::new()
    }
    fn waiting(&self) -> usize {
        0
    }
}

impl Timings for Parts {
    fn points(&self) -> Vec<PointTimings> {
        self.hit();
        Vec::new()
    }
    fn point(&self, _: &str) -> Option<PointTimings> {
        None
    }
    fn reset(&self) {}
    fn add_budget_sink(&self, _: BudgetSink) {}
}

impl Journal for Parts {
    fn last_seq(&self) -> u64 {
        self.hit()
    }
    fn since(&self, _: u64, _: usize) -> Since {
        Since::new(Vec::new(), 0)
    }
    fn wait(&self, _: &[&str], _: u64, _: Duration) -> Option<JournalEntry> {
        None
    }
    fn publish_any(
        &self,
        _: PointKey,
        _: AnyInput,
        _: FreshFn,
        _: Option<u64>,
    ) -> Result<u64, Refused> {
        Ok(self.hit())
    }
    fn flush(&self, _: Duration) -> bool {
        true
    }
}

impl Relay for Parts {
    fn cursor(&self) -> Cursor {
        Cursor {
            run: 1,
            seq: self.hit(),
        }
    }
    fn resume(&self, from: Cursor, _: usize) -> Resumed {
        Resumed::new(Vec::new(), from)
    }
    fn ingest(&self, _: &Origin, _: RelayedEntry) -> Result<u64, IngestError> {
        Ok(self.hit())
    }
    fn answer(&self, _: &Origin, _: RelayedAsk) -> RelayedAnswer {
        unimplemented!("not used by the tests")
    }
    fn add_authority(&self, _: Authority) {}
}

/// The core methods, which the tests never call.
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

/// A bus that offers its capabilities through `Bus::capability` only.
#[derive(Default)]
pub struct LookupBus {
    /// What it offers.
    pub offers: Offers,
    /// Its capability methods.
    pub parts: Parts,
}

impl LookupBus {
    /// A bus offering `offers`.
    pub fn new(offers: Offers) -> LookupBus {
        LookupBus {
            offers,
            parts: Parts::default(),
        }
    }
}

fn parts(bus: &dyn Any) -> Option<&Parts> {
    bus.downcast_ref::<LookupBus>().map(|b| &b.parts)
}
static AS_DEFER: Cast<dyn Defer> = |b| parts(b).map(|p| p as _);
static AS_TIMINGS: Cast<dyn Timings> = |b| parts(b).map(|p| p as _);
static AS_JOURNAL: Cast<dyn Journal> = |b| parts(b).map(|p| p as _);
static AS_RELAY: Cast<dyn Relay> = |b| parts(b).map(|p| p as _);

impl Bus for LookupBus {
    core_unused!();
    fn capability(&self, contract: TypeId) -> Option<Provided<'_>> {
        let o = self.offers;
        if o.defer && contract == TypeId::of::<dyn Defer>() {
            Some(Provided::new::<dyn Defer>(self, &AS_DEFER))
        } else if o.timings && contract == TypeId::of::<dyn Timings>() {
            Some(Provided::new::<dyn Timings>(self, &AS_TIMINGS))
        } else if o.journal && contract == TypeId::of::<dyn Journal>() {
            Some(Provided::new::<dyn Journal>(self, &AS_JOURNAL))
        } else if o.relay && contract == TypeId::of::<dyn Relay>() {
            Some(Provided::new::<dyn Relay>(self, &AS_RELAY))
        } else {
            None
        }
    }
}

/// The address of the object a reference points at, to compare two
/// trait objects found by different routes.
pub fn addr<T: ?Sized>(r: &T) -> *const () {
    r as *const T as *const ()
}
