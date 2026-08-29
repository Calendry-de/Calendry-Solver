//! `PersonPreferenceFit`: the soft term that prices a placement against the
//! days and blocks its lecturers said they would rather have.
//!
//! # Why this is not a `SoftParams` variant
//!
//! [`crate::soft::SoftModel`] is a precomputed `(profile, slot, room)` table,
//! where a *profile* is the set of soft instances applying to one tenant `kind`.
//! A preference cost does not fit that key: it depends on **who leads this
//! placement**, which varies per placement rather than per kind. Keying it into
//! the profile dimension would mean one profile per distinct preference
//! signature — potentially one per placement — and the table stops being small.
//!
//! So this type gets its own list on [`crate::problem::ConstraintSet`] and its
//! own table, exactly as `OnlineOnsiteSameDay` did for the same reason.
//!
//! # Why the cost is nevertheless part of `Objective::soft`
//!
//! `day_mix_cost` is a separate `Objective` field because a mixed `(group, day)`
//! cell belongs to no single placement, so it can only be read whole off the
//! counters. A preference cost is the opposite: put this Session at that
//! `(day, block)` and its cost is determined — no other placement participates.
//! It is therefore delta-accumulated like the six slot-keyed soft types, which
//! buys three things:
//!
//! 1. the existing incremental-objective machinery and its debug drift
//!    assertion cover it unchanged;
//! 2. `ruin_worst` — which ranks placements by their soft contribution, and is
//!    for that reason blind to `day_mix_cost` and `aggregate` — can see it;
//! 3. no new objective field means no new place for the breakdown and the
//!    search to disagree.
//!
//! # The key, and what it depends on
//!
//! A preference has **no week axis** (it is a recurring weekly shape, not a
//! dated absence), so its cost is a function of `(day, block)` rather than of
//! the full slot. The table is `placement × weekly cell`, and at
//! `large-university` scale that is 27,136 × 41 ≈ 1.1 M `f32` (~4.5 MB). The
//! naive `placement × slot` table would be 27,136 × 924 ≈ 25 M entries
//! (~100 MB) for the same information, storing each `(day, block)` value once
//! per week of the term.
//!
//! The entry **already is the mean** over the placement's lecturers. It is
//! deliberately not a per-lecturer table combined at scoring time: that would
//! put an aggregation over the lecturer set inside the candidate loop, and the
//! solver's largest measured win to date (31× on construction) came from
//! hoisting an attendee scan out of a hot loop.
//!
//! **This collapse is only valid because a placement's lecturer set is fixed
//! before the search starts.** That holds today because genuine lecturer-*pool*
//! selection is unimplemented — the conversion layer accepts only the
//! degenerate case where the pool equals the requirement. If pool selection
//! ever lands, the lecturer set becomes a decision variable, the mean can no
//! longer be precomputed per placement, and this key is wrong: it would need
//! `(placement, chosen-lecturer-set, day, block)`, which is not a table. The
//! shape then is a per-person table `[person][day][block]` (small: people ×
//! days × blocks) with the mean taken over the chosen set at scoring time,
//! accepting an O(|P|) scoring step in exchange for a set that can change.
//!
//! That is a cross-repo coupling invisible from either side, and the failure
//! would be silent: a stale mean over whichever lecturers the Offering happened
//! to list first, still bounded, still plausible, quietly pricing the wrong
//! people's preferences.

use crate::ids::{PersonIdx, PlacementIdx, SlotIdx};
use crate::problem::{Offering, Person, PlacementVar};
use crate::slots::SlotTable;
use crate::solution::MAX_LECTURERS;

/// Bounds on a Person's weight override. The app validates the range at its
/// write boundary and a database CHECK backs it up; this service accepts
/// possibly-invalid input by design, so it clamps again on read.
pub const MIN_WEIGHT_MULTIPLIER: f64 = 0.5;
/// See [`MIN_WEIGHT_MULTIPLIER`].
pub const MAX_WEIGHT_MULTIPLIER: f64 = 2.0;

