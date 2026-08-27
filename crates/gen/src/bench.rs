//! Benchmark harness: generate an instance, then measure the search on it.
//!
//! A library module, not the binary. `src/bin/bench.rs` is now a `main` that
//! parses argv and calls [`run`]; everything else lives here so it has a **test
//! surface**. Nothing in this file used to be reachable from any test — an
//! integration test cannot link a binary — which is why the argument parser
//! could `panic!` on malformed input with nothing asserting that any flag was
//! even wired to its field.
//!
//! Reporting is separated from measurement: every `report_*` function appends to
//! a [`Report`] rather than calling `println!`, so the output is a value the
//! binary prints and a test can snapshot.
//!
//! ```text
//! cargo run --release -p calendry-solver-gen --bin bench -- [preset...] \
//!     [--seeds N] [--moves N] [--wall SECONDS] [--probe N]
//! ```
//!
//! # Why a plain binary and not `criterion`
//!
//! What is being measured is a **stateful whole-run metaheuristic**, not a pure
//! function. Criterion's model — call the same thing many times and take a
//! distribution — fits that badly, and most of the interesting quantities here
//! are counters (candidates enumerated versus scored, unplaced after
//! construction, objective against budget) rather than wall time.
//!
//! # The repair probe mirrors `repair_one`, it is not `repair_one`
//!
//! Splitting repair into enumerate / sample / score phases needs a clock, and
//! `calendry-solver-core` deliberately has none — that ban is what keeps the
//! search a pure, reproducible function. So the probe below reconstructs the
//! same three phases from the public API.
//!
//! That is a real duplication and it can drift: if `repair_one` changes shape,
//! this probe measures something that no longer exists. It is kept honest by
//! sharing the one constant that matters (`tuning::MAX_CANDIDATES`) and by
//! cross-checking against `SolveOutcome::candidates_enumerated`, which comes
//! from the actual search — if the probe and the counter disagree about
//! enumeration width, the probe has drifted.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use calendry_solver_core::evaluator::{CpuEvaluator, Move, MoveEvaluator, Score};
use calendry_solver_core::ids::PlacementIdx;
use calendry_solver_core::problem::Problem;
use calendry_solver_core::rng::Rng;
use calendry_solver_core::search::{self, Budget, Halt, tuning};
use calendry_solver_core::solution::{Placement, SearchState, Solution};

use crate::{InstanceStats, Preset, TARGET_SATURATION, generate};

/// The harness's output, accumulated rather than printed.
///
/// Reporting used to be ~40 `println!` calls interleaved with the measurement
/// they described, which made the report format unobservable to anything but a
/// human reading a terminal. As a value it is snapshot-testable, and the split
/// between "measure" and "render" becomes a real seam.
#[derive(Default, Debug)]
pub struct Report {
    text: String,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a formatted line. Infallible — writing to a `String` cannot fail,
    /// so no call site has to handle an error that cannot occur.
    pub fn line(&mut self, args: std::fmt::Arguments<'_>) {
        use std::fmt::Write as _;
        let _ = self.text.write_fmt(args);
        self.text.push('\n');
    }

    /// Append without a newline, for the few places that build a line in pieces.
    pub fn write(&mut self, args: std::fmt::Arguments<'_>) {
        use std::fmt::Write as _;
        let _ = self.text.write_fmt(args);
    }

