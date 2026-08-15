//! Slot flattening and per-slot flag tables.
//!
//! This module is the structural reason the TimeCraft prototype's magic-number
//! bugs cannot recur. Every `(week, day, block)` is resolved once into a global
//! [`SlotIdx`], and everything a constraint might want to know about that slot
//! — is it the first block of its day, the last block, which weekday, is its
//! week an exam week — is precomputed into a flag table.
//!
//! Soft constraints then become table lookups. There is no arithmetic anywhere
//! downstream that could hardcode `% 3`, `> 14`, or `weeks[-n:]`.

use crate::ids::SlotIdx;

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum WeekKind {
    #[default]
    Teaching,
    Exam,
    Break,
    Holiday,
}

#[derive(Clone, Debug)]
pub struct SlotFlags {
    pub week: u32,
    /// ISO weekday, 1 = Monday .. 7 = Sunday.
    pub iso_weekday: u32,
    /// 0-based within the day.
    pub block: u32,
    pub is_first_block: bool,
    pub is_last_block: bool,
    /// Dense identity for the calendar day this slot belongs to, i.e. a unique
    /// `(week, weekday)` pair. Day-granularity constraints key on this rather
    /// than deriving a day from slot arithmetic.
    pub day_index: u32,
    pub week_kind: WeekKind,
    pub is_holiday: bool,
}

/// The tenant's grid, resolved. Built once per run.
#[derive(Clone, Debug)]
pub struct SlotTable {
    flags: Vec<SlotFlags>,
    blocks_per_day: u32,
    /// Sorted, deduplicated ISO weekdays that this tenant actually teaches on.
    active_days: Vec<u32>,
    week_count: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GridError {
    NoBlocksPerDay,
    NoActiveDays,
    NoWeeks,
    InvalidWeekday(u32),
}

impl std::fmt::Display for GridError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GridError::NoBlocksPerDay => write!(f, "time_grid.blocks_per_day must be >= 1"),
            GridError::NoActiveDays => write!(f, "time_grid.active_days must not be empty"),
            GridError::NoWeeks => write!(f, "calendar.weeks must not be empty"),
            GridError::InvalidWeekday(d) => {
                write!(f, "invalid ISO weekday {d}, expected 1..=7")
            }
        }
    }
}

impl std::error::Error for GridError {}

/// Per-week metadata, as supplied by the caller's academic calendar.
#[derive(Clone, Debug)]
pub struct WeekSpec {
    pub kind: WeekKind,
    /// ISO weekdays within this week that are holidays.
    pub holiday_weekdays: Vec<u32>,
}