/// Days and blocks a Person would RATHER have.
///
/// **EMPTY MEANS NO PREFERENCE** — the inverse of
/// [`crate::problem::Unavailability`], where an empty axis means "every value
/// on that axis". The two are structurally identical and semantically
/// inverted, which is why they are separate types rather than one reused one:
/// reusing `Unavailability` here would put the inversion one refactor away from
/// being lost, with a compiler that cannot warn.
///
/// **No `weeks` axis**, and that absence is load-bearing rather than an
/// omission — it is what collapses this module's table from `placement × slot`
/// to `placement × (day, block)`.
#[derive(Clone, Debug, Default)]
pub struct Preference {
    /// ISO weekday, 1 = Monday.
    pub days: Vec<u32>,
    /// 0-based within the day.
    pub blocks: Vec<u32>,
    /// Room-type keys this Person would RATHER teach in, from `Room.
    /// feature_tags`' vocabulary. Empty means no room-type preference stated —
    /// the same "empty means nothing" reading `days`/`blocks` already have.
    ///
    /// Deliberately NOT folded into the same `axes`/`met` computation those two
    /// use: a room is not a grid coordinate, so there is nothing to narrow it
    /// against, and — the load-bearing reason — doing so would change the
    /// EXISTING divisor for any Person who states both kinds of preference,
    /// silently reweighting day/block credit that already shipped. It is
    /// scored as an independent additive term instead; see
    /// [`PreferenceModel::cost`].
    pub room_features: Vec<String>,
    /// Bounded per-person override of the tenant's weight. `None` means "use
    /// the tenant weight unmodified", which is a distinct state from `Some(0.0)`
    /// — hence `Option` rather than a plain `f64` defaulting to zero.
    pub weight_multiplier: Option<f64>,
}

/// A configured `PersonPreferenceFit`.
///
/// Its own type rather than a [`crate::soft::SoftInstance`] for the reason in
/// the module docs, and the same shape `DayMixInstance` has: an id, a kind
/// scope and a weight.
///
/// Like `LecturerVeto`, this is a tenant-level switch over per-person data —
/// the preference VALUES live on [`Person::preferred`] and are not restated
/// here — one severity down.
#[derive(Clone, Debug)]
pub struct PreferenceInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    /// Non-negative. Zero means "report the fit but do not steer".
    pub weight: f64,
}

impl PreferenceInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One lecturer's preference, narrowed to this run's grid and reduced to what
/// the table build needs.
#[derive(Clone, Debug)]
struct Narrowed {
    /// Indexed by position within `SlotTable::active_days`.
    days: Vec<bool>,
    /// Indexed by block, 0-based within the day.
    blocks: Vec<bool>,
    /// How many axes this person actually stated: 1 or 2, never 0 — a person
    /// who stated nothing usable is not counted at all.
    ///
    /// The divisor is the number of STATED axes, not always 2. Otherwise
    /// someone who named only days could never exceed a fit of 0.5 and would
    /// be permanently half-penalized for a preference they fully expressed.
    axes: f64,
    /// Already clamped.
    multiplier: f64,
}

