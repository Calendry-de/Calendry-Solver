//! The benchmark parameter space, and the named presets built on top of it.
//!
//! # Saturation is the parameter that matters — and rooms are not the binding axis
//!
//! A generator that makes instances *big* rather than *hard* measures nothing.
//! Ten thousand Offerings against ten thousand Rooms is trivially feasible:
//! greedy construction succeeds on its first probe, LNS never has to repair
//! anything, and the repair cost you measured is the cost of a code path nothing
//! exercises.
//!
//! The obvious candidate for "how hard is this instance" is room tightness,
//! `demand_blocks / (rooms x slots)`. **It is the wrong quantity**, and
//! calibrating against it produces instances construction cannot solve at all.
//!
//! The reason is conflict propagation. A Room's row accumulates only the
//! Sessions actually placed in that Room. A **Group's** row accumulates every
//! Session of every Group in its conflict closure — so a Cohort is marked busy
//! by every Session of every Class and Seminar beneath it. Demand that spreads
//! across many Rooms piles onto a *single* Cohort row. For any realistic
//! hierarchy that row saturates first, by a large margin.
//!
//! So the calibrated quantity is the **binding axis**:
//!
//! ```text
//! saturation = max( demand / (rooms x slots),          // room axis
//!                   max_g  blocked_blocks(g) / slots,   // group axis  <- usually this
//!                   max_l  taught_blocks(l) / slots )   // lecturer axis
//! ```
//!
//! Presets are calibrated so this lands in [`TARGET_SATURATION`], and the
//! generator reports all three axes so which one binds is always visible rather
//! than assumed. Room tightness stays reported, as a secondary figure; at these
//! hierarchies it sits far below the band, which is correct and not a defect.
//!
//! Two further knobs create contention that no saturation figure expresses:
//!
//! * **`elective_ratio`** — the fraction of students who belong to a second
//!   Group *unrelated in the nesting tree*. This is the only thing that makes
//!   `PersonDoubleBooking` do work the Group check does not already do.
//! * **`feature_demand_ratio`** and room capacity — together these set
//!   `eligible_rooms`, which is one of the two factors in the repair
//!   enumeration width.

/// The band a preset's binding-axis saturation should land in: hard but
/// feasible.
///
/// Below it, construction succeeds greedily and the search is never tested.
/// Above it, instances become infeasible and every run degenerates into
/// reporting unplaced Sessions, which is also not a search measurement.
pub const TARGET_SATURATION: std::ops::RangeInclusive<f64> = 0.55..=0.75;

/// Named scale presets, built on top of [`InstanceParams`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Preset {
    SmallSchool,
    LargeSchool,
    SmallUniversity,
    LargeUniversity,
}

