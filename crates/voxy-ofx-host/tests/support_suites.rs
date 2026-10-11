//! The message, progress, time line and multi-thread suites, called through
//! `fetchSuite` the way a plugin calls them.

#![allow(unsafe_code, clippy::unwrap_used)]

use std::ffi::{c_char, c_int, c_long, c_uint, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use voxy_ofx_host::effect::{Effect, Message, MessageKind, Progress, TimeLine};
use voxy_ofx_host::props::{PropSet, Values};
use voxy_ofx_host::suites::{
    HostStruct, effect_handle, release_effect_handle, take_unattached_messages,
};
use voxy_ofx_sys as ofx;

fn host() -> HostStruct {
    let props = Arc::new(PropSet::new("host"));
    props.define_ro("OfxPropName", Values::strings(&["org.voxy.test"]));
    HostStruct::new(props)
}

fn suite<T>(h: &HostStruct, name: &[u8], version: i32) -> &'static T {
    let p = h.fetch_suite(name, version);
    assert!(!p.is_null(), "{}", String::from_utf8_lossy(name));
    // SAFETY: the host returns its static suite of this type.
    unsafe { &*p.cast::<T>() }
}

fn c(s: &[u8]) -> *const c_char {
    s.as_ptr().cast()
}

#[test]
fn messages_are_formatted_like_printf() {
    let h = host();
    let m: &ofx::OfxMessageSuiteV2 = suite(&h, ofx::kOfxMessageSuite, 2);
    let fx = Arc::new(Effect::new());
    let fh = effect_handle(&fx).cast::<c_void>();
    let post = m.message.unwrap();
    let warn = c(ofx::kOfxMessageWarning);
    // SAFETY: each format matches its arguments, as C requires.
    unsafe {
        assert_eq!(post(fh, warn, c(b"id.1\0"), c(b"plain 100%%\0")), 0);
        assert_eq!(
            post(
                fh,
                warn,
                std::ptr::null(),
                c(b"%d|%5d|%-5d|%05d|%+d|%x|%#X|%o|%u\0"),
                -42 as c_int,
                7 as c_int,
                7 as c_int,
                -7 as c_int,
                3 as c_int,
                255 as c_uint,
                255 as c_uint,
                8 as c_uint,
                4_000_000_000u32
            ),
            0
        );
        assert_eq!(
            post(
                fh,
                warn,
                std::ptr::null(),
                c(b"%ld %lld %zu %c\0"),
                -5 as c_long,
                1i64 << 40,
                12usize,
                c_int::from(b'Q')
            ),
            0
        );
        assert_eq!(
            post(
                fh,
                warn,
                std::ptr::null(),
                c(b"[%s] [%.3s] [%8s] [%-4s] [%s]\0"),
                c(b"text\0"),
                c(b"abcdef\0"),
                c(b"right\0"),
                c(b"l\0"),
                std::ptr::null::<c_char>()
            ),
            0
        );
        assert_eq!(
            post(
                fh,
                warn,
                std::ptr::null(),
                c(b"%f %.2f %8.3f %e %.1E %g %g %g %G\0"),
                1.5f64,
                -2.345f64,
                3.142_59_f64,
                12345.678f64,
                0.000_123_f64,
                100_000.0_f64,
                1e-5f64,
                0.5f64,
                1e20f64
            ),
            0
        );
        assert_eq!(
            post(
                fh,
                warn,
                std::ptr::null(),
                c(b"%*d|%.*f|%n|%Lf after\0"),
                4 as c_int,
                9 as c_int,
                1 as c_int,
                2.25f64,
                std::ptr::null_mut::<c_int>()
            ),
            0
        );
    }
    let texts: Vec<String> = fx.take_messages().into_iter().map(|m| m.text).collect();
    assert_eq!(
        texts,
        [
            "plain 100%",
            "-42|    7|7    |-0007|+3|ff|0XFF|10|4000000000",
            "-5 1099511627776 12 Q",
            "[text] [abc] [   right] [l   ] [(null)]",
            "1.500000 -2.35    3.143 1.234568e+04 1.2E-04 100000 1e-05 0.5 1E+20",
            "   9|2.2||…",
        ]
    );
    release_effect_handle(&fx);
}

