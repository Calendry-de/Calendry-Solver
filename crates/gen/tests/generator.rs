//! Generator tests.
//!
//! These deliberately assert only that generated instances are **well-formed
//! and reproducible**. They never assert that the solver's answer on a generated
//! instance is correct — that is what the hand-written fixtures in
//! `calendry_solver_core::testing` are for, and a generator that graded its own
//! output would be a bug that silently validates itself.

use calendry_solver_gen::{Preset, TARGET_SATURATION, digest, generate};

#[test]
fn same_seed_reproduces_the_instance_exactly() {
    let params = Preset::SmallSchool.params();

    let a = generate(&params, 7);
    let b = generate(&params, 7);
    assert_eq!(
        digest(&a.problem),
        digest(&b.problem),
        "same (params, seed) must reproduce the same instance"
    );
    assert_eq!(a.stats.placements, b.stats.placements);
    assert_eq!(a.stats.total_demand_blocks, b.stats.total_demand_blocks);

    // A benchmark you cannot vary is as useless as one you cannot reproduce.
    let c = generate(&params, 8);
    assert_ne!(
        digest(&a.problem),
        digest(&c.problem),
        "a different seed must produce a different instance"
    );
}

#[test]
fn every_offering_can_be_placed_somewhere() {
    // An Offering with no eligible Room is unplaceable by construction, which
    // would show up as an unplaced Session and be misread as a search failure.
    for preset in Preset::ALL {
        let instance = generate(&preset.params(), 3);
        for o in &instance.problem.offerings {
            assert!(
                !o.eligible_rooms.is_empty(),
                "{}: offering '{}' has no eligible room",
                preset.name(),
                o.id
            );
        }
    }
}

#[test]
fn presets_are_calibrated_into_the_saturation_band() {
    for preset in Preset::ALL {
        let params = preset.params();

        // Closed form first: cheap, and it is what calibration is done against.
        let predicted = params.predicted_saturation();
        assert!(
            TARGET_SATURATION.contains(&predicted),
            "{}: predicted saturation {predicted:.3} outside {TARGET_SATURATION:?}",
            preset.name()
        );

        // Then the measured value, which is the one that actually decides
        // whether construction can solve the instance.
        let instance = generate(&params, 11);
        let s = &instance.stats;
        assert!(
            TARGET_SATURATION.contains(&s.saturation),
            "{}: measured saturation {:.3} outside {TARGET_SATURATION:?} \
             (group {:.3}, lecturer {:.3}, room {:.3})",
            preset.name(),
            s.saturation,
            s.max_group_load,
            s.max_lecturer_load,
            s.room_tightness
        );

        // The group axis binds; if a preset ever stops being group-bound that is
        // a change in what the benchmark measures and should be deliberate.
        assert!(
            s.max_group_load >= s.room_tightness,
            "{}: room axis has overtaken the group axis",
            preset.name()
        );
    }
}

#[test]
fn electives_create_tree_unrelated_co_membership() {
    // This is the whole reason `elective_ratio` exists. PersonDoubleBooking only
    // catches clashes the Group check structurally cannot: a Person in two
    // Groups where neither is an ancestor or a descendant of the other. If the
    // generator never produced that shape, the type would be dead weight at
    // benchmark scale and its cost would be invisible.
    let instance = generate(&Preset::SmallUniversity.params(), 5);
    let closure = &instance.problem.closure;

    let found = instance.problem.persons.iter().any(|p| {
        p.groups.len() >= 2
            && p.groups.iter().enumerate().any(|(i, &a)| {
                p.groups[i + 1..].iter().any(|&b| !closure.conflicts(a, b))
            })
    });

    assert!(
        found,
        "no person belongs to two tree-unrelated groups; PersonDoubleBooking \
         would be unexercised"
    );
}

#[test]
fn locked_sessions_are_linked_and_complete_their_offerings() {
    // Every occurrence is realized exactly once: either as a placement variable
    // the solver must position, or as a locked Session carrying its Offering
    // link. Together they must equal required_session_count, so
    // `constraints::exact_frequency` is satisfiable — otherwise every Offering
    // holding a lock would report a violation it does not have, and the
    // benchmark would measure that gap instead of the search.
    let instance = generate(&Preset::SmallSchool.params(), 2);
    let problem = &instance.problem;
    assert!(instance.stats.fixed > 0, "preset should generate some locks");

    let mut realized = vec![0u32; problem.offerings.len()];
    for p in problem.placement_ids() {
        realized[problem.placement(p).offering.get()] += 1;
    }

    let mut linked_locks = 0;
    for f in &problem.fixed {
        let o = f.offering.expect("a generated lock always realizes an Offering");
        realized[o.get()] += 1;
        linked_locks += 1;
    }
    assert_eq!(
        linked_locks, instance.stats.fixed,
        "every locked Session must carry its Offering link"
    );

    for (o, &n) in problem.offerings.iter().zip(&realized) {
        assert_eq!(
            o.required_session_count, n,
            "offering '{}' requires {} but {n} occurrences realize it",
            o.id, o.required_session_count
        );
    }
}

#[test]
fn the_hierarchy_is_a_three_level_forest() {
    let params = Preset::LargeSchool.params();
    let instance = generate(&params, 1);
    let groups = &instance.problem.groups;

    assert_eq!(groups.len(), params.group_count() as usize);

    // Depth is exactly Cohort -> Class -> Seminar. `Problem::build` already
    // rejects cycles, so reaching a root proves acyclicity for these paths.
    let depth = |mut g: usize| {
        let mut d = 0;
        while let Some(parent) = groups[g].parent {
            g = parent.get();
            d += 1;
            assert!(d <= 8, "cycle or unexpected depth in group hierarchy");
        }
        d
    };

    assert_eq!(depth(0), 0, "first group must be a cohort root");
    assert_eq!(
        depth(params.cohorts as usize),
        1,
        "classes sit one level below cohorts"
    );
    assert_eq!(
        depth((params.cohorts + params.cohorts * params.classes_per_cohort) as usize),
        2,
        "seminars sit two levels below cohorts"
    );
}
