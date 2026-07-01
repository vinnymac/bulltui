//! The keybinding registry: a single source of truth so the `?` help overlay
//! ([`crate::ui::overlay::draw_help`]) and the status-line hints
//! ([`crate::ui::draw_status`]) can never drift apart. Add or change a key in
//! one place here and both surfaces update together; a unit test ties every
//! status hint back to a documented binding.

use crate::app::Screen;

/// A logical grouping of bindings — a help-section header, and the scope a
/// binding applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Global,
    Overview,
    Queue,
    Job,
    Schedulers,
    Workers,
    Events,
    Input,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Global => "Global",
            Group::Overview => "Overview",
            Group::Queue => "Queue",
            Group::Job => "Job (queue & detail)",
            Group::Schedulers => "Schedulers",
            Group::Workers => "Workers / Busy",
            Group::Events => "Events feed",
            Group::Input => "Input dialogs",
        }
    }
}

/// One documented keybinding. `keys` / `desc` are the verbose form rendered in
/// the help overlay.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub group: Group,
    pub keys: &'static str,
    pub desc: &'static str,
}

/// A compact status-line hint. `binding` is the exact [`Binding::keys`] this
/// hint stands for; the lock test asserts that binding actually exists, so a
/// hint can never advertise a key the help doesn't document.
#[derive(Debug, Clone, Copy)]
pub struct Hint {
    pub keys: &'static str,
    pub label: &'static str,
    pub binding: &'static str,
}

/// Help sections, in display order.
pub const GROUP_ORDER: [Group; 8] = [
    Group::Global,
    Group::Overview,
    Group::Queue,
    Group::Job,
    Group::Schedulers,
    Group::Workers,
    Group::Events,
    Group::Input,
];