    pub fn blank(&mut self) {
        self.text.push('\n');
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn into_string(self) -> String {
        self.text
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// What the harness was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub presets: Vec<Preset>,
    pub seeds: u64,
    pub moves: u64,
    pub wall: u64,
    pub probe: usize,
    /// The **instance** seed, kept separate from the solve seed so an instance
    /// can be held fixed while the search varies, and vice versa.
    pub gen_seed: u64,
    /// Generate and report only. Used to calibrate presets without paying for
    /// a search on an instance whose shape is still being tuned.
    pub calibrate: bool,
    /// Attribute construction failures to axes, over this many unplaced
    /// placements. 0 = skip. Implies skipping the solve.
    pub diagnose: usize,
    pub elective: Option<f64>,
    /// Attribute `evaluate_hard` across its four phases.
    pub evaluate: bool,
}

/// Why an argument list was rejected.
///
/// A typed error rather than a `panic!`. The parser used to `panic!("{name} needs
/// a number")` and `.expect("--elective needs a ratio")`, which is defensible in
/// a `main` — but it also meant no test could assert that any flag was wired to
/// its field at all, because none of this was reachable from a test.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArgError {
    #[error("{flag} needs a number")]
    NotANumber { flag: String },
    #[error("{flag} needs a ratio")]
    NotARatio { flag: String },
    #[error("unknown preset {preset:?}; known: {known}")]
    UnknownPreset { preset: String, known: String },
}

impl Default for Args {
    fn default() -> Self {
        Self {
            presets: Vec::new(),
            seeds: 1,
            moves: 200_000,
            wall: 120,
            probe: 32,
            gen_seed: 1,
            calibrate: false,
            diagnose: 0,
            elective: None,
            evaluate: false,
        }
    }
}

impl Args {
    /// Parse an explicit argument list — not `std::env::args`, so a test can
    /// supply one.
    ///
    /// An empty preset list means "every preset", which is the behaviour the
    /// binary has always had.
    pub fn parse<I, S>(argv: I) -> Result<Self, ArgError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut a = Self::default();
        let mut it = argv.into_iter().map(|s| s.as_ref().to_string());

        while let Some(arg) = it.next() {
            let mut num = |flag: &str| -> Result<u64, ArgError> {
                it.next()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| ArgError::NotANumber { flag: flag.to_string() })
            };
            match arg.as_str() {
                "--seeds" => a.seeds = num("--seeds")?,
                "--moves" => a.moves = num("--moves")?,
                "--wall" => a.wall = num("--wall")?,
                "--probe" => a.probe = num("--probe")? as usize,
                "--gen-seed" => a.gen_seed = num("--gen-seed")?,
                "--calibrate" => a.calibrate = true,
                "--diagnose" => a.diagnose = num("--diagnose")? as usize,
                "--evaluate" => a.evaluate = true,
                // Override the one parameter under test, so a sweep does not
                // need a preset edit and a rebuild per point.
                "--elective" => {
                    a.elective = Some(
                        it.next()
                            .and_then(|v| v.parse::<f64>().ok())
                            .ok_or_else(|| ArgError::NotARatio {
                                flag: "--elective".to_string(),
                            })?,
                    );
                }
                other => match Preset::parse(other) {
                    Some(p) => a.presets.push(p),
                    None => {
                        return Err(ArgError::UnknownPreset {
                            preset: other.to_string(),
                            known: Preset::ALL.map(Preset::name).join(", "),
                        });
                    }
                },
            }
        }
        if a.presets.is_empty() {
            a.presets = Preset::ALL.to_vec();
        }
        Ok(a)
    }
}

/// Generate, measure and report every preset the arguments name.
///
/// Returns the report rather than printing it, so the binary owns the only
/// `println!` in the harness and a test can assert on the text.
pub fn run(args: &Args) -> Report {
    let mut report = Report::new();
    let r = &mut report;

    r.line(format_args!(
        "build: {}   (the drift assertion in search::solve is debug-only; \
         a debug build measures the assertion, not the search)",
        if cfg!(debug_assertions) { "DEBUG" } else { "release" }
    ));

    for preset in &args.presets {
        let mut params = preset.params();
        if let Some(e) = args.elective {
            params.elective_ratio = e;
        }
        r.line(format_args!("\n{:=<78}", ""));
        r.line(format_args!("{}", preset.name()));
        r.line(format_args!("{:=<78}", ""));

        let t = Instant::now();
        let instance = generate(&params, args.gen_seed);
        let gen_time = t.elapsed();

        report_instance(r, &instance.stats, gen_time);
        if args.calibrate {
            continue;
        }

        for seed in 0..args.seeds {
            if args.seeds > 1 {
                r.line(format_args!("\n-- solve seed {seed} --"));
            }
            run_phases(r, &instance.problem, &instance.stats, seed, args);
        }
    }
    report
}

