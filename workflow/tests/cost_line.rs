//! The cost line is the one record of what a session spent, and the hub,
//! status and serve all read it back off mem's run log, so what it writes
//! must parse back whole, with or without the listing's prefix.

use workflow::cost::CostLine;

fn bare(slug: &str, kind: &str, name: &str) -> CostLine {
    CostLine {
        slug: slug.into(),
        kind: kind.into(),
        name: name.into(),
        minutes: None,
        context: None,
        input: None,
        output: None,
        model: None,
        role: None,
        sessions: None,
    }
}

#[test]
fn a_line_with_every_field_parses_back_to_itself() {
    let full = CostLine {
        minutes: Some(42),
        context: Some(118000),
        input: Some(5200000),
        output: Some(61000),
        sessions: Some(1),
        model: Some("opus".into()),
        role: Some("lead".into()),
        ..bare("m71-measure", "worker", "cost-line")
    };
    let line = full.line();
    assert_eq!(
        line,
        "cost m71-measure worker cost-line: minutes=42 context=118000 in=5200000 out=61000 sessions=1 model=opus role=lead"
    );
    assert_eq!(CostLine::parse(&line), Some(full));
}

#[test]
fn a_line_with_no_field_parses_back_to_itself() {
    let none = bare("m71-measure", "walk", "m71-measure");
    assert_eq!(CostLine::parse(&none.line()), Some(none));
}

#[test]
fn the_sum_of_two_lines_counts_two_sessions() {
    let a = CostLine {
        minutes: Some(10),
        context: Some(100),
        input: Some(1000),
        output: Some(50),
        model: Some("opus".into()),
        ..bare("m71-measure", "worker", "cost-line")
    };
    let b = CostLine {
        minutes: Some(5),
        context: Some(200),
        role: Some("lead".into()),
        ..bare("m71-measure", "lead", "pickup")
    };
    let sum = CostLine::sum("m71-measure", &[a, b]);
    assert_eq!(
        sum.line(),
        "cost m71-measure milestone m71-measure: minutes=15 context=300 in=1000 out=50 sessions=2"
    );
}

#[test]
fn a_listing_line_with_the_date_and_id_parses() {
    let row = "2026-10-07  #A6DJWPSN  cost m71-measure worker cost-line: minutes=12 in=900 color=blue model=sonnet";
    assert_eq!(
        CostLine::parse(row),
        Some(CostLine {
            minutes: Some(12),
            input: Some(900),
            model: Some("sonnet".into()),
            ..bare("m71-measure", "worker", "cost-line")
        })
    );
}

#[test]
fn a_run_line_that_is_not_a_cost_is_none() {
    assert_eq!(
        CostLine::parse("2026-10-07  #218986HV  run m71-measure: dispatched usage"),
        None
    );
}
