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
    fn dry_is_the_only_non_live_mode() {
        assert!(!Mode::Dry.live());
        assert!(Mode::Direct.live());
        assert!(Mode::Chaos.live());
    }
}
