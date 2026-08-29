//! Generator tests.
//!
//! These deliberately assert only that generated instances are **well-formed
//! and reproducible**. They never assert that the solver's answer on a generated
//! instance is correct — that is what the hand-written fixtures in
//! `calendry_solver_core::testing` are for, and a generator that graded its own
//! output would be a bug that silently validates itself.

use calendry_solver_core::problem::Problem;
use calendry_solver_gen::{Preset, TARGET_SATURATION, digest, generate, person_clique};

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

/// How far the closed-form saturation may sit from the measured value.
///
/// See the assertion in `presets_are_calibrated_into_the_saturation_band` for
/// why it is this loose and why it exists at all.
const PREDICTION_TOLERANCE: f64 = 0.20;

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

        // The closed form and the measurement must AGREE, not merely each land
        // in the band. Asserting only the two bands lets predicted 0.56 pass
        // against measured 0.74 — the model badly wrong, the test green. The
        // model has already been off by 1.28x at school scale and 1.55x at
        // university scale, and until then the only thing watching for it was a
        // human reading the harness printout.
        //
        // 20% is deliberately loose: the closed form is a calibration proxy, not
        // a prediction, and a tight bound here would fail on ordinary
        // round-robin quantisation. It is tight enough to catch a model that has
        // stopped describing the generator.
        assert!(
            s.prediction_error.abs() <= PREDICTION_TOLERANCE,
            "{}: closed form {:.3} is {:+.1}% off measured {:.3}, beyond ±{:.0}% — \
             the calibration model no longer describes the generator",
            preset.name(),
            s.predicted_saturation,
            s.prediction_error * 100.0,
            s.saturation,
            PREDICTION_TOLERANCE * 100.0
        );

        // The guard that slice 5's calibration lacked. A load figure cannot see
        // pairwise-conflicting attendee sets, so every axis above can read "in
        // band" while the instance is provably unplaceable. Anything at or above
        // 1.0 here is a certificate of infeasibility, not a difficulty setting.
        assert!(
            s.person_clique_load < 1.0,
            "{}: person-clique load {:.3} over {} pairwise-conflicting Offerings \
             means the instance cannot be placed at all",
            preset.name(),
            s.person_clique_load,
            s.person_clique_size
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
            && p.groups
                .iter()
                .enumerate()
                .any(|(i, &a)| p.groups[i + 1..].iter().any(|&b| !closure.conflicts(a, b)))
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
        let o = f
            .offering
            .expect("a generated lock always realizes an Offering");
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
    assert_eq!(depth(params.cohorts as usize), 1, "classes sit one level below cohorts");
    assert_eq!(
        depth((params.cohorts + params.cohorts * params.classes_per_cohort) as usize),
        2,
        "seminars sit two levels below cohorts"
    );
}

// ---------------------------------------------------------------------------
// The person-axis feasibility metric
// ---------------------------------------------------------------------------

/// Build a tiny Problem whose Offerings all share `shared` attendees.
///
/// With `shared > 0` every pair conflicts under `PersonDoubleBooking`, so the
/// Offerings are mutually exclusive in time and their Sessions need one slot
/// each. With `shared == 0` they are independent and can be stacked.
fn overlapping_problem(offerings: u32, sessions: u32, shared: usize) -> Problem {
    use calendry_solver_core::problem::{
        OfferingSpec, Person, ProblemBuilder, Room, SchedulingPattern,
    };
    use calendry_solver_core::slots::{SlotTable, WeekKind, WeekSpec};

    // Built through `ProblemBuilder`, which hands back the typed index of
    // everything it inserts. This function used to construct `Room` and `Person`
    // field-by-field and then index into those vectors with raw `u32`s, which is
    // the hazard the builder removes: `OfferingSpec.participants` is a list of
    // indices into a vector the caller has not finished building, and a stale one
    // is not a reportable error — `Problem::build` declares only `GroupCycle`, so
    // a dangling index panics on a raw slice index inside closure derivation.
    let mut b = ProblemBuilder::new(
        // 4 slots.
        SlotTable::build(
            4,
            &[1],
            &[WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] }],
        )
        .unwrap(),
    );

    let room = b.room(Room {
        id: "r0".into(),
        name: "r0".into(),
        capacity: 999,
        rank: 1,
        is_virtual: false,
        features: vec![],
        federation_owned: false,
    });

    let person = |i: usize| Person {
        id: format!("p{i}"),
        role_tags: vec![],
        groups: vec![],
        blackouts: vec![],
        preferred: None,
    };

    // `shared` people common to every Offering...
    let common: Vec<_> = (0..shared).map(|i| b.person(person(i))).collect();

    for i in 0..offerings as usize {
        // ...then one private person each.
        let private = b.person(person(shared + i));
        let mut participants = common.clone();
        participants.push(private);

        b.offering(OfferingSpec {
            id: format!("o{i}"),
            kind: "lecture".into(),
            required_session_count: sessions,
            duration_blocks: 1,
            lecturers: vec![],
            groups: vec![],
            participants,
            eligible_rooms: vec![room],
            required_room_count: 0,
            eligible_room_combinations: vec![],
            min_capacity: 0,
            scheduling_pattern: SchedulingPattern::Unspecified,
        });
    }

    b.expand_placements();
    b.build().unwrap()
}

