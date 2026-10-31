//! False-green guard for the StickDoctor breadcrumbs.
//!
//! This test keeps the annotations honest. It fails when a breadcrumb is
//! missing, a cross-reference dangles, a flow is out of order, an invariant has
//! no guard test, a verification function can succeed on an unknown input, or a
//! test contains no assertion. It also fails when the committed
//! `docs/AGENT_MAP.md` no longer matches the code.

use stickdoctor_agent::{render_markdown, scan_workspace, validate, workspace_root};

#[test]
fn annotations_are_well_formed_and_consistent() {
    let map = scan_workspace().expect("scan the workspace");
    let errors = validate(&map);
    assert!(
        errors.is_empty(),
        "annotation guard failed ({} error(s)):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

#[test]
fn committed_map_is_up_to_date() {
    let map = scan_workspace().expect("scan the workspace");
    let rendered = render_markdown(&map);
    let path = workspace_root()
        .expect("workspace root")
        .join("docs/AGENT_MAP.md");
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    assert_eq!(
        committed.trim(),
        rendered.trim(),
        "docs/AGENT_MAP.md is stale — run `cargo run -p stickdoctor-cli -- agent-map --write`"
    );
}
