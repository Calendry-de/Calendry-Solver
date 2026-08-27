//! Day- and window-granularity counters for the two Group-scoped hard types.
//!
//! # Why these are not occupancy bitsets
//!
//! Slices 1–3 needed exactly two shapes: **pairwise** interactions keyed by
//! `(entity, slot)` (the four structural types), and **unary** per-Session costs
//! keyed by `(slot, room)` (the six soft types). Neither shape fits here.
//!
//! * `OnlineOnsiteSameDay` interacts at **day** granularity, not slot. Two
//!   Sessions of one Group clash only if they share a *day* and disagree about
//!   being online — a pair sharing a slot is neither necessary nor sufficient.
//!
//!   It WAS a feasibility filter, because it is monotone-safe: placing the
//!   first Session on a day can never violate it. It is now SOFT, so the filter
//!   is gone and the same counters feed the objective instead. The counters did
//!   not change; what changed is that `day_mix_allows` answers "would this cost
//!   something" rather than "is this permitted at all".
//!
//! * `MaxOnlineShare` is a **cardinality ratio over a set**, and cannot be a
//!   filter at all. "31% online" is invisible in any pair of Sessions, and a
//!   filter would dead-end construction: the first online Session placed makes
//!   the ratio 100%, over any threshold below 1.0, because the denominator has
//!   not grown yet. For `PER_WEEK` the denominator also *moves* when a Session
//!   relocates between weeks.
//!
//!   So it lives on the **objective** instead, on the hard side. A run can
//!   therefore succeed while still reporting a `MaxOnlineShare` violation —
//!   exactly how `ExactFrequency` already behaves for unplaced Sessions, rather
//!   than a new exception.

use crate::ids::GroupIdx;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShareWindow {
    /// One bucket for the whole term.
    PerTerm,
    /// One bucket per calendar week index from the academic calendar.
    PerWeek,
}

/// One configured `OnlineOnsiteSameDay`.
///
/// SOFT since the reclassification. It carries a weight for the same reason
/// every other soft instance does — the objective needs to know what a mixed
/// day costs — and it stays in its own list rather than joining
/// [`crate::soft::SoftModel`] because that model is a precomputed
/// `(slot, room)` table and a mixed day is a property of what ELSE is already
/// placed for the Group that day. It cannot be read off a table keyed by the
/// candidate alone.
#[derive(Clone, Debug)]
pub struct DayMixInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    pub weight: f64,
}

impl DayMixInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

/// One configured `MaxOnlineShare`.
#[derive(Clone, Debug)]
pub struct ShareInstance {
    pub id: String,
    /// Empty means all kinds.
    pub kinds: Vec<String>,
    /// 0.0 ..= 1.0
    pub max_ratio: f64,
    pub window: ShareWindow,
}

impl ShareInstance {
    #[inline]
    pub fn covers(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }

    /// The permitted number of online Sessions out of `total`.
    ///
    /// Floor, so 3 Sessions at a 0.3 ratio permit **zero** online — the
    /// constraint is a ceiling on the share, and rounding up would silently
    /// allow 33%.
    #[inline]
    pub fn allowance(&self, total: u32) -> u32 {
        (self.max_ratio * total as f64).floor() as u32
    }
}

/// Per-instance counters for one `MaxOnlineShare` rule.
#[derive(Clone, Debug)]
struct ShareCounters {
    total: Vec<u32>,
    online: Vec<u32>,
    windows: usize,
    violated: u32,
}

impl ShareCounters {
    fn new(groups: usize, windows: usize) -> Self {
        Self {
            total: vec![0; groups * windows],
            online: vec![0; groups * windows],
            windows,
            violated: 0,
        }
    }

    #[inline]
    fn cell(&self, group: GroupIdx, window: usize) -> usize {
        group.get() * self.windows + window
    }

    #[inline]
    fn is_violated(&self, rule: &ShareInstance, cell: usize) -> bool {
        self.online[cell] > rule.allowance(self.total[cell])
    }
}

#[derive(Clone, Debug, Default)]
pub struct Aggregates {
    /// `[group * n_days + day]` — Sessions of that Group on that day, split by
    /// delivery mode. `OnlineOnsiteSameDay` is violated when both are non-zero.
    online_day: Vec<u32>,
    onsite_day: Vec<u32>,
    n_days: usize,

    rules: Vec<ShareInstance>,
    counters: Vec<ShareCounters>,
}

impl Aggregates {
    pub fn new(n_groups: usize, n_days: usize, n_weeks: usize, rules: Vec<ShareInstance>) -> Self {
        let groups = n_groups.max(1);
        let counters = rules
            .iter()
            .map(|r| {
                let windows = match r.window {
                    ShareWindow::PerTerm => 1,
                    ShareWindow::PerWeek => n_weeks.max(1),
                };
                ShareCounters::new(groups, windows)
            })
            .collect();

        Self {
            online_day: vec![0; groups * n_days.max(1)],
            onsite_day: vec![0; groups * n_days.max(1)],
            n_days: n_days.max(1),
            rules,
            counters,
        }
    }

