//! Easing JSON (`interp` values and whole keyframe tracks) never panics, and
//! anything accepted satisfies the easing guarantees: valid parameters, exact
//! endpoints, finite values and speeds.
#![no_main]

use libfuzzer_sys::fuzz_target;
use voxy_core::{Interp, KeyframeTrack};

fn check(interp: &Interp) {
    assert!(interp.validate().is_ok(), "accepted invalid {interp:?}");
    if interp.is_parametric() {
        assert_eq!(interp.shape_progress(0.0), Some(0.0));
        assert_eq!(interp.shape_progress(1.0), Some(1.0));
        for i in 0..=64 {
            let u = f64::from(i) / 64.0;
            assert!(interp.shape_progress(u).unwrap().is_finite());
            assert!(interp.shape_slope(u).unwrap().is_finite());
        }
        for j in interp.shape_jumps() {
            assert!(j.height > 0.0 && (0.0..=1.0).contains(&j.at));
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    if let Ok(interp) = serde_json::from_str::<Interp>(text) {
        check(&interp);
        // Round trip: what is written reads back identically.
        let json = serde_json::to_string(&interp).expect("encodes");
        assert_eq!(serde_json::from_str::<Interp>(&json).expect("re-read"), interp);
    }
    if let Ok(track) = serde_json::from_str::<KeyframeTrack>(text) {
        for k in track.keys() {
            check(&k.interp);
        }
        // Evaluation is total.
        for k in track.keys() {
            let _ = track.evaluate(k.time, true);
        }
    }
});