#[test]
fn the_clique_metric_detects_mutually_exclusive_offerings() {
    // This is the falsification test for the metric itself. Before slice 6a the
    // generator produced instances where cohort lectures pairwise shared an
    // attendee, needing one slot each — 1146 Sessions against 350 slots. Every
    // load metric reported "in band" because each individual row was quiet, and
    // the impossible instance was certified as a valid benchmark.
    //
    // 3 Offerings x 2 Sessions = 6 blocks, all pairwise conflicting, against a
    // 4-slot term.
    let problem = overlapping_problem(3, 2, 1);
    let (size, blocks) = person_clique(&problem);

    assert_eq!(size, 3, "all three Offerings share an attendee");
    assert_eq!(blocks, 6, "6 Sessions of 1 block each");
    assert!(
        blocks as f64 / problem.slots.len() as f64 > 1.0,
        "6 mutually-exclusive blocks in a 4-slot term must exceed 1.0"
    );
}

#[test]
fn the_clique_metric_does_not_fire_on_independent_offerings() {
    // The control. A metric that flagged everything would be useless, and would
    // block calibration rather than guard it.
    let problem = overlapping_problem(3, 2, 0);
    let (size, blocks) = person_clique(&problem);

    assert_eq!(size, 1, "no shared attendees, so no conflicting pair");
    assert_eq!(blocks, 2, "the bound sees a single Offering's 2 Sessions");
    assert!(blocks as f64 / problem.slots.len() as f64 <= 1.0);
}

#[test]
fn electives_do_not_pull_students_into_another_cohorts_subtree() {
    // The 6a fix, asserted structurally rather than via its symptom.
    //
    // An elective group must be a ROOT. Parenting it under a Cohort puts every
    // enrolled student into that Cohort's subtree, making them an attendee of
    // its cohort-wide lectures — which is what welded two Cohorts' lecture
    // series together and made the instances infeasible.
    let params = Preset::SmallUniversity.params();
    let instance = generate(&params, 4);
    let problem = &instance.problem;

    let n_elective = params.elective_groups();
    assert!(n_elective > 0, "this preset should have electives");

    let first = (params.group_count() - n_elective) as usize;
    for g in first..problem.groups.len() {
        assert!(
            problem.groups[g].parent.is_none(),
            "elective group '{}' must be a root",
            problem.groups[g].id
        );
    }

    // And the consequence: no student is an attendee of two different Cohorts'
    // lecture Offerings.
    let cohort_lectures: Vec<usize> = problem
        .offerings
        .iter()
        .enumerate()
        .filter(|(_, o)| o.kind == "lecture")
        .map(|(i, _)| i)
        .collect();

    let mut shared_pairs = 0;
    for (n, &a) in cohort_lectures.iter().enumerate() {
        for &b in cohort_lectures.iter().skip(n + 1).take(40) {
            let (x, y) = (&problem.offerings[a], &problem.offerings[b]);
            if x.own_groups != y.own_groups
                && x.attendees
                    .iter()
                    .any(|p| y.attendees.binary_search(p).is_ok())
            {
                shared_pairs += 1;
            }
        }
    }
    assert_eq!(shared_pairs, 0, "lectures of different Cohorts must not share attendees");
}