/// Precomputed per-placement preference costs.
///
/// Empty and inert when no `PersonPreferenceFit` is configured, so every read
/// site can call [`PreferenceModel::cost`] unconditionally.
#[derive(Clone, Debug, Default)]
pub struct PreferenceModel {
    pub instances: Vec<PreferenceInstance>,
    /// `placement * row_width + cell_of_slot[slot]`, holding the **unmet**
    /// fraction: `Σ m(p) × (1 - fit(p)) / |P|`, in `0.0..=MAX_WEIGHT_MULTIPLIER`.
    ///
    /// `f32` because this is the one table large enough for the choice to
    /// matter, and the value is a normalized fraction that never needs `f64`
    /// range. The objective sums in `f64`.
    table: Vec<f32>,
    /// slot -> weekly cell, `day position * blocks_per_day + block`.
    cell_of_slot: Vec<u32>,
    /// `cells + 1`: one row per placement plus the always-zero sentinel cell.
    row_width: usize,
    /// Summed weight of the instances covering each placement's kind. Kept
    /// OUT of the table: it is one scalar per placement rather than per cell,
    /// and folding it in would mean rebuilding 1.1 M entries to change a
    /// weight.
    weight_of: Vec<f64>,
    /// Summed weight of every configured instance, for the derived hard penalty
    /// and the initial temperature.
    pub total_weight: f64,
    /// Per-placement room-type preference: which counted lecturers stated
    /// `preferred_room_features`, and their (already-clamped) multiplier.
    /// Bounded by the placement's lecturer count — a handful of entries, not
    /// a table — so unlike `table` this clones its small string lists rather
    /// than indexing into a shared array. See [`PreferenceModel::cost`] for
    /// why room preference is scored live instead of precomputed per room.
    room_wanted: Vec<Vec<(Vec<String>, f64)>>,
    /// Every Person's OWN narrowed day/block preference, persisted rather
    /// than kept local to `build` — the module doc's own blueprint for what
    /// a genuine lecturer pool needs: `table`/`room_wanted` are only valid
    /// while a placement's lecturer set is fixed before the search starts,
    /// so a pool Offering's placements bypass them entirely and read this
    /// per-PERSON table live instead, at scoring time, over whichever
    /// lecturers the search is actually trying. See
    /// [`PreferenceModel::cost_for`].
    narrowed: Vec<Option<Narrowed>>,
    /// The per-person counterpart of `room_wanted`, for the same reason
    /// `narrowed` is: a pool Offering's room preference cost is computed live
    /// over the candidate's chosen lecturers, not read from a per-placement
    /// row built before any choice existed.
    person_room_wanted: Vec<Option<(Vec<String>, f64)>>,
    /// Persisted so [`PreferenceModel::cost_for`] can turn a `slot` into
    /// `(day_pos, block)` the same way `cell_of_slot` lets the static path
    /// turn it into a table row — both read the identical `cell_of_slot`
    /// mapping, just decoded back into its two components here.
    blocks_per_day: usize,
}

/// Every additive term `unmet`/`cost` can carry per placement, for the shared
/// ceiling. Day/block share ONE `axes` divisor and count as one family; room
/// is a second, independent family — see `Preference::room_features` for why
/// it is not folded into the same divisor. A third term would need this
/// updated alongside it, which is the entire point of naming it once.
const PREFERENCE_AXIS_FAMILIES: f64 = 2.0;