fn report_instance(r: &mut Report, s: &InstanceStats, gen_time: Duration) {
    let band = if TARGET_SATURATION.contains(&s.saturation) {
        "in band"
    } else {
        "OUT OF BAND"
    };
    r.line(format_args!("generated in {gen_time:>8.2?}"));
    r.line(format_args!(
        "  grid       {} slots, {} rooms ({} virtual)",
        s.slots, s.rooms, s.virtual_rooms
    ));
    r.line(format_args!(
        "  entities   {} groups, {} persons, {} offerings",
        s.groups, s.persons, s.offerings
    ));
    r.line(format_args!(
        "  demand     {} placements + {} locked, {} block-slots",
        s.placements, s.fixed, s.total_demand_blocks
    ));
    r.line(format_args!(
        "  saturation {:.3} measured / {:.3} predicted ({:+.1}%)   [{}] target {:?}",
        s.saturation,
        s.predicted_saturation,
        s.prediction_error * 100.0,
        band,
        TARGET_SATURATION
    ));
    r.line(format_args!(
        "    by axis    group {:.3}   lecturer {:.3}   room {:.3}   \
         person-clique {:.3} (|C|={})",
        s.max_group_load,
        s.max_lecturer_load,
        s.room_tightness,
        s.person_clique_load,
        s.person_clique_size
    ));
    if s.person_clique_load > 1.0 {
        r.line(format_args!(
            "               ^^ PROVABLY INFEASIBLE: {} pairwise-conflicting Offerings \
             need more slots than the term has",
            s.person_clique_size
        ));
    }
    r.line(format_args!(
        "  eligible   mean {:.1} rooms, max {}",
        s.mean_eligible_rooms, s.max_eligible_rooms
    ));
    r.line(format_args!("  attendees  mean {:.1}, max {}", s.mean_attendees, s.max_attendees));
    r.line(format_args!(
        "  H1 width   {:.0} candidates per repair, sampled to {} ({:.0}x waste)",
        s.mean_candidates,
        tuning::MAX_CANDIDATES,
        s.mean_candidates / tuning::MAX_CANDIDATES as f64
    ));
}

