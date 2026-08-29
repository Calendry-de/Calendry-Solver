//! Lecturer-pool selection (issue #61): a genuine choice among candidates,
//! not the degenerate pre-assigned case. Three angles: construction actually
//! makes a choice, the choice respects `LecturerDoubleBooking`, and
//! `PersonPreferenceFit`'s dynamic path prices different choices differently.

use calendry_solver_core::ids::PersonIdx;
use calendry_solver_core::problem::{Person, Problem, ProblemSpec};
use calendry_solver_core::search::construct;
use calendry_solver_core::testing::{
    all_constraints, fixture, grid, offering, person, person_with_preference, preference,
    preference_rule, room, with_lecturer_pool, with_preference,
};

#[test]
fn construction_chooses_a_real_lecturer_from_the_pool() {
    // 1 slot, 1 room, 1 Offering needing 1 of 2 candidates.
    let o = with_lecturer_pool(offering("o", 1, &[0]), 1, &[0, 1]);
    let mut spec = ProblemSpec {
        rooms: vec![room("r0")],
        persons: vec![person("p0", &[]), person("p1", &[])],
        offerings: vec![o],
        ..fixture(grid(1, 1), all_constraints())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let (solution, _) = construct(&problem);
    let placed = solution
        .get(problem.placement_ids().next().unwrap())
        .expect("placeable");
    assert_eq!(
        placed.lecturers.iter().flatten().count(),
        1,
        "exactly one of the two candidates must be chosen"
    );
}

#[test]
fn pool_selection_avoids_a_lecturer_double_booking() {
    // 1 slot, 2 Rooms — Room occupancy can never force these apart, only a
    // shared candidate pool with LecturerDoubleBooking enabled can. Both
    // Offerings need 1 lecturer from the SAME 2-candidate pool, so the only
    // way both get placed in the single available slot is by picking
    // DIFFERENT candidates.
    let a = with_lecturer_pool(offering("a", 1, &[0]), 1, &[0, 1]);
    let b = with_lecturer_pool(offering("b", 1, &[1]), 1, &[0, 1]);
    let mut spec = ProblemSpec {
        rooms: vec![room("r0"), room("r1")],
        persons: vec![person("p0", &[]), person("p1", &[])],
        offerings: vec![a, b],
        ..fixture(grid(1, 1), all_constraints())
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let (solution, _) = construct(&problem);
    let a_pl = solution
        .get(problem.placement_ids().next().unwrap())
        .expect("a is placeable");
    let b_pl = solution
        .get(problem.placement_ids().nth(1).unwrap())
        .expect("b is placeable");
    let a_chosen: Vec<PersonIdx> = a_pl.lecturers.iter().flatten().copied().collect();
    let b_chosen: Vec<PersonIdx> = b_pl.lecturers.iter().flatten().copied().collect();
    assert_ne!(
        a_chosen, b_chosen,
        "both Offerings share the single slot, so they must pick different lecturers"
    );
}

#[test]
fn preference_cost_for_a_pool_placement_depends_on_which_lecturer_is_chosen() {
    // p0 wants block 0; p1 wants block 1 — opposite of each other, so a
    // candidate landing in block 0 is a perfect fit for one and a total miss
    // for the other.
    let persons: Vec<Person> = vec![
        person_with_preference("p0", &[], preference(&[], &[0], None)),
        person_with_preference("p1", &[], preference(&[], &[1], None)),
    ];
    let o = with_lecturer_pool(offering("o", 1, &[0]), 1, &[0, 1]);
    let mut spec = ProblemSpec {
        rooms: vec![room("r0")],
        persons,
        offerings: vec![o],
        ..fixture(grid(2, 1), with_preference(vec![preference_rule("c-pref", 4.0)]))
    };
    spec.expand_placements();
    let problem = Problem::build(spec).unwrap();

    let p = problem.placement_ids().next().unwrap();
    let block0 = problem.slots.resolve(0, 1, 0).unwrap();
    let room_features: Vec<String> = vec![];

    let p0_only = [Some(PersonIdx(0)), None, None, None];
    let p1_only = [Some(PersonIdx(1)), None, None, None];

    let cost_p0 = problem
        .preferences
        .cost_for(p, &p0_only, block0, &room_features);
    let cost_p1 = problem
        .preferences
        .cost_for(p, &p1_only, block0, &room_features);

    assert!(
        cost_p0 < cost_p1,
        "p0's preference is met in block 0, p1's is not: {cost_p0} vs {cost_p1}"
    );
    assert_eq!(cost_p0, 0.0, "a perfectly met preference costs nothing");
    assert!(cost_p1 > 0.0, "an unmet preference must cost something");
}
