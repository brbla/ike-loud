//! Proofs for every input, checked by Kani (`scripts/kani.sh`). Not tests
//! of a formula against itself: properties the brake must have whatever the
//! formula is. Compiled only under `cfg(kani)`; mutation testing skips this
//! file (`scripts/mutants.sh`).

use crate::context::{MassChangeLimits, Threshold};

fn any_threshold() -> Threshold {
    Threshold {
        absolute: kani::any(),
        minimum: kani::any(),
        percent: kani::any(),
    }
}

/// One more change never releases the brake.
#[kani::proof]
fn more_changes_never_release_the_brake() {
    let t = any_threshold();
    let (count, total): (u64, u64) = (kani::any(), kani::any());
    kani::assume(count < u64::MAX);
    if t.exceeded_by(count, total) {
        assert!(t.exceeded_by(count + 1, total));
    }
}

/// The default limits: up to 10 deletions never stop, more than 50
/// always do, whatever the number of synced files (spec).
#[kani::proof]
fn default_deletion_limits() {
    let t = MassChangeLimits::default().destructive;
    let (count, total): (u64, u64) = (kani::any(), kani::any());
    if count <= 10 {
        assert!(!t.exceeded_by(count, total));
    }
    if count > 50 {
        assert!(t.exceeded_by(count, total));
    }
}