impl PreferenceModel {
    /// Build the table from each placement's lecturer set.
    ///
    /// The attendee scan happens `placements × 1` times here rather than once
    /// per candidate evaluation, which is the entire point of the
    /// representation.
    pub fn build(
        instances: Vec<PreferenceInstance>,
        slots: &SlotTable,
        persons: &[Person],
        offerings: &[Offering],
        placements: &[PlacementVar],
    ) -> Self {
        let total_weight = instances.iter().map(|i| i.weight).sum();
        let blocks_per_day = slots.blocks_per_day() as usize;
        let cells = slots.active_days().len() * blocks_per_day;
        // One spare cell, permanently 0.0, so a slot whose weekday is somehow
        // not in `active_days` prices at nothing instead of aliasing onto the
        // first day. Unreachable through `SlotTable::build`, which generates
        // slots FROM `active_days` — but the alternative fallback is a silent
        // mis-pricing rather than a visible failure, and one f32 per placement
        // is a cheap way not to have it.
        let row_width = cells + 1;

        let cell_of_slot: Vec<u32> = slots
            .all()
            .map(|slot| {
                let f = slots.flags(slot);
                match slots.active_days().iter().position(|&d| d == f.iso_weekday) {
                    Some(pos) if (f.block as usize) < blocks_per_day => {
                        (pos * blocks_per_day + f.block as usize) as u32
                    }
                    _ => cells as u32,
                }
            })
            .collect();

        // Narrowed once per Person, not once per placement: a lecturer leading
        // twenty Sessions is narrowed once. Computed before the early return
        // below (unlike before this field was persisted) so an empty
        // `instances` still leaves `narrowed`/`person_room_wanted` correctly
        // populated on `Self` — dead weight while nothing is configured, but
        // one fewer special case than deriving them lazily later.
        let narrowed: Vec<Option<Narrowed>> = persons
            .iter()
            .map(|p| narrow(p.preferred.as_ref(), slots, blocks_per_day))
            .collect();
        let person_room_wanted: Vec<Option<(Vec<String>, f64)>> = persons
            .iter()
            .map(|p| {
                let pref = p.preferred.as_ref()?;
                if pref.room_features.is_empty() {
                    return None;
                }
                Some((pref.room_features.clone(), clamp_multiplier(pref.weight_multiplier)))
            })
            .collect();

        if instances.is_empty() || cells == 0 {
            return Self {
                instances,
                table: Vec::new(),
                cell_of_slot,
                row_width,
                weight_of: Vec::new(),
                total_weight,
                room_wanted: Vec::new(),
                narrowed,
                person_room_wanted,
                blocks_per_day,
            };
        }

        let mut weight_of = vec![0.0f64; placements.len()];
        let mut table = vec![0.0f32; placements.len() * row_width];
        let mut room_wanted: Vec<Vec<(Vec<String>, f64)>> = vec![Vec::new(); placements.len()];

        for (i, var) in placements.iter().enumerate() {
            let offering = &offerings[var.offering.get()];
            let weight: f64 = instances
                .iter()
                .filter(|inst| inst.covers(&offering.kind))
                .map(|inst| inst.weight)
                .sum();
            weight_of[i] = weight;

            if weight == 0.0 {
                // Gates BOTH terms: an instance weight of zero means "count
                // the fit but do not steer", the same reading a zero weight
                // gives every other soft type, and it is the one input both
                // the day/block table and room preference share.
                continue;
            }

            // §4.1: LECTURERS ONLY, deliberately. A Session's attendee set is
            // its lecturers plus every member of every attached Group's
            // descendant closure — ~65 people at benchmark scale — so counting
            // attendees would turn "this tutor prefers mornings" into an
            // unweighted vote a 200-student cohort wins.
            //
            // A lecturer with nothing usable stated is not in the counted set,
            // rather than being in it with a fit of zero. The difference is the
            // whole term: "stated nothing" must cost nothing, while "stated
            // something and did not get it" must cost.
            let counted: Vec<&Narrowed> = offering
                .lecturers
                .iter()
                .filter_map(|l| narrowed[l.get()].as_ref())
                .collect();

            // Independent of `counted` above and of whether it ends up empty:
            // a lecturer stating ONLY a room preference and no usable
            // day/block axis is invisible to `narrow()` (`axes == 0` there),
            // so this reads `person_room_wanted` directly rather than riding
            // on `narrowed`. Reusing `counted`'s day/block gate here would
            // silently drop that lecturer's room preference whenever they
            // said nothing about days or blocks — exactly the "stated
            // something and did not get it" case this type exists to charge.
            room_wanted[i] = offering
                .lecturers
                .iter()
                .filter_map(|l| person_room_wanted[l.get()].clone())
                .collect();

            if counted.is_empty() {
                // `|P| = 0`, which is reachable: `required_lecturer_count` is a
                // `uint32` defaulting to 0, so a tenant-defined `staff_meeting`
                // kind requires no lecturer at all. The mean is undefined
                // there, so the cost is 0 — no counted lecturer means no
                // preference signal, and 0 is the identity of a term the rule
                // has nothing to say about.
                //
                // Resolved HERE rather than at read time: the row is left all
                // zeros so the scoring path stays a branch-free indexed read.
                // The consequence is deliberate — "no lecturers" becomes
                // numerically indistinguishable from "lecturers who stated
                // nothing", which is correct, both meaning the rule has nothing
                // to say. It also means an enabled rule can be entirely inert,
                // which is why the app's assembly counts placements with no
                // preference signal: otherwise this is the `lecturer_veto`
                // shape again, a rule that looks configured and can never fire.
                //
                // `room_wanted[i]` is UNAFFECTED by this continue — it was
                // already filled above and stands on its own gate.
                continue;
            }

            let divisor = counted.len() as f64;
            for (day_pos, _) in slots.active_days().iter().enumerate() {
                for block in 0..blocks_per_day {
                    // The MEAN over the product `multiplier × unmet`, not the
                    // product of the two means. They are not interchangeable:
                    // `mean(a × b) ≠ mean(a) × mean(b)` whenever a and b
                    // covary, and here they do, since a lecturer's multiplier
                    // and whether THEIR preference is met are independent facts
                    // about that lecturer.
                    //
                    // Both forms respect the ceiling, so a bound check does not
                    // separate them. Attribution does: the separated form
                    // applies the average multiplier to the average fit, so a
                    // lecturer with a 2.0 multiplier inflates the cost even
                    // when the placement suits them perfectly and it is the 0.5
                    // lecturer who is inconvenienced.
                    //
                    // MEAN and not SUM for a different reason: a sum over |P|
                    // lecturers is bounded by `max_multiplier × |P|`, which puts
                    // an instance-data quantity back into the hard-penalty
                    // ceiling — a tenant could then raise this type's
                    // contribution arbitrarily by raising
                    // `required_lecturer_count`, with no weight change and no
                    // warning. A mean of terms each `<= B` is itself `<= B`.
                    let mut sum = 0.0f64;
                    for n in &counted {
                        sum += n.multiplier * n.unmet(day_pos, block);
                    }
                    let cell = day_pos * blocks_per_day + block;
                    table[i * row_width + cell] = (sum / divisor) as f32;
                }
            }
        }

        Self {
            instances,
            table,
            cell_of_slot,
            row_width,
            weight_of,
            total_weight,
            room_wanted,
            narrowed,
            person_room_wanted,
            blocks_per_day,
        }
    }