    pub fn has_day_mix_state(&self) -> bool {
        self.n_days > 0
    }

    pub fn rules(&self) -> &[ShareInstance] {
        &self.rules
    }

    /// Total violated `(rule, group, window)` cells. This is the number that
    /// joins the objective's hard component.
    pub fn share_violations(&self) -> u32 {
        self.counters.iter().map(|c| c.violated).sum()
    }

    // -- day mix -----------------------------------------------------------

    #[inline]
    fn day_cell(&self, group: GroupIdx, day: u32) -> usize {
        group.get() * self.n_days + day as usize
    }

    /// Would placing a Session of these Groups, in this mode, on these days,
    /// create a mixed day?
    pub fn day_mix_allows(&self, groups: &[GroupIdx], days: &[u32], is_online: bool) -> bool {
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                let blocked =
                    if is_online { self.onsite_day[c] > 0 } else { self.online_day[c] > 0 };
                if blocked {
                    return false;
                }
            }
        }
        true
    }

    pub fn add_day_mode(&mut self, groups: &[GroupIdx], days: &[u32], is_online: bool) {
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                if is_online {
                    self.online_day[c] += 1;
                } else {
                    self.onsite_day[c] += 1;
                }
            }
        }
    }

    pub fn remove_day_mode(&mut self, groups: &[GroupIdx], days: &[u32], is_online: bool) {
        for &g in groups {
            for &d in days {
                let c = self.day_cell(g, d);
                if is_online {
                    self.online_day[c] = self.online_day[c].saturating_sub(1);
                } else {
                    self.onsite_day[c] = self.onsite_day[c].saturating_sub(1);
                }
            }
        }
    }

    /// Groups whose day is currently mixed, for diagnostics.
    /// How many `(group, day)` cells currently mix the two delivery modes.
    ///
    /// The number the objective charges for, and the counterpart of
    /// [`Self::share_violations`] — read straight off the counters rather than
    /// accumulated as a delta, because a mixed day is a property of a cell and
    /// not of any one placement. Two Sessions make a cell mixed; neither of them
    /// individually "costs" anything.
    pub fn day_mix_violations(&self) -> u32 {
        (0..self.online_day.len())
            .filter(|&c| self.online_day[c] > 0 && self.onsite_day[c] > 0)
            .count() as u32
    }

    /// Total `(group, day)` cells — the exact upper bound on how many can be
    /// mixed at once, which is what bounds the day-mix term's contribution to
    /// the objective. See `Problem::hard_penalty`.
    pub fn day_mix_cell_count(&self) -> usize {
        self.online_day.len()
    }

    pub fn mixed_days(&self) -> impl Iterator<Item = (GroupIdx, u32)> + '_ {
        (0..self.online_day.len())
            .filter(move |&c| self.online_day[c] > 0 && self.onsite_day[c] > 0)
            .map(move |c| (GroupIdx((c / self.n_days) as u32), (c % self.n_days) as u32))
    }

    // -- share -------------------------------------------------------------

    /// Add or remove one Session from every rule covering `kind`, keeping the
    /// running violation count exact by re-testing only the touched cells.
    pub fn apply_share(
        &mut self,
        kind: &str,
        groups: &[GroupIdx],
        week: u32,
        is_online: bool,
        add: bool,
    ) {
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.covers(kind) {
                continue;
            }
            let window = match rule.window {
                ShareWindow::PerTerm => 0,
                ShareWindow::PerWeek => week as usize,
            };
            let counters = &mut self.counters[i];
            if window >= counters.windows {
                continue;
            }

            for &g in groups {
                let cell = counters.cell(g, window);
                let before = counters.is_violated(rule, cell);

                if add {
                    counters.total[cell] += 1;
                    if is_online {
                        counters.online[cell] += 1;
                    }
                } else {
                    counters.total[cell] = counters.total[cell].saturating_sub(1);
                    if is_online {
                        counters.online[cell] = counters.online[cell].saturating_sub(1);
                    }
                }

                let after = counters.is_violated(rule, cell);
                match (before, after) {
                    (false, true) => counters.violated += 1,
                    (true, false) => counters.violated = counters.violated.saturating_sub(1),
                    _ => {}
                }
            }
        }
    }

    /// Would adding one Session push any covering cell over its allowance?
    ///
    /// Used to *score* a candidate, never to reject it: `MaxOnlineShare` is an
    /// aggregate ratio, so a filter would dead-end construction before the
    /// denominator has grown.
    pub fn share_would_worsen(
        &self,
        kind: &str,
        groups: &[GroupIdx],
        week: u32,
        is_online: bool,
    ) -> bool {
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.covers(kind) {
                continue;
            }
            let window = match rule.window {
                ShareWindow::PerTerm => 0,
                ShareWindow::PerWeek => week as usize,
            };
            let counters = &self.counters[i];
            if window >= counters.windows {
                continue;
            }
            for &g in groups {
                let cell = counters.cell(g, window);
                let total = counters.total[cell] + 1;
                let online = counters.online[cell] + u32::from(is_online);
                if online > rule.allowance(total) {
                    return true;
                }
            }
        }
        false
    }

    /// Violated cells as `(rule index, group, window, online, total)`, for
    /// diagnostics and for the authoritative evaluator.
    pub fn violated_cells(&self) -> Vec<(usize, GroupIdx, usize, u32, u32)> {
        let mut out = Vec::new();
        for (i, counters) in self.counters.iter().enumerate() {
            let rule = &self.rules[i];
            for cell in 0..counters.total.len() {
                if counters.is_violated(rule, cell) {
                    out.push((
                        i,
                        GroupIdx((cell / counters.windows) as u32),
                        cell % counters.windows,
                        counters.online[cell],
                        counters.total[cell],
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(max_ratio: f64, window: ShareWindow) -> ShareInstance {
        ShareInstance { id: "s".into(), kinds: vec![], max_ratio, window }
    }

    #[test]
    fn allowance_floors_rather_than_rounds() {
        let r = rule(0.3, ShareWindow::PerTerm);
        assert_eq!(r.allowance(3), 0, "0.3 of 3 permits zero, not one");
        assert_eq!(r.allowance(4), 1);
        assert_eq!(r.allowance(10), 3);
        assert_eq!(rule(0.0, ShareWindow::PerTerm).allowance(100), 0);
        assert_eq!(rule(1.0, ShareWindow::PerTerm).allowance(7), 7);
    }

    #[test]
    fn day_mix_blocks_only_the_opposite_mode() {
        let mut a = Aggregates::new(2, 3, 1, vec![]);
        let g = [GroupIdx(0)];

        assert!(a.day_mix_allows(&g, &[0], true), "empty day accepts anything");
        a.add_day_mode(&g, &[0], true);

        assert!(a.day_mix_allows(&g, &[0], true), "another online session is fine");
        assert!(!a.day_mix_allows(&g, &[0], false), "on-site would make it a mix");

        // A different day, and a different group, are unaffected.
        assert!(a.day_mix_allows(&g, &[1], false));
        assert!(a.day_mix_allows(&[GroupIdx(1)], &[0], false));

        a.remove_day_mode(&g, &[0], true);
        assert!(a.day_mix_allows(&g, &[0], false), "removal reopens the day");
    }

    #[test]
    fn share_violation_tracks_a_moving_denominator() {
        let mut a = Aggregates::new(1, 1, 1, vec![rule(0.5, ShareWindow::PerTerm)]);
        let g = [GroupIdx(0)];

        // 1 online of 1 total = 100% > 50%.
        a.apply_share("lecture", &g, 0, true, true);
        assert_eq!(a.share_violations(), 1);

        // Adding an on-site Session grows the denominator and clears it.
        a.apply_share("lecture", &g, 0, false, true);
        assert_eq!(a.share_violations(), 0, "1 of 2 is exactly 50%");

        // Removing the on-site one puts it back over.
        a.apply_share("lecture", &g, 0, false, false);
        assert_eq!(a.share_violations(), 1);
    }

    #[test]
    fn per_week_and_per_term_bucket_differently() {
        let g = [GroupIdx(0)];
        // Two online in week 0, two on-site in week 1, ratio 0.5.
        let load = |a: &mut Aggregates| {
            a.apply_share("lecture", &g, 0, true, true);
            a.apply_share("lecture", &g, 0, true, true);
            a.apply_share("lecture", &g, 1, false, true);
            a.apply_share("lecture", &g, 1, false, true);
        };

        // PER_TERM: 2 online of 4 = 50%, allowed.
        let mut term = Aggregates::new(1, 1, 2, vec![rule(0.5, ShareWindow::PerTerm)]);
        load(&mut term);
        assert_eq!(term.share_violations(), 0);

        // PER_WEEK: week 0 is 2 of 2 = 100%, violated.
        let mut week = Aggregates::new(1, 1, 2, vec![rule(0.5, ShareWindow::PerWeek)]);
        load(&mut week);
        assert_eq!(week.share_violations(), 1);
    }

    #[test]
    fn kind_scoping_skips_rules_that_do_not_cover() {
        let mut a = Aggregates::new(
            1,
            1,
            1,
            vec![ShareInstance {
                id: "s".into(),
                kinds: vec!["lecture".into()],
                max_ratio: 0.0,
                window: ShareWindow::PerTerm,
            }],
        );
        a.apply_share("staff_meeting", &[GroupIdx(0)], 0, true, true);
        assert_eq!(a.share_violations(), 0, "out-of-scope kind must not count");

        a.apply_share("lecture", &[GroupIdx(0)], 0, true, true);
        assert_eq!(a.share_violations(), 1);
    }
}
