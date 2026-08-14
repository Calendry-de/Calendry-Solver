//! Nested-group closures.
//!
//! The parent/child conflict rule is evaluated in the local-search hot loop,
//! potentially millions of times per run, so it must read **precomputed
//! in-memory sets** rather than walking the tree live. Everything here is built
//! once per run and then only read.
//!
//! # The asymmetry, which is deliberate
//!
//! Two different expansions are needed, and conflating them is a real bug:
//!
//! * **Conflict** propagates in **both** directions. A conflict on a parent
//!   Group blocks its children, and a conflict on a child blocks its parent.
//!   So the conflict closure of `g` is `{g} ∪ ancestors(g) ∪ descendants(g)`.
//!
//! * **Attendance** propagates **downward only**. A Session assigned to cohort
//!   `A` involves everyone in `A`'s classes, but a Session assigned to class `C`
//!   does *not* pull in the whole cohort. So attendance expands over
//!   `{g} ∪ descendants(g)`.
//!
//! # Why the conflict check expands one side only
//!
//! Expanding *both* sessions' groups and intersecting is wrong. Siblings `C` and
//! `D` under cohort `A` would both have `A` in their closures, so the
//! intersection would be non-empty and two seminar groups meeting at the same
//! time — the normal case — would be reported as a clash.
//!
//! The rule is "same root-to-leaf path", not "shares an ancestor". So one side
//! is expanded to its closure and the other is tested by **identity**. That is
//! correct in either direction because closure membership is symmetric:
//! `y ∈ closure(x) ⟺ x ∈ closure(y)`.

use crate::bitset::BitSet;
use crate::ids::GroupIdx;

#[derive(Debug)]
pub struct GroupCycle(pub Vec<GroupIdx>);

impl std::fmt::Display for GroupCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "group hierarchy contains a cycle: {:?}", self.0)
    }
}

impl std::error::Error for GroupCycle {}

#[derive(Clone, Debug)]
pub struct GroupClosure {
    /// `{g} ∪ ancestors(g) ∪ descendants(g)` — for conflict propagation.
    conflict: Vec<BitSet>,
    /// `{g} ∪ descendants(g)` — for attendance resolution.
    subtree: Vec<BitSet>,
}

impl GroupClosure {
    pub fn build(parent_of: &[Option<GroupIdx>]) -> Result<Self, GroupCycle> {
        let n = parent_of.len();
        detect_cycle(parent_of)?;

        // Children lists.
        let mut children: Vec<Vec<GroupIdx>> = vec![Vec::new(); n];
        for (i, p) in parent_of.iter().enumerate() {
            if let Some(p) = p {
                children[p.get()].push(GroupIdx(i as u32));
            }
        }

        // Subtree ({g} ∪ descendants), computed bottom-up so each node unions
        // its children's already-complete subtrees.
        let mut subtree: Vec<BitSet> = (0..n).map(|_| BitSet::new(n)).collect();
        for g in postorder(&children, parent_of) {
            let i = g.get();
            subtree[i].insert(i);
            let kids = children[i].clone();
            for c in kids {
                let child = subtree[c.get()].clone();
                subtree[i].union_with(&child);
            }
        }

        // Conflict closure = subtree ∪ ancestor chain.
        let mut conflict = subtree.clone();
        for i in 0..n {
            let mut cur = parent_of[i];
            while let Some(p) = cur {
                conflict[i].insert(p.get());
                cur = parent_of[p.get()];
            }
        }

        Ok(Self { conflict, subtree })
    }

    pub fn len(&self) -> usize {
        self.conflict.len()
    }

    pub fn is_empty(&self) -> bool {
        self.conflict.is_empty()
    }

    /// Expand groups to the set that must be **marked** as busy.
    /// Ascending, deduplicated, deterministic.
    pub fn expand_conflict(&self, groups: &[GroupIdx]) -> Vec<GroupIdx> {
        self.expand(groups, &self.conflict)
    }

    /// Expand groups to the subtree used for attendance resolution.
    pub fn expand_subtree(&self, groups: &[GroupIdx]) -> Vec<GroupIdx> {
        self.expand(groups, &self.subtree)
    }

    fn expand(&self, groups: &[GroupIdx], table: &[BitSet]) -> Vec<GroupIdx> {
        if groups.is_empty() || table.is_empty() {
            return Vec::new();
        }
        let mut acc = BitSet::new(table.len());
        for g in groups {
            acc.union_with(&table[g.get()]);
        }
        acc.iter().map(|i| GroupIdx(i as u32)).collect()
    }

    /// True if `a` and `b` lie on the same root-to-leaf path — i.e. equal, or
    /// one is an ancestor of the other.
    #[inline]
    pub fn conflicts(&self, a: GroupIdx, b: GroupIdx) -> bool {
        self.conflict[a.get()].contains(b.get())
    }
}

fn detect_cycle(parent_of: &[Option<GroupIdx>]) -> Result<(), GroupCycle> {
    let n = parent_of.len();
    // 0 = unvisited, 1 = on current path, 2 = settled.
    let mut state = vec![0u8; n];

    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        let mut path = Vec::new();
        let mut cur = Some(GroupIdx(start as u32));

        while let Some(g) = cur {
            match state[g.get()] {
                1 => {
                    let at = path.iter().position(|&x: &GroupIdx| x == g).unwrap_or(0);
                    return Err(GroupCycle(path[at..].to_vec()));
                }
                2 => break,
                _ => {}
            }
            state[g.get()] = 1;
            path.push(g);
            cur = parent_of[g.get()];
        }

        for g in path {
            state[g.get()] = 2;
        }
    }
    Ok(())
}