    /// The unmet fraction for `p` starting at `slot`, landing in a Room with
    /// `room_features`, in `0.0..=PREFERENCE_AXIS_FAMILIES * MAX_WEIGHT_MULTIPLIER`.
    ///
    /// Day/block and room are independent additive terms — see
    /// [`Preference::room_features`] for why room is not folded into the same
    /// `axes` divisor day/block share. Keyed on the START slot, like every
    /// other soft cost: a two-block Session beginning in block 1 is priced on
    /// block 1.
    #[inline]
    pub fn unmet(&self, p: PlacementIdx, slot: SlotIdx, room_features: &[String]) -> f64 {
        self.day_block_unmet(p, slot) + self.room_unmet(p, room_features)
    }

    /// What the objective is charged for placing `p` at `slot` in a Room with
    /// `room_features`.
    #[inline]
    pub fn cost(&self, p: PlacementIdx, slot: SlotIdx, room_features: &[String]) -> f64 {
        if self.table.is_empty() {
            return 0.0;
        }
        self.weight_of[p.get()] * self.unmet(p, slot, room_features)
    }

    /// The [`Self::cost`] counterpart for a pool Offering — see the module
    /// doc's own blueprint. `lecturers` is the CANDIDATE'S actual chosen
    /// combination (`Placement::lecturers`/`Occupant::pool_lecturers`), read
    /// live against the per-person `narrowed`/`person_room_wanted` tables
    /// instead of `table`/`room_wanted`, which were built before any choice
    /// existed and are never valid for a pool placement.
    ///
    /// `weight_of[p]` is still read from the static table: it depends only on
    /// `offering.kind`, never on which lecturer leads the placement, so it is
    /// exactly as precomputable for a pool Offering as for a fixed one.
    #[inline]
    pub fn cost_for(
        &self,
        p: PlacementIdx,
        lecturers: &[Option<PersonIdx>; MAX_LECTURERS],
        slot: SlotIdx,
        room_features: &[String],
    ) -> f64 {
        if self.table.is_empty() {
            return 0.0;
        }
        self.weight_of[p.get()] * self.unmet_for(lecturers, slot, room_features)
    }

    /// The [`Self::unmet`] counterpart for a pool Offering — see
    /// [`Self::cost_for`].
    #[inline]
    pub fn unmet_for(
        &self,
        lecturers: &[Option<PersonIdx>; MAX_LECTURERS],
        slot: SlotIdx,
        room_features: &[String],
    ) -> f64 {
        self.day_block_unmet_for(lecturers, slot) + self.room_unmet_for(lecturers, room_features)
    }

    #[inline]
    fn day_block_unmet(&self, p: PlacementIdx, slot: SlotIdx) -> f64 {
        if self.table.is_empty() {
            return 0.0;
        }
        let cell = self.cell_of_slot[slot.get()] as usize;
        f64::from(self.table[p.get() * self.row_width + cell])
    }