impl SlotTable {
    pub fn build(
        blocks_per_day: u32,
        active_days: &[u32],
        weeks: &[WeekSpec],
    ) -> Result<Self, GridError> {
        if blocks_per_day == 0 {
            return Err(GridError::NoBlocksPerDay);
        }
        if active_days.is_empty() {
            return Err(GridError::NoActiveDays);
        }
        if weeks.is_empty() {
            return Err(GridError::NoWeeks);
        }
        for &d in active_days {
            if !(1..=7).contains(&d) {
                return Err(GridError::InvalidWeekday(d));
            }
        }

        let mut days: Vec<u32> = active_days.to_vec();
        days.sort_unstable();
        days.dedup();

        let week_count = weeks.len() as u32;
        let mut flags = Vec::with_capacity(weeks.len() * days.len() * blocks_per_day as usize);

        for (w, spec) in weeks.iter().enumerate() {
            for (day_pos, &day) in days.iter().enumerate() {
                let is_holiday = spec.holiday_weekdays.contains(&day);
                let day_index = (w * days.len() + day_pos) as u32;
                for block in 0..blocks_per_day {
                    flags.push(SlotFlags {
                        week: w as u32,
                        iso_weekday: day,
                        block,
                        is_first_block: block == 0,
                        is_last_block: block == blocks_per_day - 1,
                        day_index,
                        week_kind: spec.kind,
                        is_holiday,
                    });
                }
            }
        }

        Ok(Self {
            flags,
            blocks_per_day,
            active_days: days,
            week_count,
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.flags.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    #[inline]
    pub fn blocks_per_day(&self) -> u32 {
        self.blocks_per_day
    }

    #[inline]
    pub fn week_count(&self) -> u32 {
        self.week_count
    }

    #[inline]
    pub fn active_days(&self) -> &[u32] {
        &self.active_days
    }

    /// Number of distinct calendar days in the term — the dimension that
    /// day-granularity counters are sized against.
    #[inline]
    pub fn day_count(&self) -> usize {
        self.week_count as usize * self.active_days.len()
    }

    #[inline]
    pub fn flags(&self, slot: SlotIdx) -> &SlotFlags {
        &self.flags[slot.get()]
    }

    #[inline]
    pub fn all(&self) -> impl Iterator<Item = SlotIdx> {
        (0..self.flags.len() as u32).map(SlotIdx)
    }

    fn day_position(&self, iso_weekday: u32) -> Option<usize> {
        self.active_days.iter().position(|&d| d == iso_weekday)
    }

    /// Resolve an exact `(week, day, block)` address. Returns `None` if the day
    /// is not active for this tenant, or the address is out of range.
    pub fn resolve(&self, week: u32, iso_weekday: u32, block: u32) -> Option<SlotIdx> {
        if week >= self.week_count || block >= self.blocks_per_day {
            return None;
        }
        let day_pos = self.day_position(iso_weekday)?;
        let per_week = self.active_days.len() as u32 * self.blocks_per_day;
        Some(SlotIdx(
            week * per_week + day_pos as u32 * self.blocks_per_day + block,
        ))
    }

    /// The first slot at or after `(week, day, block)`.
    ///
    /// Used to resolve `reference_slot` — the caller's "now" — into something
    /// comparable, because a caller may legitimately hand us a timestamp
    /// falling on an inactive day (a Sunday), which [`Self::resolve`] rejects.
    /// Returns `None` when the address is after the entire term.
    pub fn lower_bound(&self, week: u32, iso_weekday: u32, block: u32) -> Option<SlotIdx> {
        if week >= self.week_count {
            return None;
        }
        let per_week = self.active_days.len() as u32 * self.blocks_per_day;

        // First active day at or after `iso_weekday`.
        match self.active_days.iter().position(|&d| d >= iso_weekday) {
            Some(pos) => {
                let exact_day = self.active_days[pos] == iso_weekday;
                let block = if exact_day { block.min(self.blocks_per_day) } else { 0 };
                if exact_day && block >= self.blocks_per_day {
                    // Past the last block of that day; roll to the next day.
                    return self.lower_bound(week, iso_weekday + 1, 0);
                }
                Some(SlotIdx(
                    week * per_week + pos as u32 * self.blocks_per_day + block,
                ))
            }
            // No active day left this week; roll into the next.
            None => self.lower_bound(week + 1, 1, 0),
        }
    }

    /// The slots a session occupies, given a start and a duration in blocks.
    ///
    /// Returns `None` if the session would spill past the end of its day — a
    /// session must be contiguous *within* one day, and block indices are
    /// per-day, so spilling is not representable.
    pub fn span(&self, start: SlotIdx, duration_blocks: u32) -> Option<Vec<SlotIdx>> {
        if duration_blocks == 0 || start.get() >= self.flags.len() {
            return None;
        }
        let f = self.flags(start);
        if f.block + duration_blocks > self.blocks_per_day {
            return None;
        }
        Some((0..duration_blocks).map(|i| SlotIdx(start.0 + i)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> SlotTable {
        // 2 weeks, Mon/Tue/Wed, 3 blocks/day.
        let weeks = vec![
            WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] },
            WeekSpec { kind: WeekKind::Exam, holiday_weekdays: vec![2] },
        ];
        SlotTable::build(3, &[1, 2, 3], &weeks).unwrap()
    }

    #[test]
    fn flattens_and_resolves() {
        let g = grid();
        assert_eq!(g.len(), 2 * 3 * 3);
        assert_eq!(g.resolve(0, 1, 0), Some(SlotIdx(0)));
        assert_eq!(g.resolve(0, 2, 0), Some(SlotIdx(3)));
        assert_eq!(g.resolve(1, 1, 0), Some(SlotIdx(9)));
        // Thursday is not an active day for this tenant.
        assert_eq!(g.resolve(0, 4, 0), None);
    }

    #[test]
    fn day_index_identifies_a_calendar_day() {
        let g = grid(); // 2 weeks, Mon/Tue/Wed, 3 blocks
        assert_eq!(g.day_count(), 6);

        // Every block of one day shares a day_index...
        let a = g.flags(g.resolve(0, 1, 0).unwrap()).day_index;
        let b = g.flags(g.resolve(0, 1, 2).unwrap()).day_index;
        assert_eq!(a, b);

        // ...and different days, or the same weekday in another week, do not.
        assert_ne!(a, g.flags(g.resolve(0, 2, 0).unwrap()).day_index);
        assert_ne!(a, g.flags(g.resolve(1, 1, 0).unwrap()).day_index);
        assert_eq!(g.flags(g.resolve(1, 3, 0).unwrap()).day_index, 5);
    }

    #[test]
    fn flags_are_precomputed_not_derived() {
        let g = grid();
        let first = g.flags(g.resolve(0, 1, 0).unwrap());
        assert!(first.is_first_block && !first.is_last_block);

        // "Last block" is blocks_per_day - 1, not a hardcoded index 2.
        let last = g.flags(g.resolve(0, 1, 2).unwrap());
        assert!(last.is_last_block && !last.is_first_block);

        let exam = g.flags(g.resolve(1, 1, 0).unwrap());
        assert_eq!(exam.week_kind, WeekKind::Exam);

        assert!(g.flags(g.resolve(1, 2, 0).unwrap()).is_holiday);
        assert!(!g.flags(g.resolve(1, 1, 0).unwrap()).is_holiday);
    }

    #[test]
    fn last_block_tracks_grid_width() {
        // Same calendar, 5 blocks/day: the last block is now index 4.
        let weeks = vec![WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] }];
        let g = SlotTable::build(5, &[1], &weeks).unwrap();
        assert!(!g.flags(g.resolve(0, 1, 2).unwrap()).is_last_block);
        assert!(g.flags(g.resolve(0, 1, 4).unwrap()).is_last_block);
    }

    #[test]
    fn lower_bound_rolls_over_inactive_days() {
        let g = grid();
        // Thursday week 0 -> first slot of Monday week 1.
        assert_eq!(g.lower_bound(0, 4, 0), Some(SlotIdx(9)));
        // Exact hit.
        assert_eq!(g.lower_bound(0, 2, 1), Some(SlotIdx(4)));
        // Past the end of the term.
        assert_eq!(g.lower_bound(2, 1, 0), None);
    }

    #[test]
    fn span_refuses_to_spill_past_end_of_day() {
        let g = grid();
        let start = g.resolve(0, 1, 1).unwrap();
        assert_eq!(g.span(start, 2).map(|v| v.len()), Some(2));
        // Block 1 + 3 blocks would run off a 3-block day.
        assert_eq!(g.span(start, 3), None);
    }

    #[test]
    fn rejects_degenerate_grids() {
        let weeks = vec![WeekSpec { kind: WeekKind::Teaching, holiday_weekdays: vec![] }];
        let err = |r: Result<SlotTable, GridError>| r.err();

        assert_eq!(err(SlotTable::build(0, &[1], &weeks)), Some(GridError::NoBlocksPerDay));
        assert_eq!(err(SlotTable::build(1, &[], &weeks)), Some(GridError::NoActiveDays));
        assert_eq!(err(SlotTable::build(1, &[1], &[])), Some(GridError::NoWeeks));
        assert_eq!(
            err(SlotTable::build(1, &[9], &weeks)),
            Some(GridError::InvalidWeekday(9))
        );
    }
}