/// Every documented binding. Order within a group is the help display order.
pub const BINDINGS: &[Binding] = &[
    // Global
    Binding {
        group: Group::Global,
        keys: "q / Ctrl+C",
        desc: "quit",
    },
    Binding {
        group: Group::Global,
        keys: "?",
        desc: "toggle this help",
    },
    Binding {
        group: Group::Global,
        keys: "Esc",
        desc: "back / close",
    },
    Binding {
        group: Group::Global,
        keys: "i",
        desc: "Redis stats",
    },
    Binding {
        group: Group::Global,
        keys: ",",
        desc: "settings",
    },
    Binding {
        group: Group::Global,
        keys: "Ctrl+R",
        desc: "refresh now",
    },
    Binding {
        group: Group::Global,
        keys: ":",
        desc: "command palette",
    },
    Binding {
        group: Group::Global,
        keys: "w",
        desc: "workers / busy view",
    },
    Binding {
        group: Group::Global,
        keys: "E",
        desc: "live events feed",
    },
    Binding {
        group: Group::Global,
        keys: "Ctrl+O",
        desc: "toggle mouse capture (on by default; Shift/⌥-drag selects text, off restores native selection)",
    },
    // Overview
    Binding {
        group: Group::Overview,
        keys: "↑↓ / j k",
        desc: "move selection",
    },
    Binding {
        group: Group::Overview,
        keys: "Enter / →",
        desc: "open queue",
    },
    Binding {
        group: Group::Overview,
        keys: "s",
        desc: "cycle sort",
    },
    Binding {
        group: Group::Overview,
        keys: "v",
        desc: "toggle table / bar view",
    },
    Binding {
        group: Group::Overview,
        keys: "/",
        desc: "search queues",
    },
    Binding {
        group: Group::Overview,
        keys: "f",
        desc: "filter queues by status",
    },
    Binding {
        group: Group::Overview,
        keys: "Esc",
        desc: "clear active search / filter",
    },
    Binding {
        group: Group::Overview,
        keys: "P / R",
        desc: "pause / resume ALL queues",
    },
    Binding {
        group: Group::Overview,
        keys: "S",
        desc: "job schedulers (selected queue)",
    },
    // Queue
    Binding {
        group: Group::Queue,
        keys: "↑↓",
        desc: "move job",
    },
    Binding {
        group: Group::Queue,
        keys: "Tab / S-Tab",
        desc: "next / prev status tab",
    },
    Binding {
        group: Group::Queue,
        keys: "[ ]",
        desc: "previous / next page",
    },
    Binding {
        group: Group::Queue,
        keys: "Enter / →",
        desc: "open job detail",
    },
    Binding {
        group: Group::Queue,
        keys: "p",
        desc: "pause / resume queue",
    },
    Binding {
        group: Group::Queue,
        keys: "e / o",
        desc: "empty (drain) / obliterate",
    },
    Binding {
        group: Group::Queue,
        keys: "c",
        desc: "clean current status",
    },
    Binding {
        group: Group::Queue,
        keys: "A / M",
        desc: "retry-all (failed/completed) / promote-all (delayed)",
    },
    Binding {
        group: Group::Queue,
        keys: "a / C",
        desc: "add job / set global concurrency",
    },
    Binding {
        group: Group::Queue,
        keys: "m",
        desc: "queue metrics",
    },
    Binding {
        group: Group::Queue,
        keys: "/",
        desc: "filter jobs (live; ! negates)",
    },
    Binding {
        group: Group::Queue,
        keys: "Space",
        desc: "toggle job selection",
    },
    Binding {
        group: Group::Queue,
        keys: "v",
        desc: "range-select jobs",
    },
    Binding {
        group: Group::Queue,
        keys: "S",
        desc: "job schedulers",
    },
    Binding {
        group: Group::Queue,
        keys: "R",
        desc: "reschedule delay (Delayed tab)",
    },
    Binding {
        group: Group::Queue,
        keys: "#",
        desc: "re-prioritize (Prioritized tab)",
    },
    // Job (queue & detail)
    Binding {
        group: Group::Job,
        keys: "↑↓",
        desc: "scroll detail · move flow node (Flow tab)",
    },
    Binding {
        group: Group::Job,
        keys: "PgUp/PgDn",
        desc: "page the detail body / flow cursor",
    },
    Binding {
        group: Group::Job,
        keys: "g / G",
        desc: "jump to top / bottom (Home/End)",
    },
    Binding {
        group: Group::Job,
        keys: "Enter",
        desc: "jump to selected flow node (Flow tab)",
    },
    Binding {
        group: Group::Job,
        keys: "r",
        desc: "retry job",
    },
    Binding {
        group: Group::Job,
        keys: "P",
        desc: "promote job",
    },
    Binding {
        group: Group::Job,
        keys: "d / x",
        desc: "remove job",
    },
    Binding {
        group: Group::Job,
        keys: "D",
        desc: "duplicate job",
    },
    Binding {
        group: Group::Job,
        keys: "u",
        desc: "update job data",
    },
    Binding {
        group: Group::Job,
        keys: "Tab",
        desc: "switch detail tab",
    },
    Binding {
        group: Group::Job,
        keys: "y",
        desc: "copy current tab to clipboard",
    },
    // Schedulers
    Binding {
        group: Group::Schedulers,
        keys: "↑↓ / j k",
        desc: "move selection",
    },
    Binding {
        group: Group::Schedulers,
        keys: "t",
        desc: "trigger scheduler now",
    },
    Binding {
        group: Group::Schedulers,
        keys: "d / x",
        desc: "remove scheduler",
    },
    Binding {
        group: Group::Schedulers,
        keys: "r",
        desc: "refresh",
    },
    // Workers / Busy
    Binding {
        group: Group::Workers,
        keys: "↑↓ / j k",
        desc: "move selection",
    },
    Binding {
        group: Group::Workers,
        keys: "Tab",
        desc: "switch Busy / Workers",
    },
    Binding {
        group: Group::Workers,
        keys: "Enter / →",
        desc: "open job (Busy tab)",
    },
    Binding {
        group: Group::Workers,
        keys: "r",
        desc: "refresh",
    },
    // Events feed
    Binding {
        group: Group::Events,
        keys: "↑↓ / j k",
        desc: "scroll (up pauses follow)",
    },
    Binding {
        group: Group::Events,
        keys: "f",
        desc: "toggle follow",
    },
    Binding {
        group: Group::Events,
        keys: "p / Space",
        desc: "pause / resume ingest",
    },
    Binding {
        group: Group::Events,
        keys: "/",
        desc: "filter feed",
    },
    Binding {
        group: Group::Events,
        keys: "s",
        desc: "cycle scope (all / this queue)",
    },
    Binding {
        group: Group::Events,
        keys: "n / N",
        desc: "next / prev failure",
    },
    Binding {
        group: Group::Events,
        keys: "Enter / →",
        desc: "open the event's job",
    },
    // Input dialogs
    Binding {
        group: Group::Input,
        keys: "Ctrl+U",
        desc: "clear the focused field",
    },
    Binding {
        group: Group::Input,
        keys: "Ctrl+W",
        desc: "delete previous word",
    },
    Binding {
        group: Group::Input,
        keys: "Ctrl+Enter",
        desc: "submit (multi-line forms)",
    },
];