    /// The same MEAN-over-counted-lecturers math [`Self::build`]'s table-fill
    /// loop uses, computed for one `(day, block)` cell on demand instead of
    /// precomputed for all of them — the O(|P|) scoring step the module doc
    /// trades for a lecturer set that can change.
    #[inline]
    fn day_block_unmet_for(
        &self,
        lecturers: &[Option<PersonIdx>; MAX_LECTURERS],
        slot: SlotIdx,
    ) -> f64 {
        if self.table.is_empty() {
            return 0.0;
        }
        let cell = self.cell_of_slot[slot.get()] as usize;
        // The sentinel cell (`row_width - 1`): unreachable in practice, same
        // as the static path's — see `cell_of_slot`'s own doc.
        if cell >= self.row_width - 1 {
            return 0.0;
        }
        let day_pos = cell / self.blocks_per_day;
        let block = cell % self.blocks_per_day;
        let counted: Vec<&Narrowed> = lecturers
            .iter()
            .flatten()
            .filter_map(|l| self.narrowed[l.get()].as_ref())
            .collect();
        if counted.is_empty() {
            return 0.0;
        }
        let sum: f64 = counted
            .iter()
            .map(|n| n.multiplier * n.unmet(day_pos, block))
            .sum();
        sum / counted.len() as f64
    }

    /// The [`Self::room_unmet`] counterpart for a pool Offering, reading
    /// `person_room_wanted` per candidate lecturer instead of the
    /// per-placement `room_wanted` row.
    #[inline]
    fn room_unmet_for(
        &self,
        lecturers: &[Option<PersonIdx>; MAX_LECTURERS],
        room_features: &[String],
    ) -> f64 {
        if self.person_room_wanted.is_empty() {
            return 0.0;
        }
        let wanted: Vec<&(Vec<String>, f64)> = lecturers
            .iter()
            .flatten()
            .filter_map(|l| self.person_room_wanted[l.get()].as_ref())
            .collect();
        if wanted.is_empty() {
            return 0.0;
        }
        let sum: f64 = wanted
            .iter()
            .map(|(features, multiplier)| {
                let met = features.iter().any(|f| room_features.contains(f));
                multiplier * if met { 0.0 } else { 1.0 }
            })
            .sum();
        sum / wanted.len() as f64
    }

