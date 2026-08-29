//! Empirical measurement backing issue #58, "In-scope Sessions have no
//! stay-put pressure". This runs in the default `cargo test --workspace`
//! gate, so it stays to ONE seed and a modest move budget — enough to prove
//! the harness still works, not to re-derive precise ratios every run (the
//! real multi-seed sweep that produced the numbers quoted on the issue was
//! run once, by hand, in `--release`). For that sweep: `cargo test -p
//! calendry-solver-gen --release --test churn -- --nocapture` with the seed
//! range below widened back to `1..=3` and the budget raised.
use calendry_solver_core::search::Budget;
use calendry_solver_gen::{Preset, churn, generate};

#[test]
fn churn_report() {
    // One small preset, one large — full `Preset::ALL` coverage belongs to
    // the by-hand `--release` sweep documented above, not every `cargo test
    // --workspace` run.
    for preset in [Preset::SmallSchool, Preset::LargeUniversity] {
        let instance = generate(&preset.params(), 1);
        let budget = Budget { max_wall_millis: 0, max_moves: 20_000 };
        let seed = 1u64;
        match churn::measure_with_control(&instance.problem, seed, budget) {
            Some((with_clash, control)) => {
                eprintln!(
                    "{:<18} seed={seed} target={:<16} with-clash free={:<5} \
                     churned={:<5} ratio={:.3} | control free={:<5} churned={:<5} \
                     ratio={:.3}",
                    preset.name(),
                    with_clash.target_offering,
                    with_clash.free_placements,
                    with_clash.churned,
                    with_clash.churn_ratio,
                    control.free_placements,
                    control.churned,
                    control.churn_ratio
                );
            }
            None => {
                eprintln!("{:<18} seed={seed} no Offering large enough / unlocked", preset.name());
            }
        }
    }
}
