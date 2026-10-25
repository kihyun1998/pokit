//! How a test tells CI, where nobody uses the machine, from a machine in use.

mod common;

use common::*;

#[test]
fn only_the_value_github_actions_sets_marks_a_run_as_ci() {
    assert!(is_ci(Some("true")));
    for other in [None, Some(""), Some("false"), Some("1"), Some("TRUE")] {
        assert!(!is_ci(other), "{other:?}");
    }
}

#[test]
fn a_quiet_machine_test_runs_on_ci_and_is_skipped_elsewhere() {
    assert_eq!(quiet_machine("this test"), on_ci());
}
