//! Empirical measurement backing issue #58, "In-scope Sessions have no
//! stay-put pressure": run `cargo test -p calendry-solver-gen --release
//! churn_report -- --nocapture` to print the numbers for every preset.
use calendry_solver_core::search::Budget;
use calendry_solver_gen::{Preset, churn, generate};

#[test]
fn churn_report() {
    for preset in Preset::ALL {
        let instance = generate(&preset.params(), 1);
        let budget = Budget { max_wall_millis: 0, max_moves: 200_000 };
        for seed in 1..=3u64 {
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
                None => eprintln!(
                    "{:<18} seed={seed} no Offering large enough / unlocked",
                    preset.name()
                ),
            }
        }
    }
}