impl Preset {
    pub const ALL: [Preset; 4] = [
        Preset::SmallSchool,
        Preset::LargeSchool,
        Preset::SmallUniversity,
        Preset::LargeUniversity,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Preset::SmallSchool => "small-school",
            Preset::LargeSchool => "large-school",
            Preset::SmallUniversity => "small-university",
            Preset::LargeUniversity => "large-university",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.name() == s)
    }

    pub fn params(self) -> InstanceParams {
        match self {
            // A Gymnasium: eight year-groups, one building, few specialist
            // rooms, everybody in a fixed class for most of the week.
            Preset::SmallSchool => InstanceParams {
                weeks: 12,
                exam_weeks: vec![11],
                holiday_weeks: vec![6],
                active_days: vec![1, 2, 3, 4, 5],
                blocks_per_day: 8,

                cohorts: 8,
                classes_per_cohort: 2,
                seminars_per_class: 2,
                students_per_seminar: 10,
                elective_ratio: 0.10,

                lecturers: 10,
                blackout_ratio: 0.25,

                physical_rooms: 18,
                virtual_rooms: 2,
                premium_ratio: 0.15,
                feature_coverage: 0.35,

                offerings: 128,
                sessions_per_offering: 12,
                duration_blocks: (1, 2),
                group_level_mix: [0.10, 0.35, 0.55],
                feature_demand_ratio: 0.30,
                locked_ratio: 0.05,

                max_online_share: Some(0.30),
                soft_weight: 1.0,
            },

            // A large comprehensive school or Berufsschule.
            Preset::LargeSchool => InstanceParams {
                weeks: 12,
                exam_weeks: vec![10, 11],
                holiday_weeks: vec![6],
                active_days: vec![1, 2, 3, 4, 5],
                blocks_per_day: 9,

                cohorts: 16,
                classes_per_cohort: 3,
                seminars_per_class: 2,
                students_per_seminar: 10,
                elective_ratio: 0.15,

                lecturers: 19,
                blackout_ratio: 0.25,

                physical_rooms: 28,
                virtual_rooms: 3,
                premium_ratio: 0.15,
                feature_coverage: 0.35,

                offerings: 272,
                sessions_per_offering: 12,
                duration_blocks: (1, 2),
                group_level_mix: [0.10, 0.35, 0.55],
                feature_demand_ratio: 0.30,
                locked_ratio: 0.05,

                max_online_share: Some(0.30),
                soft_weight: 1.0,
            },

            // A faculty-scale university: 14-week term, longer teaching day,
            // one cohort per programme-year.
            Preset::SmallUniversity => InstanceParams {
                weeks: 14,
                exam_weeks: vec![12, 13],
                holiday_weeks: vec![7],
                active_days: vec![1, 2, 3, 4, 5],
                blocks_per_day: 10,

                cohorts: 30,
                classes_per_cohort: 4,
                seminars_per_class: 3,
                students_per_seminar: 12,
                elective_ratio: 0.25,

                lecturers: 43,
                blackout_ratio: 0.30,

                physical_rooms: 54,
                virtual_rooms: 6,
                premium_ratio: 0.12,
                feature_coverage: 0.30,

                offerings: 480,
                sessions_per_offering: 14,
                duration_blocks: (2, 2),
                group_level_mix: [0.15, 0.35, 0.50],
                feature_demand_ratio: 0.35,
                locked_ratio: 0.05,

                max_online_share: Some(0.30),
                soft_weight: 1.0,
            },

            // A full university, Saturday teaching, 12 blocks/day.
            Preset::LargeUniversity => InstanceParams {
                weeks: 14,
                exam_weeks: vec![12, 13],
                holiday_weeks: vec![7],
                active_days: vec![1, 2, 3, 4, 5, 6],
                blocks_per_day: 12,

                cohorts: 80,
                classes_per_cohort: 5,
                seminars_per_class: 3,
                students_per_seminar: 14,
                elective_ratio: 0.30,

                lecturers: 118,
                blackout_ratio: 0.30,

                physical_rooms: 130,
                virtual_rooms: 10,
                premium_ratio: 0.10,
                feature_coverage: 0.30,

                offerings: 1920,
                sessions_per_offering: 14,
                duration_blocks: (2, 2),
                group_level_mix: [0.15, 0.35, 0.50],
                feature_demand_ratio: 0.35,
                locked_ratio: 0.05,

                max_online_share: Some(0.30),
                soft_weight: 1.0,
            },
        }
    }
}

/// The full parameter space. Every field is an explicit input; nothing about
/// the grid or the calendar is inferred by arithmetic.
#[derive(Clone, Debug)]
pub struct InstanceParams {
    // --- Time grid and academic calendar -----------------------------------
    pub weeks: u32,
    /// Week **indices** marked as exam weeks. Listed explicitly rather than
    /// derived as "the last n weeks" — the array-slicing shortcut is exactly
    /// the prototype bug this project exists to not repeat, and a generator
    /// that encodes it would quietly validate a solver that did too.
    pub exam_weeks: Vec<u32>,
    pub holiday_weeks: Vec<u32>,
    /// ISO weekdays, 1 = Monday.
    pub active_days: Vec<u32>,
    pub blocks_per_day: u32,