fn run_phases(r: &mut Report, problem: &Problem, stats: &InstanceStats, seed: u64, args: &Args) {
    // --- construction -------------------------------------------------------
    let t = Instant::now();
    let (solution, mut state) = search::construct(problem);
    let construct_time = t.elapsed();

    let placed = solution.placed_count();
    let unplaced = solution.len() - placed;
    r.line(format_args!(
        "\nconstruct  {:>8.2?}   placed {placed}/{}, unplaced {unplaced}",
        construct_time,
        solution.len()
    ));
    r.line(format_args!(
        "  H2 multiplier: every LNS iteration retries all {unplaced} unplaced \
         placements, so the expected per-iteration enumeration is\n             \
         ({unplaced} + k) x {:.0} = {:.3e} candidates",
        stats.mean_candidates,
        (unplaced as f64 + 4.5) * stats.mean_candidates
    ));

    if args.diagnose > 0 {
        let t = Instant::now();
        let d = crate::diagnose::diagnose(problem, &solution, &state, args.diagnose);
        report_diagnosis(r, &d, t.elapsed());
        return;
    }

    // --- repair probe -------------------------------------------------------
    probe_repair(r, problem, &solution, &mut state, args.probe, seed);

    // --- full solve ---------------------------------------------------------
    let halt = BenchHalt {
        deadline: Instant::now() + Duration::from_secs(args.wall),
        samples: Mutex::new(Vec::new()),
    };
    let budget = Budget { max_wall_millis: 0, max_moves: args.moves };

    let t = Instant::now();
    let outcome = search::solve(problem, seed, budget, &halt);
    let solve_time = t.elapsed();

    let waste = if outcome.moves_evaluated == 0 {
        0.0
    } else {
        outcome.candidates_enumerated as f64 / outcome.moves_evaluated as f64
    };
    let per_iter = if outcome.iterations == 0 {
        0.0
    } else {
        outcome.candidates_enumerated as f64 / outcome.iterations as f64
    };

    r.line(format_args!("\nsolve      {:>8.2?}   [{}]", solve_time, outcome.termination_reason));
    r.line(format_args!(
        "  iterations {}  ({:.1}/s), accepted {}",
        outcome.iterations,
        outcome.iterations as f64 / solve_time.as_secs_f64().max(1e-9),
        outcome.moves_accepted
    ));
    r.line(format_args!(
        "  moves      scored {}, enumerated {}  -> {waste:.1}x enumeration waste",
        outcome.moves_evaluated, outcome.candidates_enumerated
    ));
    r.line(format_args!(
        "  per iter   {per_iter:.3e} candidates enumerated  (mean_candidates x {:.1})",
        per_iter / stats.mean_candidates.max(1.0)
    ));
    let total = outcome.objective.total(problem.hard_penalty);

    r.line(format_args!(
        "  objective  total {:.1}  = unplaced {} + aggregate {} + soft {:.1} + day_mix {:.1}",
        total,
        outcome.objective.unplaced,
        outcome.objective.aggregate,
        outcome.objective.soft,
        outcome.objective.day_mix_cost,
    ));

    /*
     * WHAT `ruin_worst` CAN SEE, printed because it is now a smaller share than
     * it was.
     *
     * `ruin_worst` ranks placements by `problem.soft.cost(...)` alone — the
     * unary table. That already missed the hard side of the objective (a
     * tracked issue: it scores soft while `aggregate x hard_penalty` dominates
     * the total). OnlineOnsiteSameDay becoming soft ADDS a term it also cannot
     * see, because a mixed day belongs to a (group, day) cell and not to any
     * one placement, so there is nothing to rank a placement by.
     *
     * Printed as a ratio rather than described in a comment somewhere, so the
     * number moves when the code does.
     */
    let visible = outcome.objective.soft;
    r.line(format_args!(
        "  ruin_worst sees {:.1} of {:.1}  ({:.4}% of the objective; day_mix {:.1} is \
         invisible to it)",
        visible,
        total,
        if total > 0.0 { visible / total * 100.0 } else { 0.0 },
        outcome.objective.day_mix_cost,
    ));
    r.line(format_args!("  violations {}", outcome.hard_violations.len()));

    // Attribute solve time to its three parts.
    //
    // `solve` re-runs construction internally, so the harness's own construct
    // timing above measures the identical deterministic work; `evaluate_hard`
    // runs once at the end and is re-timed here. Whatever is left is the LNS
    // loop. Without this split, construction's cost is invisible inside a single
    // "solve" number and gets misread as search cost — which is exactly the
    // mistake this harness exists to prevent.
    let t = Instant::now();
    let violations = calendry_solver_core::constraints::evaluate_hard(problem, &outcome.solution);
    let eval_time = t.elapsed();
    let lns_time = solve_time
        .saturating_sub(eval_time)
        .saturating_sub(construct_time);
    let share = |d: Duration| 100.0 * d.as_secs_f64() / solve_time.as_secs_f64().max(1e-9);
    r.line(format_args!(
        "  of which   construct {:>9.2?} ({:>2.0}%) | evaluate_hard {:>9.2?} ({:>2.0}%) | \
         LNS {:>9.2?} ({:>2.0}%)",
        construct_time,
        share(construct_time),
        eval_time,
        share(eval_time),
        lns_time,
        share(lns_time),
    ));
    r.line(format_args!("  recheck    {} violations from a fresh evaluate_hard", violations.len()));

    if args.evaluate {
        attribute_evaluate(r, problem, &outcome.solution);
    }

    report_curve(r, &halt, args.moves);
}

/// Records the best-so-far trace. `Halt::report` fires only on improvement, so
/// this is the improvement curve rather than a uniform sample.
struct BenchHalt {
    deadline: Instant,
    samples: Mutex<Vec<(u64, f64)>>,
}

impl Halt for BenchHalt {
    fn should_stop(&self) -> Option<&'static str> {
        if Instant::now() >= self.deadline { Some("wall_clock") } else { None }
    }

    fn report(&self, objective: f64, moves: u64) {
        self.samples.lock().unwrap().push((moves, objective));
    }
}

fn report_curve(r: &mut Report, halt: &BenchHalt, budget: u64) {
    let samples = halt.samples.lock().unwrap();
    if samples.is_empty() {
        r.line(format_args!("  curve      no improvement recorded"));
        return;
    }
    let last = samples.last().unwrap().0.max(budget);
    r.write(format_args!("  curve     "));
    for pct in [10u64, 25, 50, 100] {
        let cutoff = last * pct / 100;
        let best = samples
            .iter()
            .take_while(|(m, _)| *m <= cutoff)
            .last()
            .map(|(_, o)| *o);
        match best {
            Some(o) => r.write(format_args!(" {pct:>3}%={o:.1}")),
            None => r.write(format_args!(" {pct:>3}%=-")),
        }
    }
    r.line(format_args!("   ({} improvements)", samples.len()));
}