    /// LIVE, not precomputed: unlike day/block, this does not vary with
    /// `slot`, so a `(placement, room)` table would only exist to cache a
    /// lookup as cheap as the comparison itself. `room_wanted[p]` is already
    /// bounded to a handful of entries — a placement's counted lecturers, not
    /// every Room in the tenant — so comparing feature lists here costs
    /// nothing a hot loop would notice.
    #[inline]
    fn room_unmet(&self, p: PlacementIdx, room_features: &[String]) -> f64 {
        if self.room_wanted.is_empty() {
            return 0.0;
        }
        let wanted = &self.room_wanted[p.get()];
        if wanted.is_empty() {
            return 0.0;
        }
        let sum: f64 = wanted
            .iter()
            .map(|(features, multiplier)| {
                // ANY overlap counts as met, mirroring the day/block axes:
                // neither is a conjunction ("Tuesday mornings only" cannot be
                // expressed by two arrays), so "any stated room type present"
                // is the reading consistent with "I like mornings, and I like
                // Tuesdays" being two separate statements rather than one.
                let met = features.iter().any(|f| room_features.contains(f));
                multiplier * if met { 0.0 } else { 1.0 }
            })
            .sum();
        sum / wanted.len() as f64
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The most this type can charge for one placement, computed from the
    /// constraint configuration ALONE.
    ///
    /// That independence from instance data is the property that makes the
    /// per-person override safe rather than merely bounded: each additive
    /// term's unmet fraction is normalized to `0.0..=1.0` per placement and
    /// each lecturer's own multiplier is bounded by `MAX_WEIGHT_MULTIPLIER`,
    /// so the ceiling does not depend on how many lecturers a placement has or
    /// how many have an override on file. `PREFERENCE_AXIS_FAMILIES` scales
    /// this for room joining day/block as a second independent term — see its
    /// own doc for why a third term must update it too. See
    /// `Problem::hard_penalty`.
    pub fn max_cost_per_placement(&self) -> f64 {
        self.total_weight * MAX_WEIGHT_MULTIPLIER * PREFERENCE_AXIS_FAMILIES
    }
}

impl Narrowed {
    /// `1 - fit`: the fraction of this person's stated axes that `(day, block)`
    /// does NOT satisfy.
    ///
    /// The term is a PENALTY on the unmet fraction rather than a reward for the
    /// met one, because `Objective::soft` is minimized and every other soft
    /// type is a non-negative cost. A negative term would put the derived
    /// `hard_penalty` bound, the `total() == 0` convergence check and
    /// `ruin_worst`'s "worst" ordering on a different footing all at once, for
    /// an ordering that is otherwise the same.
    ///
    /// ADDITIVE partial credit, per the storage semantics: each axis earns
    /// credit independently, so `{days: [2], blocks: [0, 1]}` reads as "I like
    /// Tuesdays, and I like mornings" — two separate statements — and a Tuesday
    /// first block earns both. Not a conjunction ("Tuesday mornings only"),
    /// which the two-array shape cannot express and which would need a
    /// `(day × block)` matrix on the wire and in storage.
    #[inline]
    fn unmet(&self, day_pos: usize, block: usize) -> f64 {
        let mut met = 0.0;
        if !self.days.is_empty() && self.days[day_pos] {
            met += 1.0;
        }
        if !self.blocks.is_empty() && self.blocks[block] {
            met += 1.0;
        }
        1.0 - met / self.axes
    }
}

/// Narrow a stated preference to this run's grid, or `None` if nothing usable
/// is left.
///
/// A stated day the tenant does not teach on, or a block past the end of the
/// day, is DROPPED rather than treated as unsatisfiable. Both directions of
/// that choice matter: the app validates a preference against the tenant's
/// WIDEST grid, because a preference is not term-scoped and must stay
/// expressible for every grid the tenant has, while exactly one grid is in
/// force at solve time. So a stored `block 9` is legitimate data and an
/// impossible slot in the same breath.
///
/// Dropping it makes the value inert, matching `MinimizeBlockUsage`, where a
/// stale index "simply never matches" rather than failing a run. Keeping it
/// would do the opposite of inert here: an axis that can never be satisfied
/// would charge the person at every slot in the grid, with no placement able to
/// fix it.
fn narrow(pref: Option<&Preference>, slots: &SlotTable, blocks_per_day: usize) -> Option<Narrowed> {
    let pref = pref?;

    let mut days = vec![false; slots.active_days().len()];
    let mut any_day = false;
    for d in &pref.days {
        if let Some(pos) = slots.active_days().iter().position(|a| a == d) {
            days[pos] = true;
            any_day = true;
        }
    }

    let mut blocks = vec![false; blocks_per_day];
    let mut any_block = false;
    for &b in &pref.blocks {
        if (b as usize) < blocks_per_day {
            blocks[b as usize] = true;
            any_block = true;
        }
    }

    let axes = f64::from(u8::from(any_day) + u8::from(any_block));
    if axes == 0.0 {
        // Stated nothing, or stated nothing this grid has a slot for. After
        // narrowing those are the same fact, and it keeps one representation.
        return None;
    }

    Some(Narrowed {
        days: if any_day { days } else { Vec::new() },
        blocks: if any_block { blocks } else { Vec::new() },
        axes,
        multiplier: clamp_multiplier(pref.weight_multiplier),
    })
}

/// Clamp a per-person weight override to `MIN_WEIGHT_MULTIPLIER..=
/// MAX_WEIGHT_MULTIPLIER`, defaulting to `1.0` (tenant weight unmodified).
///
/// Shared between the day/block axis and the room axis, which both bound a
/// lecturer's own contribution the same way. Clamped defensively rather than
/// via `f64::clamp` directly: that propagates NaN and panics on a NaN bound,
/// so a non-finite multiplier is replaced outright — this service accepts
/// possibly-invalid input by design, and one NaN reaching either term would
/// poison the whole objective.
fn clamp_multiplier(m: Option<f64>) -> f64 {
    match m {
        Some(m) if m.is_finite() => m.clamp(MIN_WEIGHT_MULTIPLIER, MAX_WEIGHT_MULTIPLIER),
        Some(_) | None => 1.0,
    }
}