    // --- Group hierarchy (Cohort -> Class -> Seminar) -----------------------
    pub cohorts: u32,
    pub classes_per_cohort: u32,
    pub seminars_per_class: u32,
    pub students_per_seminar: u32,
    /// Fraction of students additionally enrolled in a seminar under a
    /// *different* cohort, making the two groups tree-unrelated.
    pub elective_ratio: f64,

    // --- Staff --------------------------------------------------------------
    pub lecturers: u32,
    /// Fraction of lecturers with a whole-day blackout.
    pub blackout_ratio: f64,

    // --- Space --------------------------------------------------------------
    pub physical_rooms: u32,
    /// Online delivery is a virtual Room, never a boolean flag.
    pub virtual_rooms: u32,
    /// Fraction of physical rooms at premium rank.
    pub premium_ratio: f64,
    /// Probability a physical room carries any given feature.
    pub feature_coverage: f64,

    // --- Demand -------------------------------------------------------------
    pub offerings: u32,
    pub sessions_per_offering: u32,
    /// Inclusive range, sampled uniformly.
    pub duration_blocks: (u32, u32),
    /// Fraction of Offerings attached at cohort / class / seminar level.
    pub group_level_mix: [f64; 3],
    pub feature_demand_ratio: f64,
    /// Fraction of occurrences arriving as immovable (locked) Sessions rather
    /// than as placements the solver may position.
    pub locked_ratio: f64,

    // --- Constraint configuration -------------------------------------------
    pub max_online_share: Option<f64>,
    pub soft_weight: f64,
}

impl InstanceParams {
    pub fn slots(&self) -> u64 {
        self.weeks as u64 * self.active_days.len() as u64 * self.blocks_per_day as u64
    }

    pub fn rooms(&self) -> u64 {
        self.physical_rooms as u64 + self.virtual_rooms as u64
    }

    pub fn seminar_count(&self) -> u32 {
        self.cohorts * self.classes_per_cohort * self.seminars_per_class
    }

    pub fn group_count(&self) -> u32 {
        self.cohorts + self.cohorts * self.classes_per_cohort + self.seminar_count()
    }

    pub fn student_count(&self) -> u32 {
        self.seminar_count() * self.students_per_seminar
    }

    fn mean_duration(&self) -> f64 {
        (self.duration_blocks.0 + self.duration_blocks.1) as f64 / 2.0
    }

    pub fn demand_blocks(&self) -> f64 {
        self.offerings as f64 * self.sessions_per_offering as f64 * self.mean_duration()
    }

    pub fn predicted_room_tightness(&self) -> f64 {
        self.demand_blocks() / (self.rooms() * self.slots()) as f64
    }

    /// The busiest group row, in closed form.
    ///
    /// A Cohort is blocked by its **entire subtree**, so its row carries that
    /// cohort's whole share of demand regardless of which level the Sessions
    /// were attached at. Cohorts are therefore always the busiest rows, and
    /// this reduces to demand-per-cohort.
    pub fn predicted_group_load(&self) -> f64 {
        self.demand_blocks() / self.cohorts.max(1) as f64 / self.slots() as f64
    }

    pub fn predicted_lecturer_load(&self) -> f64 {
        self.demand_blocks() / self.lecturers.max(1) as f64 / self.slots() as f64
    }

    /// Closed-form saturation on the binding axis, computable without
    /// generating anything. Used to calibrate presets cheaply; the generator
    /// reports the measured value next to it so the two stay visible together.
    pub fn predicted_saturation(&self) -> f64 {
        self.predicted_room_tightness()
            .max(self.predicted_group_load())
            .max(self.predicted_lecturer_load())
    }

    /// Total occurrences, before the locked fraction is split off.
    pub fn total_occurrences(&self) -> u64 {
        self.offerings as u64 * self.sessions_per_offering as u64
    }
}
