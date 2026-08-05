//! Shared vocabulary for supervised Rebis execution.
//!
//! Frontends own different process and rendering mechanisms, but a queued run
//! must mean the same thing in a terminal tree and a visual card. Keeping these
//! closed enums in core prevents boolean combinations and string labels from
//! drifting between interfaces.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Program,
    Block,
}

impl Scope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Program => "program",
            Self::Block => "block",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    AwaitingPermission,
    Queued,
    Running,
    Complete,
    Cancelled,
}

impl State {
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Cancelled)
    }

    pub const fn label(self, paused: bool) -> &'static str {
        match (self, paused) {
            (Self::AwaitingPermission, _) => "PERMISSION",
            (Self::Queued, _) => "QUEUED",
            (Self::Running, true) => "PAUSED",
            (Self::Running, false) => "RUNNING",
            (Self::Complete, _) => "DONE",
            (Self::Cancelled, _) => "CANCELLED",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Dry,
    Direct,
    Chaos,
}

impl Mode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dry => "dry",
            Self::Direct => "direct",
            Self::Chaos => "chaos",
        }
    }

    pub const fn live(self) -> bool {
        !matches!(self, Self::Dry)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lane {
    Serial,
    Parallel,
}

impl Lane {
    pub const fn parallel(self) -> bool {
        matches!(self, Self::Parallel)
    }
}

/// How deep a run may sit beneath another before the nesting is refused.
///
/// A program that writes a program can write one that writes a program. That is
/// the point of it, and it is also how a run tree becomes unbounded without
/// anything looking wrong at any single level. The limit is generous enough for
/// real work — a review that plans, executes, and verifies is three — and small
/// enough that a runaway is caught while a reader can still see what happened.
pub const MAX_NESTING: usize = 8;

/// Where a run sits in the tree of runs.
///
/// A run opened by an agent inside another run is that run's child. The
/// relation is kept as data rather than as rendering, because both frontends
/// need the same answers — how deep is this, may it open another, what does it
/// belong under — and a tree drawn twice is a tree that disagrees with itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct Lineage {
    /// The run this one was opened from, if any.
    pub parent: Option<u64>,
    /// How many runs stand above this one. A root is zero.
    pub depth: usize,
}

impl Lineage {
    /// A run started by a person rather than by another run.
    #[must_use]
    pub const fn root() -> Self {
        Self {
            parent: None,
            depth: 0,
        }
    }

    /// The lineage of a run opened from inside `parent`.
    #[must_use]
    pub const fn under(parent: u64, parent_lineage: Self) -> Self {
        Self {
            parent: Some(parent),
            depth: parent_lineage.depth + 1,
        }
    }

    /// Whether a run at this lineage may open another beneath it.
    #[must_use]
    pub const fn may_nest(self) -> bool {
        self.depth + 1 < MAX_NESTING
    }

    /// Whether this run was opened by another.
    #[must_use]
    pub const fn nested(self) -> bool {
        self.parent.is_some()
    }
}