// ---------------------------------------------------------------------------
// Repair probe
// ---------------------------------------------------------------------------

fn probe_repair(
    r: &mut Report,
    problem: &Problem,
    solution: &Solution,
    state: &mut SearchState,
    samples: usize,
    seed: u64,
) {
    let mut rng = Rng::new(seed ^ 0x5eed_0000);

    // Sample from the UNPLACED placements when there are any.
    //
    // Those are what LNS actually repairs every iteration, and they are not a
    // random draw from the population: construction failed on them precisely
    // because they are the constrained ones. Probing placed Sessions instead
    // understated per-repair cost by more than an order of magnitude.
    let unplaced: Vec<PlacementIdx> = problem
        .placement_ids()
        .filter(|&p| solution.get(p).is_none())
        .collect();
    let placed: Vec<PlacementIdx> = problem
        .placement_ids()
        .filter(|&p| solution.get(p).is_some())
        .collect();

    let (pool, label) =
        if unplaced.is_empty() { (&placed, "placed") } else { (&unplaced, "unplaced") };
    if pool.is_empty() {
        r.line(format_args!("\nprobe      nothing to repair, skipped"));
        return;
    }

    let mut select = Duration::ZERO;
    let mut score = Duration::ZERO;
    let mut enumerated_total = 0u64;
    let mut scored_total = 0u64;
    let mut rooms_total = 0u64;
    let mut n = 0u64;

    // Sampling with replacement keeps the same few placements' data hot in
    // cache, which flatters the measurement. When asked for at least as many
    // samples as there are repairs, replay the pool in index order instead —
    // that is exactly what `ruin` hands the repair loop.
    let replay = samples >= pool.len();
    for s in 0..samples.min(pool.len().max(1) * if replay { 1 } else { usize::MAX }) {
        let p = if replay { pool[s] } else { pool[rng.below(pool.len())] };
        let offering = problem.offering_of(p);

        // A placed sample has to be removed first, or the probe measures a state
        // that never occurs: LNS only ever scores placements it already ruined.
        let restore = solution.get(p).filter(|&pl| state.unplace(problem, p, pl));
        let mut trial = solution.clone();
        trial.set(p, None);

        // Mirrors `repair_one`'s candidate selection: addressed by index, never
        // materialized. Kept structurally identical so the split stays honest.
        let t = Instant::now();
        let n_rooms = offering.eligible_rooms.len();
        let n_starts = problem.slots.start_count(offering.duration_blocks);
        let total = n_starts * n_rooms;
        let keep = total.min(tuning::MAX_CANDIDATES);
        let mut candidates: Vec<Move> = Vec::with_capacity(keep);
        let at = |i: usize| Move {
            placement: p,
            to: Placement {
                start: problem
                    .slots
                    .nth_start(offering.duration_blocks, i / n_rooms.max(1))
                    .unwrap_or(
                        problem
                            .slots
                            .nth_start(offering.duration_blocks, 0)
                            .unwrap(),
                    ),
                room: offering.eligible_rooms[i % n_rooms.max(1)],
            },
        };
        if total <= tuning::MAX_CANDIDATES {
            candidates.extend((0..total).map(at));
        } else {
            let mut moved: std::collections::HashMap<usize, usize> =
                std::collections::HashMap::with_capacity(keep);
            for i in 0..keep {
                let j = i + rng.below(total - i);
                let picked = moved.get(&j).copied().unwrap_or(j);
                let displaced = moved.get(&i).copied().unwrap_or(i);
                candidates.push(at(picked));
                moved.insert(j, displaced);
            }
            candidates.sort_by_key(|m| (m.to.start.get(), m.to.room.get()));
        }
        select += t.elapsed();
        enumerated_total += total as u64;
        scored_total += candidates.len() as u64;
        rooms_total += n_rooms as u64;

        let t = Instant::now();
        let mut scores = vec![Score::default(); candidates.len()];
        CpuEvaluator.score_batch(problem, &trial, state, &candidates, &mut scores);
        score += t.elapsed();

        if let Some(pl) = restore {
            let marked = state.place(problem, p, pl);
            debug_assert!(marked, "the probe just released this exact placement");
        }
        n += 1;
    }

    if n == 0 {
        r.line(format_args!("\nprobe      no usable samples"));
        return;
    }

    let total = select + score;
    let pct = |d: Duration| 100.0 * d.as_secs_f64() / total.as_secs_f64().max(1e-12);
    r.line(format_args!(
        "\nprobe      {n} {label} repairs, space {} -> {} scored, {} eligible rooms",
        enumerated_total / n,
        scored_total / n,
        rooms_total / n
    ));
    r.line(format_args!("  select    {:>10.2?}/repair  {:>5.1}%", select / n as u32, pct(select)));
    r.line(format_args!("  score     {:>10.2?}/repair  {:>5.1}%", score / n as u32, pct(score)));
    r.line(format_args!("  total     {:>10.2?}/repair", total / n as u32));
}

