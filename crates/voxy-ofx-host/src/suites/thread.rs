//! `OfxMultiThreadSuiteV1`.
//!
//! `multiThread` runs `min(nThreads, CPUs)` threads (all CPUs when
//! `nThreads` is 0), each calling the function once with its index and
//! that count, and returns when all are done. A call from a spawned thread
//! gets `kOfxStatErrExists`, as the spec forbids recursion. A plugin's own
//! threading is outside voXY's determinism promise (`docs/ofx-host.md`).
//!
//! Mutexes are recursive: the owning thread may lock again, and each lock
//! needs an unlock. `mutexCreate` with a lock count of n creates it held n
//! times by the calling thread. Unlocking a mutex the thread does not hold
//! gives `kOfxStatFailed`.

use std::cell::Cell;
use std::ffi::{c_int, c_uint, c_void};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};
use std::thread::ThreadId;

use voxy_ofx_sys as ofx;

use super::{Registry, guard};

pub(super) static SUITE: ofx::OfxMultiThreadSuiteV1 = ofx::OfxMultiThreadSuiteV1 {
    multiThread: Some(multi_thread),
    multiThreadNumCPUs: Some(num_cpus),
    multiThreadIndex: Some(thread_index),
    multiThreadIsSpawnedThread: Some(is_spawned),
    mutexCreate: Some(mutex_create),
    mutexDestroy: Some(mutex_destroy),
    mutexLock: Some(mutex_lock),
    mutexUnLock: Some(mutex_unlock),
    mutexTryLock: Some(mutex_try_lock),
};

thread_local! {
    /// The index of a thread spawned by `multiThread`, or `None`.
    static INDEX: Cell<Option<c_uint>> = const { Cell::new(None) };
}

fn cpus() -> c_uint {
    std::thread::available_parallelism()
        .map_or(1, |n| c_uint::try_from(n.get()).unwrap_or(c_uint::MAX))
}

/// The plugin's argument, sent to the worker threads as an address.
#[derive(Clone, Copy)]
struct Arg(usize);

unsafe extern "C" fn multi_thread(
    func: ofx::OfxThreadFunctionV1,
    n_threads: c_uint,
    arg: *mut c_void,
) -> ofx::OfxStatus {
    guard(|| {
        let Some(func) = func else {
            return ofx::kOfxStatErrValue;
        };
        if INDEX.get().is_some() {
            return ofx::kOfxStatErrExists;
        }
        let max = if n_threads == 0 {
            cpus()
        } else {
            n_threads.min(cpus())
        };
        let arg = Arg(arg as usize);
        let launched = std::thread::scope(|s| {
            let handles: Vec<_> = (0..max)
                .map(|i| {
                    std::thread::Builder::new()
                        .name(format!("ofx-worker-{i}"))
                        .spawn_scoped(s, move || {
                            let arg = arg;
                            INDEX.set(Some(i));
                            // SAFETY: the plugin's thread function with its
                            // own argument, as the spec says.
                            unsafe { func(i, max, arg.0 as *mut c_void) };
                        })
                })
                .collect();
            handles
                .into_iter()
                .all(|h| h.is_ok_and(|h| h.join().is_ok()))
        });
        if launched {
            ofx::kOfxStatOK
        } else {
            ofx::kOfxStatFailed
        }
    })
}

unsafe extern "C" fn num_cpus(out: *mut c_uint) -> ofx::OfxStatus {
    guard(|| {
        if out.is_null() {
            return ofx::kOfxStatErrValue;
        }
        // SAFETY: a valid out pointer from the plugin.
        unsafe { out.write(cpus()) };
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn thread_index(out: *mut c_uint) -> ofx::OfxStatus {
    guard(|| {
        if out.is_null() {
            return ofx::kOfxStatErrValue;
        }
        // SAFETY: as above.
        unsafe { out.write(INDEX.get().unwrap_or(0)) };
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn is_spawned() -> c_int {
    c_int::from(INDEX.get().is_some())
}

/// A recursive mutex.
#[derive(Debug, Default)]
pub(super) struct OfxMutex {
    owner: Mutex<Option<(ThreadId, usize)>>,
    freed: Condvar,
}

impl OfxMutex {
    fn try_lock(&self, wait: bool) -> bool {
        let me = std::thread::current().id();
        let mut owner = self.owner.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match &mut *owner {
                None => {
                    *owner = Some((me, 1));
                    return true;
                }
                Some((t, n)) if *t == me => {
                    *n += 1;
                    return true;
                }
                Some(_) if wait => {
                    owner = self
                        .freed
                        .wait(owner)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(_) => return false,
            }
        }
    }

    fn unlock(&self) -> bool {
        let me = std::thread::current().id();
        let mut owner = self.owner.lock().unwrap_or_else(PoisonError::into_inner);
        match &mut *owner {
            Some((t, n)) if *t == me => {
                *n -= 1;
                if *n == 0 {
                    *owner = None;
                    self.freed.notify_one();
                }
                true
            }
            _ => false,
        }
    }
}

static MUTEXES: LazyLock<Registry<OfxMutex>> = LazyLock::new(Registry::new);

unsafe extern "C" fn mutex_create(
    out: *mut ofx::OfxMutexHandle,
    lock_count: c_int,
) -> ofx::OfxStatus {
    guard(|| {
        if out.is_null() {
            return ofx::kOfxStatErrValue;
        }
        let m = Arc::new(OfxMutex::default());
        if let Ok(n) = usize::try_from(lock_count)
            && n > 0
        {
            *m.owner.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((std::thread::current().id(), n));
        }
        let h = MUTEXES.register(&m);
        // SAFETY: a valid out pointer from the plugin.
        unsafe { out.write(h as ofx::OfxMutexHandle) };
        ofx::kOfxStatOK
    })
}

fn with_mutex(
    h: ofx::OfxMutexHandle,
    f: impl FnOnce(&OfxMutex) -> ofx::OfxStatus,
) -> ofx::OfxStatus {
    guard(|| match MUTEXES.get(h as usize) {
        Some(m) => f(&m),
        None => ofx::kOfxStatErrBadHandle,
    })
}

unsafe extern "C" fn mutex_destroy(h: ofx::OfxMutexHandle) -> ofx::OfxStatus {
    with_mutex(h, |_| {
        MUTEXES.unregister_addr(h as usize);
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn mutex_lock(h: ofx::OfxMutexHandle) -> ofx::OfxStatus {
    with_mutex(h, |m| {
        m.try_lock(true);
        ofx::kOfxStatOK
    })
}

unsafe extern "C" fn mutex_unlock(h: ofx::OfxMutexHandle) -> ofx::OfxStatus {
    with_mutex(h, |m| {
        if m.unlock() {
            ofx::kOfxStatOK
        } else {
            ofx::kOfxStatFailed
        }
    })
}

unsafe extern "C" fn mutex_try_lock(h: ofx::OfxMutexHandle) -> ofx::OfxStatus {
    with_mutex(h, |m| {
        if m.try_lock(false) {
            ofx::kOfxStatOK
        } else {
            ofx::kOfxStatFailed
        }
    })
}