/// Order runs for display: each parent immediately followed by its descendants.
///
/// Both frontends draw the tree — one with indentation in a terminal, one with
/// nested cards — and both need the same order and the same depth for each row.
/// The algorithm lives here so a run cannot appear under one parent in the
/// terminal and another in the window.
///
/// `rows` is `(id, parent)` in the order the runs were created. The result is
/// `(depth, id)`. Runs whose parent is missing — cancelled, evicted from a
/// bounded history — are treated as roots rather than dropped: a child that
/// outlives its parent is still a run somebody needs to see.
#[must_use]
pub fn tree_order(rows: &[(u64, Option<u64>)]) -> Vec<(usize, u64)> {
    let known: std::collections::HashSet<u64> = rows.iter().map(|(id, _)| *id).collect();
    let mut ordered = Vec::with_capacity(rows.len());
    let mut placed = std::collections::HashSet::new();

    // Depth-first from each root, in creation order, so a tree reads top to
    // bottom the way it happened.
    fn walk(
        parent: Option<u64>,
        depth: usize,
        rows: &[(u64, Option<u64>)],
        placed: &mut std::collections::HashSet<u64>,
        ordered: &mut Vec<(usize, u64)>,
    ) {
        for (id, own) in rows {
            if *own != parent || !placed.insert(*id) {
                continue;
            }
            ordered.push((depth, *id));
            // A cycle cannot arise from ids that only ever point backwards, but
            // the guard costs nothing and the data comes from two frontends.
            if depth + 1 < MAX_NESTING {
                walk(Some(*id), depth + 1, rows, placed, ordered);
            }
        }
    }

    walk(None, 0, rows, &mut placed, &mut ordered);
    // Anything whose parent is gone, or that the depth guard stopped, still has
    // to appear.
    for (id, parent) in rows {
        if placed.contains(id) {
            continue;
        }
        let orphan = parent.is_some_and(|parent| !known.contains(&parent));
        let depth = usize::from(!orphan);
        placed.insert(*id);
        ordered.push((depth, *id));
    }
    ordered
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Authority {
    Ask,
    Once,
    Session,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_finished_states_are_terminal() {
        assert!(State::Complete.terminal());
        assert!(State::Cancelled.terminal());
        assert!(!State::Queued.terminal());
        assert!(!State::Running.terminal());
    }

    #[test]
    fn a_run_opened_from_a_run_knows_what_it_sits_under() {
        let root = Lineage::root();
        assert!(!root.nested());
        assert_eq!(root.depth, 0);

        let child = Lineage::under(7, root);
        assert_eq!(child.parent, Some(7));
        assert_eq!(child.depth, 1);
        assert!(child.nested());

        // Nesting is allowed until the limit, and refused at it — checked from
        // the parent, so a run knows before it spawns rather than after.
        let mut lineage = Lineage::root();
        let mut opened = 0;
        while lineage.may_nest() {
            lineage = Lineage::under(opened as u64, lineage);
            opened += 1;
            assert!(opened < 100, "nesting never refused");
        }
        assert_eq!(lineage.depth, MAX_NESTING - 1);
        assert!(!lineage.may_nest(), "the limit must be reachable");
    }

    #[test]
    fn runs_are_ordered_as_a_tree_with_each_child_under_its_parent() {
        // Created in this order: two roots, then children of the first, then a
        // grandchild — the order an agent would actually open them in.
        let rows = [
            (1, None),
            (2, None),
            (3, Some(1)),
            (4, Some(1)),
            (5, Some(3)),
            (6, Some(2)),
        ];
        assert_eq!(
            tree_order(&rows),
            vec![(0, 1), (1, 3), (2, 5), (1, 4), (0, 2), (1, 6),],
            "each parent must be followed immediately by its own descendants"
        );

        // A child whose parent is gone is still shown, as a root of its own —
        // dropping it would hide a run that is still going.
        let orphaned = [(1, None), (9, Some(404))];
        let ordered = tree_order(&orphaned);
        assert_eq!(ordered.len(), 2, "{ordered:?}");
        assert!(ordered.contains(&(0, 9)), "{ordered:?}");

        // Every run appears exactly once, whatever the shape.
        for rows in [
            vec![],
            vec![(1, None)],
            vec![(1, Some(1))],
            vec![(1, Some(2)), (2, Some(1))],
        ] {
            let ordered = tree_order(&rows);
            assert_eq!(ordered.len(), rows.len(), "{rows:?} -> {ordered:?}");
            let seen: std::collections::HashSet<u64> = ordered.iter().map(|(_, id)| *id).collect();
            assert_eq!(seen.len(), rows.len(), "a run appeared twice: {ordered:?}");
        }
    }

    #[test]
    fn dry_is_the_only_non_live_mode() {
        assert!(!Mode::Dry.live());
        assert!(Mode::Direct.live());
        assert!(Mode::Chaos.live());
    }
}