const OVERVIEW_STATUS: &[Hint] = &[
    Hint {
        keys: "↑↓",
        label: "move",
        binding: "↑↓ / j k",
    },
    Hint {
        keys: "⏎",
        label: "open",
        binding: "Enter / →",
    },
    Hint {
        keys: "s",
        label: "sort",
        binding: "s",
    },
    Hint {
        keys: "v",
        label: "view",
        binding: "v",
    },
    Hint {
        keys: "/",
        label: "search",
        binding: "/",
    },
    Hint {
        keys: "f",
        label: "filter",
        binding: "f",
    },
    Hint {
        keys: "P/R",
        label: "pause/resume all",
        binding: "P / R",
    },
    Hint {
        keys: ":",
        label: "cmd",
        binding: ":",
    },
    Hint {
        keys: "?",
        label: "help",
        binding: "?",
    },
];

const QUEUE_STATUS: &[Hint] = &[
    Hint {
        keys: "↑↓",
        label: "job",
        binding: "↑↓",
    },
    Hint {
        keys: "⇥",
        label: "tab",
        binding: "Tab / S-Tab",
    },
    Hint {
        keys: "/",
        label: "filter",
        binding: "/",
    },
    Hint {
        keys: "␣",
        label: "select",
        binding: "Space",
    },
    Hint {
        keys: "⏎",
        label: "detail",
        binding: "Enter / →",
    },
    Hint {
        keys: "p",
        label: "pause",
        binding: "p",
    },
    Hint {
        keys: "r/d",
        label: "retry/remove",
        binding: "r",
    },
    Hint {
        keys: ":",
        label: "cmd",
        binding: ":",
    },
    Hint {
        keys: "?",
        label: "help",
        binding: "?",
    },
];

const JOB_STATUS: &[Hint] = &[
    Hint {
        keys: "⇥",
        label: "tab",
        binding: "Tab",
    },
    Hint {
        keys: "↑↓",
        label: "scroll/flow",
        binding: "↑↓",
    },
    Hint {
        keys: "⇟",
        label: "page",
        binding: "PgUp/PgDn",
    },
    Hint {
        keys: "⏎",
        label: "open node",
        binding: "Enter",
    },
    Hint {
        keys: "y",
        label: "copy",
        binding: "y",
    },
    Hint {
        keys: ":",
        label: "cmd",
        binding: ":",
    },
    Hint {
        keys: "r",
        label: "retry",
        binding: "r",
    },
    Hint {
        keys: "d",
        label: "remove",
        binding: "d / x",
    },
    Hint {
        keys: "esc",
        label: "back",
        binding: "Esc",
    },
    Hint {
        keys: "?",
        label: "help",
        binding: "?",
    },
];

const SCHEDULERS_STATUS: &[Hint] = &[
    Hint {
        keys: "↑↓",
        label: "move",
        binding: "↑↓ / j k",
    },
    Hint {
        keys: "t",
        label: "trigger",
        binding: "t",
    },
    Hint {
        keys: "d",
        label: "remove",
        binding: "d / x",
    },
    Hint {
        keys: "r",
        label: "refresh",
        binding: "r",
    },
    Hint {
        keys: "w",
        label: "busy",
        binding: "w",
    },
    Hint {
        keys: ":",
        label: "cmd",
        binding: ":",
    },
    Hint {
        keys: "esc",
        label: "back",
        binding: "Esc",
    },
    Hint {
        keys: "?",
        label: "help",
        binding: "?",
    },
];

const WORKERS_STATUS: &[Hint] = &[
    Hint {
        keys: "↑↓",
        label: "move",
        binding: "↑↓ / j k",
    },
    Hint {
        keys: "⇥",
        label: "tab",
        binding: "Tab",
    },
    Hint {
        keys: "⏎",
        label: "job",
        binding: "Enter / →",
    },
    Hint {
        keys: "r",
        label: "refresh",
        binding: "r",
    },
    Hint {
        keys: "E",
        label: "events",
        binding: "E",
    },
    Hint {
        keys: ":",
        label: "cmd",
        binding: ":",
    },
    Hint {
        keys: "esc",
        label: "back",
        binding: "Esc",
    },
    Hint {
        keys: "?",
        label: "help",
        binding: "?",
    },
];

