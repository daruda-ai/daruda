use super::*;

const ROOMY: GroupFacts = GroupFacts {
    keyboard_aside: false,
    beside_setting: true,
    tabs: 2,
    work_shown: true,
    fits_both: true,
};

#[test]
fn what_goes_beside_the_tabs() {
    use PaneEntry::{Enter, Preview};
    let narrow = GroupFacts {
        fits_both: false,
        ..ROOMY
    };
    let cases = [
        ("the default", ROOMY, Enter, true),
        ("a skim, with room for both", ROOMY, Preview, true),
        (
            "the setting off",
            GroupFacts {
                beside_setting: false,
                ..ROOMY
            },
            Enter,
            false,
        ),
        (
            "no tab to be beside",
            GroupFacts {
                tabs: 0,
                work_shown: false,
                ..ROOMY
            },
            Enter,
            false,
        ),
        (
            "files alone shown: it joins them",
            GroupFacts {
                work_shown: false,
                ..ROOMY
            },
            Enter,
            false,
        ),
        ("a skim where one group fits", narrow, Preview, false),
        ("entered where one group fits", narrow, Enter, true),
        (
            "the keyboard aside, whatever else",
            GroupFacts {
                keyboard_aside: true,
                beside_setting: false,
                tabs: 0,
                work_shown: false,
                fits_both: false,
            },
            Preview,
            true,
        ),
    ];
    for (label, facts, entry, beside) in cases {
        assert_eq!(facts.goes_beside(entry), beside, "{label}");
    }
    // What only the side tabs hold needs a tab to be beside, of any kind.
    let files_shown = GroupFacts {
        work_shown: false,
        ..ROOMY
    };
    assert!(files_shown.side_only_goes_beside(Enter));
    assert!(
        !GroupFacts {
            tabs: 0,
            ..files_shown
        }
        .side_only_goes_beside(Enter)
    );
}

#[test]
fn an_open_shows_refills_or_makes_a_tab() {
    use OpenIntent::{Commit, Enter, Preview};
    let (left, aside) = (
        (PanePlace::Tab(1), PaneId(11)),
        (PanePlace::Side(0), PaneId(22)),
    );
    let facts = |existing, scratch, groups| OpenFacts {
        groups,
        existing,
        scratch,
    };
    let among_tabs = GroupFacts {
        beside_setting: false,
        ..ROOMY
    };
    let new = |beside| OpenPlan::New { beside };
    let keyboard_aside = GroupFacts {
        keyboard_aside: true,
        beside_setting: false,
        ..ROOMY
    };
    let cases = [
        (
            "already open: shown, whatever the rest",
            Preview,
            facts(Some(left), Some(aside), ROOMY),
            OpenPlan::Show(left),
        ),
        (
            "nothing to reuse",
            Enter,
            facts(None, None, ROOMY),
            new(true),
        ),
        (
            "the scratch tab is where it goes",
            Preview,
            facts(None, Some(aside), ROOMY),
            OpenPlan::Refill(aside),
        ),
        (
            "a kept open still refills the scratch tab there",
            Enter,
            facts(None, Some(aside), ROOMY),
            OpenPlan::Refill(aside),
        ),
        (
            "a skim takes its scratch tab to the other group",
            Preview,
            facts(None, Some(left), ROOMY),
            OpenPlan::Replace(left),
        ),
        (
            "and back among the tabs",
            Preview,
            facts(None, Some(aside), among_tabs),
            OpenPlan::Replace(aside),
        ),
        (
            "a file opened to keep leaves the preview alone",
            Commit,
            facts(None, Some(left), ROOMY),
            new(true),
        ),
        (
            // Where the new tab goes is not decided here, but once the
            // scratch tab is closed.
            "the keyboard aside, the scratch tab among the tabs",
            Preview,
            facts(None, Some(left), keyboard_aside),
            OpenPlan::Replace(left),
        ),
        (
            "kept with the keyboard aside: beside, the preview left alone",
            Commit,
            facts(None, Some(left), keyboard_aside),
            new(true),
        ),
    ];
    for (label, intent, facts, plan) in cases {
        assert_eq!(plan_open(intent, &facts), plan, "{label}");
    }
}