// ---------------------------------------------------------------------------
// Construction failure diagnosis
// ---------------------------------------------------------------------------

fn report_diagnosis(r: &mut Report, d: &crate::diagnose::ConstructionFailure, took: Duration) {
    let pool = if d.pool_was_unplaced { "unplaced" } else { "placed" };
    r.line(format_args!(
        "\ndiagnose   {:.2?}   {} {pool} placements examined ({} unplaced in total)",
        took, d.sampled, d.total_unplaced
    ));
    if d.slots_examined > 0 {
        let share = 100.0 * d.slots_blocked_room_independent as f64 / d.slots_examined as f64;
        r.line(format_args!(
            "  scan cost  {} of {} start slots ({share:.1}%) rejected by a room-INDEPENDENT axis",
            d.slots_blocked_room_independent, d.slots_examined
        ));
        r.line(format_args!(
            "             {} of {} probes wasted re-testing them per room ({:.1}%)",
            d.wasted_probes,
            d.totals.candidates,
            100.0 * d.wasted_probes as f64 / d.totals.candidates.max(1) as f64
        ));
    }
    if d.total_unplaced == 0 {
        r.line(format_args!("  (construction placed everything; no failure to attribute)"));
        return;
    }
    r.line(format_args!(
        "  profile    mean {:.1} eligible rooms, kinds {:?}",
        d.mean_eligible_rooms, d.by_kind
    ));

    let t = &d.totals;
    let pct = |v: u64| 100.0 * v as f64 / t.candidates.max(1) as f64;
    r.line(format_args!(
        "  space      {} candidates examined, {} free ({:.4}%)",
        t.candidates,
        t.free,
        pct(t.free)
    ));
    r.line(format_args!("  blocked by (a candidate may be blocked by several at once):"));
    for (name, v) in [
        ("group", t.blocked_group),
        ("person", t.blocked_person),
        ("room", t.blocked_room),
        ("lecturer", t.blocked_lecturer),
        ("veto", t.blocked_veto),
    ] {
        r.line(format_args!("    {name:<9} {v:>12}  {:>6.2}%", pct(v)));
    }
    // Separate line, separate meaning: since OnlineOnsiteSameDay became soft it
    // never blocks a candidate, it prices one. Printed under "blocked by" it
    // would read as a filter that never binds.
    r.line(format_args!(
        "  priced (not blocked):\n    day_mix   {:>12}  {:>6.2}%",
        t.day_mix_priced,
        pct(t.day_mix_priced)
    ));

    r.line(format_args!(
        "  free space {} of {} sampled placements have somewhere to go",
        d.with_free_space, d.sampled
    ));
    let labels = ["0", "1-9", "10-99", "100-999", "1000+"];
    r.write(format_args!("    histogram"));
    for (l, n) in labels.iter().zip(&d.free_buckets) {
        r.write(format_args!("  {l}={n}"));
    }
    r.blank();

    if let Some(c) = &d.clique {
        report_clique(r, c);
    }

    if d.with_free_space > 0 {
        r.line(format_args!(
            "    free ratio min {:.5}%, median {:.5}%",
            100.0 * d.min_free_ratio,
            100.0 * d.median_free_ratio
        ));
        // The number that decides whether LNS can ever recover these: repair
        // samples MAX_CANDIDATES out of the space uniformly, so the chance of
        // even seeing a free candidate is 1 - (1 - ratio)^MAX_CANDIDATES.
        let hit = |r: f64| 100.0 * (1.0 - (1.0 - r).powi(tuning::MAX_CANDIDATES as i32));
        r.line(format_args!(
            "    P(repair's {}-sample sees a free candidate): min {:.1}%, median {:.1}%",
            tuning::MAX_CANDIDATES,
            hit(d.min_free_ratio),
            hit(d.median_free_ratio)
        ));
    }
}