const EVENTS_STATUS: &[Hint] = &[
    Hint {
        keys: "↑↓",
        label: "scroll",
        binding: "↑↓ / j k",
    },
    Hint {
        keys: "f",
        label: "follow",
        binding: "f",
    },
    Hint {
        keys: "p",
        label: "pause",
        binding: "p / Space",
    },
    Hint {
        keys: "/",
        label: "filter",
        binding: "/",
    },
    Hint {
        keys: "s",
        label: "scope",
        binding: "s",
    },
    Hint {
        keys: "n",
        label: "fail",
        binding: "n / N",
    },
    Hint {
        keys: "⏎",
        label: "open",
        binding: "Enter / →",
    },
    Hint {
        keys: "esc",
        label: "back",
        binding: "Esc",
    },
    Hint {
        keys: "?",
        label: "help",
        binding: "?",
    },
];

/// The compact hints shown in the status line for `screen`.
pub fn status_hints(screen: Screen) -> &'static [Hint] {
    match screen {
        Screen::Overview => OVERVIEW_STATUS,
        Screen::Queue => QUEUE_STATUS,
        Screen::Job => JOB_STATUS,
        Screen::Schedulers => SCHEDULERS_STATUS,
        Screen::Workers => WORKERS_STATUS,
        Screen::Events => EVENTS_STATUS,
    }
}

/// All bindings in `group`, in help display order.
pub fn bindings_in(group: Group) -> impl Iterator<Item = &'static Binding> {
    BINDINGS.iter().filter(move |b| b.group == group)
}

/// The help sections relevant to `screen` (contextual `?` help): always Global
/// and Input, plus the screen's own group(s). The queue list also shares the
/// per-job actions in the Job group.
pub fn help_groups(screen: Screen) -> Vec<Group> {
    let mut groups = vec![Group::Global];
    match screen {
        Screen::Overview => groups.push(Group::Overview),
        Screen::Queue => {
            groups.push(Group::Queue);
            groups.push(Group::Job);
        }
        Screen::Job => groups.push(Group::Job),
        Screen::Schedulers => groups.push(Group::Schedulers),
        Screen::Workers => groups.push(Group::Workers),
        Screen::Events => groups.push(Group::Events),
    }
    groups.push(Group::Input);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every status hint must point at a binding the help actually documents,
    /// so the two surfaces can't drift.
    #[test]
    fn status_hints_reference_real_bindings() {
        for screen in [
            Screen::Overview,
            Screen::Queue,
            Screen::Job,
            Screen::Schedulers,
            Screen::Workers,
            Screen::Events,
        ] {
            for h in status_hints(screen) {
                assert!(
                    BINDINGS.iter().any(|b| b.keys == h.binding),
                    "status hint {:?} ({:?}) on {screen:?} references unknown binding {:?}",
                    h.keys,
                    h.label,
                    h.binding,
                );
            }
        }
    }

    #[test]
    fn every_group_has_bindings() {
        for g in GROUP_ORDER {
            assert!(bindings_in(g).next().is_some(), "no bindings in {g:?}");
        }
    }

    /// The operator screens (Schedulers / Workers / Events) advertise their
    /// high-value-but-easy-to-miss actions on screen: Events its drill-in to the
    /// job behind an event, and Schedulers/Workers the global jump along the
    /// "scheduled → running now → just happened" operator chain. Guards against a
    /// future edit silently dropping these from the status line.
    #[test]
    fn operator_screens_surface_their_key_shortcuts() {
        let surfaces = |screen: Screen, binding: &str| {
            status_hints(screen).iter().any(|h| h.binding == binding)
        };
        // Events: the event's job is openable — say so.
        assert!(
            surfaces(Screen::Events, "Enter / →"),
            "Events status line must advertise opening the event's job"
        );
        // Schedulers → busy workers; Workers → live events feed.
        assert!(
            surfaces(Screen::Schedulers, "w"),
            "Schedulers status line must advertise the jump to busy workers"
        );
        assert!(
            surfaces(Screen::Workers, "E"),
            "Workers status line must advertise the jump to the events feed"
        );
    }
}
