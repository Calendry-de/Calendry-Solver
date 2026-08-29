//! Lecturer-pool selection (issue #61) at the wire boundary: a placed
//! Session's `lecturer_ids` names whichever candidate the search actually
//! chose, not the whole pool and not nothing.

use calendry_solver::convert::{build_output, convert};
use calendry_solver_core::search::{Budget, NeverHalt, solve};
use calendry_solver_proto::v1 as pb;

mod common;
use common::{base_input, offering, person, scope};

const SEED: u64 = 0xC0FFEE;

fn budget() -> Budget {
    Budget { max_wall_millis: 0, max_moves: 20_000 }
}

#[test]
fn a_placed_pool_sessions_lecturer_ids_names_the_chosen_candidate() {
    let mut input = base_input();
    input.persons.push(person("p2"));
    input.offerings = vec![pb::Offering {
        candidate_lecturer_ids: vec!["p1".into(), "p2".into()],
        required_lecturer_count: 1,
        ..offering("o1", 1)
    }];

    let problem = convert(&input, &scope(&["o1"])).expect("a genuine pool is supported");
    let outcome = solve(&problem, SEED, budget(), &NeverHalt);
    let output = build_output(&problem, &outcome, 0);

    assert_eq!(output.sessions.len(), 1, "the one required Session must be placed");
    let session = &output.sessions[0];
    assert_eq!(
        session.lecturer_ids.len(),
        1,
        "exactly one of the two candidates must be chosen, got {:?}",
        session.lecturer_ids
    );
    assert!(
        session.lecturer_ids[0] == "p1" || session.lecturer_ids[0] == "p2",
        "the chosen lecturer must be one of the two candidates, got '{}'",
        session.lecturer_ids[0]
    );
}