#[test]
fn message_kinds_ids_questions_and_persistent_messages() {
    let h = host();
    let m: &ofx::OfxMessageSuiteV2 = suite(&h, ofx::kOfxMessageSuite, 2);
    // V1 is the same struct's first entry.
    let v1: &ofx::OfxMessageSuiteV1 = suite(&h, ofx::kOfxMessageSuite, 1);
    assert_eq!(
        std::ptr::from_ref(v1).cast::<c_void>(),
        std::ptr::from_ref(m).cast::<c_void>()
    );
    let fx = Arc::new(Effect::new());
    let fh = effect_handle(&fx).cast::<c_void>();
    // SAFETY: valid strings and no format arguments.
    unsafe {
        assert_eq!(
            m.message.unwrap()(fh, c(ofx::kOfxMessageError), c(b"e\0"), c(b"bad\0")),
            0
        );
        assert_eq!(
            m.message.unwrap()(fh, c(ofx::kOfxMessageQuestion), c(b"q\0"), c(b"sure?\0")),
            ofx::kOfxStatFailed,
            "no user to ask in the sandbox"
        );
        assert_eq!(
            m.message.unwrap()(fh, c(b"NotAType\0"), c(b"x\0"), c(b"x\0")),
            ofx::kOfxStatErrValue
        );
        assert_eq!(
            m.message.unwrap()(
                std::ptr::null_mut(),
                c(ofx::kOfxMessageLog),
                std::ptr::null(),
                c(b"loose\0")
            ),
            0
        );
        assert_eq!(
            m.message.unwrap()(
                0x10 as *mut c_void,
                c(ofx::kOfxMessageLog),
                std::ptr::null(),
                c(b"x\0")
            ),
            ofx::kOfxStatErrBadHandle
        );
        assert_eq!(
            m.setPersistentMessage.unwrap()(
                fh,
                c(ofx::kOfxMessageWarning),
                c(b"w\0"),
                c(b"slow %d\0"),
                3 as c_int
            ),
            0
        );
        assert_eq!(
            m.setPersistentMessage.unwrap()(fh, c(ofx::kOfxMessageLog), c(b"w\0"), c(b"no\0")),
            ofx::kOfxStatErrValue
        );
        assert_eq!(
            m.setPersistentMessage.unwrap()(
                std::ptr::null_mut(),
                c(ofx::kOfxMessageError),
                c(b"w\0"),
                c(b"no\0")
            ),
            ofx::kOfxStatErrBadHandle
        );
    }
    let msgs = fx.take_messages();
    assert_eq!(
        msgs.iter()
            .map(|m| (m.kind, m.id.as_deref()))
            .collect::<Vec<_>>(),
        [
            (MessageKind::Error, Some("e")),
            (MessageKind::Question, Some("q"))
        ]
    );
    assert!(take_unattached_messages().iter().any(|m| m.text == "loose"));
    assert_eq!(
        fx.persistent(),
        Some(Message {
            kind: MessageKind::Warning,
            id: Some("w".into()),
            text: "slow 3".into(),
        })
    );
    // SAFETY: a valid effect handle.
    unsafe { assert_eq!(m.clearPersistentMessage.unwrap()(fh), 0) };
    assert_eq!(fx.persistent(), None);
    release_effect_handle(&fx);
}

#[test]
fn progress_reports_and_abort_replies_no() {
    let h = host();
    let p1: &ofx::OfxProgressSuiteV1 = suite(&h, ofx::kOfxProgressSuite, 1);
    let p2: &ofx::OfxProgressSuiteV2 = suite(&h, ofx::kOfxProgressSuite, 2);
    let fx = Arc::new(Effect::new());
    let fh = effect_handle(&fx).cast::<c_void>();
    // SAFETY: a valid effect handle and strings.
    unsafe {
        assert_eq!(
            p2.progressStart.unwrap()(fh, c(b"Rendering\0"), c(b"r\0")),
            0
        );
        assert_eq!(p2.progressUpdate.unwrap()(fh, 0.25), 0);
        assert_eq!(
            fx.progress(),
            Some(Progress {
                label: "Rendering".into(),
                id: Some("r".into()),
                fraction: 0.25,
            })
        );
        fx.set_abort(true);
        assert_eq!(p1.progressUpdate.unwrap()(fh, 2.0), ofx::kOfxStatReplyNo);
        assert_eq!(fx.progress().unwrap().fraction, 1.0, "clamped");
        assert_eq!(p1.progressEnd.unwrap()(fh), 0);
        assert_eq!(fx.progress(), None);
        assert_eq!(p1.progressStart.unwrap()(fh, c(b"Again\0")), 0);
        assert_eq!(fx.progress().unwrap().id, None);
    }
    release_effect_handle(&fx);
    // SAFETY: a released handle is refused.
    unsafe {
        assert_eq!(
            p1.progressUpdate.unwrap()(fh, 0.5),
            ofx::kOfxStatErrBadHandle
        );
    }
}

#[test]
fn the_time_line_is_what_the_host_set_and_cannot_be_moved() {
    let h = host();
    let t: &ofx::OfxTimeLineSuiteV1 = suite(&h, ofx::kOfxTimeLineSuite, 1);
    let fx = Arc::new(Effect::new());
    fx.set_time_line(TimeLine {
        time: 12.0,
        first: 0.0,
        last: 99.0,
    });
    let fh = effect_handle(&fx).cast::<c_void>();
    // SAFETY: valid handle and out pointers.
    unsafe {
        let (mut now, mut a, mut b) = (0.0, 0.0, 0.0);
        assert_eq!(t.getTime.unwrap()(fh, &raw mut now), 0);
        assert_eq!(t.getTimeBounds.unwrap()(fh, &raw mut a, &raw mut b), 0);
        assert_eq!((now, a, b), (12.0, 0.0, 99.0));
        assert_eq!(t.gotoTime.unwrap()(fh, 5.0), ofx::kOfxStatFailed);
        assert_eq!(fx.time_line().time, 12.0);
    }
    release_effect_handle(&fx);
}

