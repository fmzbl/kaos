//! The Eight Rays of Chaos — the routing domains.
//!
//! *Liber Kaos* enumerates **eight magics**, each a colour of the octarine
//! spectrum, each governing a different sphere of work. kaos keeps Carroll's
//! attributions and reads them as **specialist domains for a mixture-of-experts
//! router**: every task belongs to a ray, and the Pact routes it to the adept
//! whose affinity for that ray is strongest. Matching the ray raises **G** (the
//! gnosis factor) in [`crate::equation`] — a competence match is a stronger
//! altered state. Octarine is reserved: it is the pure magical power itself, the
//! ray of the Magus/orchestrator, not a worker domain.

use crate::equation::clamp01;

/// The eight colours of magic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ray {
    /// Pure magic, the magician-self. The orchestrator's own ray. Meta-work.
    Octarine,
    /// Death, entropy, dissolution. Deletions, teardown, deprecation, refactors
    /// that remove.
    Black,
    /// Wealth, expansion, resources (Jupiter). Dependencies, infra, scaling, build.
    Blue,
    /// War, energy, vitality (Mars). Debugging under fire, concurrency, perf,
    /// crashes. **The principal ray of kaos.**
    Red,
    /// Ego, the self, identity (Sol). Core architecture, public API, the system's
    /// own identity and contracts.
    Yellow,
    /// Love, harmony, union (Venus). UX, integration, interfaces, making parts fit.
    Green,
    /// Cunning, intellect, trickery (Mercury). Algorithms, parsing, optimization,
    /// clever tricks.
    Orange,
    /// Lust, creation, generation (the generative current). New features, codegen,
    /// scaffolding from nothing.
    Purple,
}

impl Ray {
    /// Stable exhaustive array index for per-ray state.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Octarine => 0,
            Self::Black => 1,
            Self::Blue => 2,
            Self::Red => 3,
            Self::Yellow => 4,
            Self::Green => 5,
            Self::Orange => 6,
            Self::Purple => 7,
        }
    }

    /// All eight, octarine first.
    pub fn all() -> [Ray; 8] {
        use Ray::*;
        [Octarine, Black, Blue, Red, Yellow, Green, Orange, Purple]
    }

    /// The seven worker rays (everything but Octarine, which is the Magus').
    pub fn worker_rays() -> [Ray; 7] {
        use Ray::*;
        [Black, Blue, Red, Yellow, Green, Orange, Purple]
    }

    pub fn name(&self) -> &'static str {
        use Ray::*;
        match self {
            Octarine => "Octarine",
            Black => "Black",
            Blue => "Blue",
            Red => "Red",
            Yellow => "Yellow",
            Green => "Green",
            Orange => "Orange",
            Purple => "Purple",
        }
    }

    /// The sphere this ray governs — used to explain a routing decision.
    pub fn sphere(&self) -> &'static str {
        use Ray::*;
        match self {
            Octarine => "pure magic — the Magus' own work",
            Black => "death & entropy — deletion, teardown, deprecation",
            Blue => "wealth & expansion — deps, infra, scaling, build",
            Red => "war & vitality — debugging, concurrency, perf, crashes",
            Yellow => "ego & identity — core architecture, public API",
            Green => "love & union — UX, integration, interfaces",
            Orange => "cunning & intellect — algorithms, parsing, optimization",
            Purple => "lust & creation — new features, codegen, scaffolding",
        }
    }

    /// A 24-bit ANSI colour for the red-forward TUI. Red is foregrounded; the rest
    /// are muted so the principal ray dominates the palette.
    pub fn rgb(&self) -> (u8, u8, u8) {
        use Ray::*;
        match self {
            Octarine => (190, 70, 90), // a magic-pink leaning red
            Black => (110, 70, 78),
            Blue => (120, 96, 120),
            Red => (220, 40, 48), // the principal colour
            Yellow => (170, 120, 80),
            Green => (110, 120, 96),
            Orange => (190, 96, 70),
            Purple => (150, 80, 110),
        }
    }

    /// Keywords that pull a task toward this ray. Crude but deterministic — a real
    /// classifier would replace this, but it makes routing legible and testable.
    fn lexicon(&self) -> &'static [&'static str] {
        use Ray::*;
        match self {
            Octarine => &["orchestrate", "meta", "strategy", "plan", "route"],
            Black => &[
                "delete",
                "remove",
                "deprecate",
                "teardown",
                "drop",
                "purge",
                "kill",
            ],
            Blue => &[
                "dependency",
                "deps",
                "infra",
                "build",
                "scale",
                "package",
                "version",
                "ci",
            ],
            Red => &[
                "bug", "crash", "panic", "race", "deadlock", "perf", "slow", "debug", "fix",
                "error",
            ],
            Yellow => &[
                "architecture",
                "api",
                "interface",
                "contract",
                "core",
                "design",
                "schema",
            ],
            Green => &[
                "ui",
                "ux",
                "integrate",
                "frontend",
                "style",
                "layout",
                "accessib",
                "merge",
            ],
            Orange => &[
                "algorithm",
                "parse",
                "optimi",
                "regex",
                "sort",
                "search",
                "encode",
                "compress",
            ],
            Purple => &[
                "feature",
                "scaffold",
                "generate",
                "new",
                "create",
                "codegen",
                "boilerplate",
            ],
        }
    }

    /// Score how strongly a free-text task belongs to this ray (0..). A simple
    /// keyword count over the lowercased task description.
    pub fn affinity_for(&self, task: &str) -> u32 {
        let t = task.to_lowercase();
        self.lexicon().iter().filter(|kw| t.contains(*kw)).count() as u32
    }

    /// Classify a task to its most plausible worker ray. A total miss (no keyword
    /// scores at all) falls to Red — the principal ray, and the right default for
    /// "something is wrong, make it work." A tie between scoring rays falls to
    /// whichever comes first in [`Ray::worker_rays`] order, since only a strictly
    /// higher score displaces the leader.
    pub fn classify(task: &str) -> Ray {
        let mut best = Ray::Red;
        let mut best_score = 0u32;
        for ray in Ray::worker_rays() {
            let s = ray.affinity_for(task);
            if s > best_score {
                best_score = s;
                best = ray;
            }
        }
        best
    }
}