/// Children-before-parents ordering. Cycle-free by the time this runs.
fn postorder(children: &[Vec<GroupIdx>], parent_of: &[Option<GroupIdx>]) -> Vec<GroupIdx> {
    let n = children.len();
    let mut out = Vec::with_capacity(n);
    let mut seen = vec![false; n];

    for (root, parent) in parent_of.iter().enumerate() {
        if parent.is_some() {
            continue;
        }
        // Iterative DFS emitting post-order.
        let mut stack = vec![(GroupIdx(root as u32), false)];
        while let Some((g, expanded)) = stack.pop() {
            if expanded {
                out.push(g);
                continue;
            }
            if seen[g.get()] {
                continue;
            }
            seen[g.get()] = true;
            stack.push((g, true));
            for &c in &children[g.get()] {
                stack.push((c, false));
            }
        }
    }

    // Defensive: any node not reached from a root (should not happen once
    // cycles are rejected) still needs an entry.
    for (i, &was_seen) in seen.iter().enumerate() {
        if !was_seen {
            out.push(GroupIdx(i as u32));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cohort A(0) -> classes B(1), C(2); C -> seminars D(3), E(4). F(5) separate root.
    fn tree() -> Vec<Option<GroupIdx>> {
        vec![
            None,
            Some(GroupIdx(0)),
            Some(GroupIdx(0)),
            Some(GroupIdx(2)),
            Some(GroupIdx(2)),
            None,
        ]
    }

    #[test]
    fn conflict_closure_covers_both_directions() {
        let c = GroupClosure::build(&tree()).unwrap();

        // A conflicts with everything beneath it.
        for g in [0u32, 1, 2, 3, 4] {
            assert!(c.conflicts(GroupIdx(0), GroupIdx(g)), "A vs {g}");
        }
        // A deep seminar conflicts with its ancestors, both ways.
        assert!(c.conflicts(GroupIdx(3), GroupIdx(0)));
        assert!(c.conflicts(GroupIdx(0), GroupIdx(3)));
        assert!(c.conflicts(GroupIdx(3), GroupIdx(2)));
    }

    #[test]
    fn siblings_do_not_conflict() {
        let c = GroupClosure::build(&tree()).unwrap();
        // B and C are siblings under A.
        assert!(!c.conflicts(GroupIdx(1), GroupIdx(2)));
        assert!(!c.conflicts(GroupIdx(2), GroupIdx(1)));
        // D and E are siblings under C.
        assert!(!c.conflicts(GroupIdx(3), GroupIdx(4)));
        // Different roots never conflict.
        assert!(!c.conflicts(GroupIdx(0), GroupIdx(5)));
    }

    #[test]
    fn expanding_one_side_reproduces_the_pairwise_rule() {
        let c = GroupClosure::build(&tree()).unwrap();
        // Marking a session on B must not make C look busy.
        let marked = c.expand_conflict(&[GroupIdx(1)]);
        assert!(!marked.contains(&GroupIdx(2)), "sibling must not be marked");
        assert!(marked.contains(&GroupIdx(0)), "ancestor must be marked");

        // Marking a cohort-level session blocks every descendant class.
        let marked = c.expand_conflict(&[GroupIdx(0)]);
        for g in [0u32, 1, 2, 3, 4] {
            assert!(marked.contains(&GroupIdx(g)), "A should block {g}");
        }
        assert!(!marked.contains(&GroupIdx(5)));
    }

    #[test]
    fn attendance_expands_downward_only() {
        let c = GroupClosure::build(&tree()).unwrap();
        // Cohort session pulls in every descendant class.
        assert_eq!(
            c.expand_subtree(&[GroupIdx(0)]),
            vec![GroupIdx(0), GroupIdx(1), GroupIdx(2), GroupIdx(3), GroupIdx(4)]
        );
        // Seminar session pulls in only itself — NOT the cohort.
        assert_eq!(c.expand_subtree(&[GroupIdx(3)]), vec![GroupIdx(3)]);
        // Class C pulls in its seminars but not its parent.
        assert_eq!(
            c.expand_subtree(&[GroupIdx(2)]),
            vec![GroupIdx(2), GroupIdx(3), GroupIdx(4)]
        );
    }

    #[test]
    fn deep_chains_are_fully_transitive() {
        // 0 <- 1 <- 2 <- 3 <- 4
        let parents = vec![
            None,
            Some(GroupIdx(0)),
            Some(GroupIdx(1)),
            Some(GroupIdx(2)),
            Some(GroupIdx(3)),
        ];
        let c = GroupClosure::build(&parents).unwrap();
        assert!(c.conflicts(GroupIdx(0), GroupIdx(4)), "root vs leaf, 4 hops");
        assert!(c.conflicts(GroupIdx(4), GroupIdx(0)));
        assert_eq!(c.expand_subtree(&[GroupIdx(0)]).len(), 5);
        assert_eq!(c.expand_subtree(&[GroupIdx(4)]).len(), 1);
    }

    #[test]
    fn rejects_cycles() {
        assert!(GroupClosure::build(&[Some(GroupIdx(1)), Some(GroupIdx(0))]).is_err());
        assert!(GroupClosure::build(&[Some(GroupIdx(0))]).is_err());
        assert!(GroupClosure::build(&tree()).is_ok());
    }
}