fn report_clique(r: &mut Report, c: &crate::diagnose::CliqueEvidence) {
    r.line(format_args!(
        "  clique     kind '{}': {:.1}% of {} sampled Offering pairs share an attendee",
        c.kind,
        100.0 * c.conflict_density,
        c.pairs_sampled
    ));
    r.line(format_args!(
        "             {} Sessions of this kind vs {} non-overlapping slots",
        c.sessions, c.capacity
    ));
    if c.conflict_density > 0.9 && c.sessions > c.capacity {
        r.line(format_args!(
            "             => INFEASIBLE BY COUNTING: near-clique needs one slot each,\n\
             \x20            so at most {} of {} can ever be placed. Not an algorithm problem.",
            c.capacity, c.sessions
        ));
    }
}

// ---------------------------------------------------------------------------
// evaluate_hard attribution
// ---------------------------------------------------------------------------

fn attribute_evaluate(r: &mut Report, problem: &Problem, solution: &Solution) {
    use calendry_solver_core::constraints as k;

    r.line(format_args!("\nevaluate_hard attribution"));

    // --- phase split ------------------------------------------------------
    let phase = |name: &str, f: &dyn Fn(&mut Vec<k::Violation>)| {
        let mut out = Vec::new();
        let t = Instant::now();
        f(&mut out);
        (name.to_string(), t.elapsed(), out)
    };

    let phases = [
        phase("exact_frequency", &|o| k::exact_frequency(problem, solution, o)),
        phase("structural", &|o| k::structural(problem, solution, o)),
        phase("lecturer_veto", &|o| k::lecturer_veto(problem, solution, o)),
        phase("aggregates", &|o| k::aggregates(problem, solution, o)),
    ];
    let total: Duration = phases.iter().map(|(_, d, _)| *d).sum();
    for (name, d, out) in &phases {
        r.line(format_args!(
            "  {name:<16} {:>10.2?}  {:>5.1}%   {} violations",
            d,
            100.0 * d.as_secs_f64() / total.as_secs_f64().max(1e-12),
            out.len()
        ));
    }
    r.line(format_args!("  {:<16} {:>10.2?}", "TOTAL", total));

    // --- can the search even create a structural violation? ---------------
    //
    // Repair only ever places into slots `SearchState::is_free` accepts, and
    // that state is seeded from the immovable input. So a structural violation
    // should only ever involve an immovable Session. If that holds, the
    // placed-vs-placed pairs — the overwhelming majority — can never report
    // anything, and the question is not how to compute the scan faster.
    let fixed_ids: std::collections::HashSet<&str> = problem
        .fixed
        .iter()
        .map(|f| f.session_id.as_str())
        .collect();

    let structural = &phases[1].2;
    let (mut both_fixed, mut one_fixed, mut neither) = (0usize, 0usize, 0usize);
    for v in structural {
        let n = v
            .session_ids
            .iter()
            .filter(|id| fixed_ids.contains(id.as_str()))
            .count();
        match n {
            2 => both_fixed += 1,
            1 => one_fixed += 1,
            _ => neither += 1,
        }
    }
    r.line(format_args!(
        "  structural violations by origin: both immovable {both_fixed}, \
         one immovable {one_fixed}, neither {neither}"
    ));

    // Pairs the scan examines, versus pairs that could possibly report.
    let placed = solution.placed_count() as u64;
    let fixed = problem.fixed.len() as u64;
    let all = placed + fixed;
    r.line(format_args!(
        "  occupancy: {placed} placed + {fixed} immovable = {all}; \
         immovable is {:.1}% of it, so immovable-involving pairs are ~{:.1}% of all pairs",
        100.0 * fixed as f64 / all.max(1) as f64,
        100.0 * (1.0 - (placed as f64 / all.max(1) as f64).powi(2))
    ));

    // --- is the cost in reporting, or in the unconditional scanning? ------
    //
    // `check_pair` runs all four clash searches and formats a location string
    // BEFORE consulting the configured instances. Emptying every constraint
    // list therefore leaves the scan intact and removes only the reporting.
    let mut bare = problem.clone();
    bare.constraints.room_double_booking.clear();
    bare.constraints.lecturer_double_booking.clear();
    bare.constraints.group_double_booking.clear();
    bare.constraints.person_double_booking.clear();

    let mut out = Vec::new();
    let t = Instant::now();
    k::structural(&bare, solution, &mut out);
    let bare_time = t.elapsed();
    r.line(format_args!(
        "  structural with ALL constraint lists emptied: {:>10.2?} ({:.1}% of the real run, \
         {} violations)",
        bare_time,
        100.0 * bare_time.as_secs_f64() / phases[1].1.as_secs_f64().max(1e-12),
        out.len()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- argument parsing ----------------------------------------------------
    //
    // None of this was testable before: it lived in `src/bin/` and panicked on
    // malformed input, so nothing asserted that any flag reached its field.

    #[test]
    fn no_arguments_means_every_preset() {
        let args = Args::parse::<[&str; 0], &str>([]).expect("empty argv is valid");
        assert_eq!(args.presets, Preset::ALL.to_vec());
    }

    #[test]
    fn a_named_preset_replaces_the_default_set() {
        let args = Args::parse(["small-school"]).expect("known preset");
        assert_eq!(args.presets, vec![Preset::SmallSchool]);
    }

    #[test]
    fn every_numeric_flag_reaches_its_field() {
        let args = Args::parse([
            "--seeds",
            "3",
            "--moves",
            "1234",
            "--wall",
            "7",
            "--probe",
            "9",
            "--gen-seed",
            "42",
            "--diagnose",
            "5",
        ])
        .expect("valid flags");

        assert_eq!(args.seeds, 3);
        assert_eq!(args.moves, 1234);
        assert_eq!(args.wall, 7);
        assert_eq!(args.probe, 9);
        assert_eq!(args.gen_seed, 42);
        assert_eq!(args.diagnose, 5);
    }

    #[test]
    fn boolean_flags_reach_their_fields() {
        let args = Args::parse(["--calibrate", "--evaluate"]).expect("valid flags");
        assert!(args.calibrate);
        assert!(args.evaluate);
    }

    #[test]
    fn the_elective_override_parses_as_a_ratio() {
        let args = Args::parse(["--elective", "0.35"]).expect("valid ratio");
        assert_eq!(args.elective, Some(0.35));
    }

    #[test]
    fn a_numeric_flag_without_a_number_is_refused_rather_than_panicking() {
        let err = Args::parse(["--moves", "lots"]).expect_err("not a number");
        assert_eq!(err, ArgError::NotANumber { flag: "--moves".into() });
    }

    #[test]
    fn a_missing_ratio_is_refused_rather_than_panicking() {
        let err = Args::parse(["--elective"]).expect_err("no ratio given");
        assert_eq!(err, ArgError::NotARatio { flag: "--elective".into() });
    }

    #[test]
    fn an_unknown_preset_lists_the_known_ones() {
        let err = Args::parse(["enormous-university"]).expect_err("unknown preset");
        let message = err.to_string();
        assert!(message.contains("enormous-university"), "{message}");
        assert!(message.contains("small-school"), "must list what is available: {message}");
    }

    // -- the report is a value -----------------------------------------------

    #[test]
    fn the_report_accumulates_instead_of_printing() {
        let mut r = Report::new();
        r.line(format_args!("first {}", 1));
        r.write(format_args!("second"));
        r.line(format_args!(" third"));
        assert_eq!(r.as_str(), "first 1\nsecond third\n");
    }

    /// The instance report's *shape* is pinned, so a change to it is deliberate.
    ///
    /// Snapshotting the numbers themselves would pin the generator's output,
    /// which is a different thing and already asserted in `tests/generator.rs`;
    /// this asserts the lines and labels a reader depends on.
    #[test]
    fn the_instance_report_carries_every_calibration_axis() {
        let instance = generate(&Preset::SmallSchool.params(), 1);
        let mut r = Report::new();
        report_instance(&mut r, &instance.stats, Duration::from_millis(1));

        let text = r.into_string();
        for label in [
            "grid",
            "entities",
            "demand",
            "saturation",
            "by axis",
            "person-clique",
            "eligible",
            "attendees",
            "H1 width",
        ] {
            assert!(text.contains(label), "report must name '{label}':\n{text}");
        }
    }
}