/// A worker ray's *competence vector* over the eight rays — how good an adept of
/// this home ray is at tasks of each ray. Diagonal-dominant: you are best in your
/// home sphere, weaker elsewhere. This is what makes routing pay off.
pub fn competence(home: Ray, task: Ray) -> f64 {
    if home == task {
        return 0.95; // a master in their own sphere — Carroll's 0.8–0.9+ gnosis band
    }
    // Adjacent rays on the colour wheel share some competence; opposite rays least.
    // A generalist forced off their ray is markedly weaker — which is exactly what
    // makes routing to the specialist pay.
    let hi = home.index();
    let ti = task.index();
    let dist = ((hi as i32 - ti as i32).abs()).min(8 - (hi as i32 - ti as i32).abs());
    clamp01(0.70 - 0.06 * dist as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_ray_is_most_competent() {
        for home in Ray::worker_rays() {
            let home_c = competence(home, home);
            for task in Ray::worker_rays() {
                if task != home {
                    assert!(home_c > competence(home, task));
                }
            }
        }
    }

    #[test]
    fn classify_routes_obvious_tasks() {
        assert_eq!(Ray::classify("fix the panic in the parser"), Ray::Red);
        assert_eq!(Ray::classify("delete the deprecated module"), Ray::Black);
        assert_eq!(Ray::classify("optimize the sort algorithm"), Ray::Orange);
        assert_eq!(Ray::classify("scaffold a new feature"), Ray::Purple);
        assert_eq!(
            Ray::classify("bump the dependency and fix ci build"),
            Ray::Blue
        );
    }

    #[test]
    fn unknown_task_falls_to_red() {
        assert_eq!(Ray::classify("zzz qqq"), Ray::Red);
    }
}