static CALLS: AtomicU32 = AtomicU32::new(0);
static INDEX_SUM: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn worker(index: c_uint, max: c_uint, arg: *mut c_void) {
    // SAFETY: the test passes the suite as the argument.
    let s = unsafe { &*arg.cast::<ofx::OfxMultiThreadSuiteV1>() };
    let mut me = 99;
    // SAFETY: valid out pointer; the nested call must be refused.
    unsafe {
        assert_eq!(s.multiThreadIndex.unwrap()(&raw mut me), 0);
        assert_eq!(s.multiThreadIsSpawnedThread.unwrap()(), 1);
        assert_eq!(
            s.multiThread.unwrap()(Some(worker), 2, arg),
            ofx::kOfxStatErrExists
        );
    }
    assert_eq!(me, index);
    assert!(index < max);
    CALLS.fetch_add(1, Ordering::SeqCst);
    INDEX_SUM.fetch_add(index, Ordering::SeqCst);
}

#[test]
fn multi_thread_runs_each_index_once_and_refuses_recursion() {
    let h = host();
    let s: &ofx::OfxMultiThreadSuiteV1 = suite(&h, ofx::kOfxMultiThreadSuite, 1);
    let mut cpus = 0;
    // SAFETY: valid pointers; the worker reads the suite through `arg`.
    unsafe {
        assert_eq!(s.multiThreadNumCPUs.unwrap()(&raw mut cpus), 0);
        assert!(cpus >= 1);
        let want = 3.min(cpus);
        let arg = std::ptr::from_ref(s).cast_mut().cast::<c_void>();
        assert_eq!(s.multiThread.unwrap()(Some(worker), 3, arg), 0);
        assert_eq!(CALLS.load(Ordering::SeqCst), want);
        assert_eq!(INDEX_SUM.load(Ordering::SeqCst), (0..want).sum::<u32>());
        let mut me = 7;
        assert_eq!(s.multiThreadIndex.unwrap()(&raw mut me), 0);
        assert_eq!(me, 0, "not a spawned thread");
        assert_eq!(s.multiThreadIsSpawnedThread.unwrap()(), 0);
        assert_eq!(s.multiThread.unwrap()(None, 1, arg), ofx::kOfxStatErrValue);
    }
}

#[test]
fn mutexes_are_recursive_and_exclusive() {
    let h = host();
    let s: &'static ofx::OfxMultiThreadSuiteV1 = suite(&h, ofx::kOfxMultiThreadSuite, 1);
    let mut m: ofx::OfxMutexHandle = std::ptr::null_mut();
    // SAFETY: a valid out pointer and the handle it returns.
    unsafe {
        assert_eq!(s.mutexCreate.unwrap()(&raw mut m, 0), 0);
        assert_eq!(s.mutexLock.unwrap()(m), 0);
        assert_eq!(s.mutexTryLock.unwrap()(m), 0, "recursive");
        let addr = m as usize;
        let other = std::thread::spawn(move || {
            let m = addr as ofx::OfxMutexHandle;
            // The same live handle, from another thread.
            (s.mutexTryLock.unwrap()(m), s.mutexUnLock.unwrap()(m))
        })
        .join()
        .unwrap();
        assert_eq!(other, (ofx::kOfxStatFailed, ofx::kOfxStatFailed));
        assert_eq!(s.mutexUnLock.unwrap()(m), 0);
        assert_eq!(s.mutexUnLock.unwrap()(m), 0);
        assert_eq!(s.mutexUnLock.unwrap()(m), ofx::kOfxStatFailed, "not held");
        assert_eq!(s.mutexDestroy.unwrap()(m), 0);
        assert_eq!(s.mutexLock.unwrap()(m), ofx::kOfxStatErrBadHandle);

        // Created held: another thread waits until the creator unlocks.
        assert_eq!(s.mutexCreate.unwrap()(&raw mut m, 1), 0);
        let addr = m as usize;
        let waiter = std::thread::spawn(move || {
            let m = addr as ofx::OfxMutexHandle;
            let got = s.mutexLock.unwrap()(m);
            s.mutexUnLock.unwrap()(m);
            got
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!waiter.is_finished());
        assert_eq!(s.mutexUnLock.unwrap()(m), 0);
        assert_eq!(waiter.join().unwrap(), 0);
        assert_eq!(s.mutexDestroy.unwrap()(m), 0);
    }
}
