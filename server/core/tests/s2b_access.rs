use porchlight_core::access::*;
use serde_json::json;

#[path = "../examples/permissions-matrix/document_check.rs"]
mod document_check;

#[test]
fn matrix_read_errors_are_not_reported_as_stale() {
    let error = document_check::check_document(std::path::Path::new(""), "expected")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("could not read permissions matrix"),
        "{error}"
    );
    assert!(error.contains("No such file or directory"), "{error}");
    assert!(!error.contains("stale"), "{error}");
}

#[test]
fn stale_matrix_reports_only_the_first_differing_line_and_regeneration_command() {
    let error = document_check::check_contents("header\nold\n", "header\nnew\n").unwrap_err();
    assert!(error.contains("stale"), "{error}");
    assert!(error.contains("first differing line: 2"), "{error}");
    assert!(error.contains("--example permissions-matrix"), "{error}");
    assert!(!error.contains("old") && !error.contains("new"), "{error}");
    let huge = "x".repeat(40_000);
    let error = document_check::check_contents(&huge, "").unwrap_err();
    assert!(error.len() < 500, "diagnostic must not dump the documents");
}

#[test]
fn matrix_comparison_detects_length_and_trailing_newline_differences() {
    assert!(document_check::check_contents("header\n", "header\n").is_ok());
    for (actual, expected) in [
        ("header\n", "header"),
        ("header", "header\n"),
        ("header\n", "header\nrow\n"),
    ] {
        let error = document_check::check_contents(actual, expected).unwrap_err();
        assert!(error.contains("first differing line: 2"), "{error}");
    }
}

fn spec_oracle(ctx: &AccessContext) -> Vec<Capability> {
    use Capability::*;
    if ctx.caller == CallerKind::Agent {
        return Capability::ALL.to_vec();
    }
    let CallerKind::AppMember(role) = ctx.caller else {
        unreachable!()
    };
    if role == Role::Dependent {
        return vec![];
    }
    let visible = match ctx.visibility {
        Visibility::Household => true,
        Visibility::Private => ctx.is_owner || ctx.is_assignee,
        Visibility::Selected => ctx.is_owner || ctx.is_assignee || ctx.is_viewer,
    };
    if !visible {
        return vec![];
    }
    let mut expected = vec![View, Comment];
    if ctx.is_owner || ctx.is_assignee || role == Role::SuperAdmin {
        expected.extend([ChangeStatus, Complete, Snooze]);
    }
    if ctx.is_owner || role == Role::SuperAdmin {
        expected.extend([Edit, EditReminders, Reassign, ChangeVisibility]);
    }
    if ctx.unassigned_dependent_item {
        expected.push(Take);
    }
    if ctx.is_owner || role == Role::SuperAdmin {
        expected.push(Delete);
    }
    expected
}

#[test]
fn every_matrix_row_matches_the_spec_independently() {
    let matrix = permission_matrix();
    assert_eq!(matrix.len(), 4 * 3 * 32);
    let mut contexts = std::collections::HashSet::new();
    for row in matrix {
        assert!(contexts.insert(row.context), "duplicate matrix context");
        let expected = spec_oracle(&row.context);
        assert_eq!(
            row.capabilities.iter().collect::<Vec<_>>(),
            expected,
            "{:?}",
            row.context
        );
        assert_eq!(can_view(&row.context), expected.contains(&Capability::View));
        assert_eq!(capabilities(&row.context), row.capabilities);
    }
}

#[test]
fn default_context_fails_closed() {
    let ctx = AccessContext::default();
    assert!(!can_view(&ctx));
    assert!(capabilities(&ctx).is_empty());
    assert_eq!(ctx.visibility, Visibility::Private);
    assert!(!ctx.is_owner && !ctx.is_assignee && !ctx.is_viewer && !ctx.is_subject);
    assert!(!ctx.unassigned_dependent_item);
    let admin = AccessContext {
        caller: CallerKind::AppMember(Role::SuperAdmin),
        ..ctx
    };
    assert!(!can_view(&admin));
    assert!(capabilities(&admin).is_empty());
}

#[test]
fn subjects_and_app_admins_never_bypass_visibility() {
    let ctx = AccessContext {
        caller: CallerKind::AppMember(Role::SuperAdmin),
        visibility: Visibility::Private,
        is_subject: true,
        unassigned_dependent_item: true,
        ..AccessContext::default()
    };
    assert!(!can_view(&ctx));
    assert!(capabilities(&ctx).is_empty());
    let agent = AccessContext {
        caller: CallerKind::Agent,
        ..ctx
    };
    assert!(can_view(&agent));
    assert_eq!(
        capabilities(&agent).iter().collect::<Vec<_>>(),
        Capability::ALL
    );
}

#[test]
fn serialized_capabilities_use_the_contract_order_and_strings() {
    let full = capabilities(&AccessContext {
        caller: CallerKind::Agent,
        ..AccessContext::default()
    });
    assert_eq!(
        serde_json::to_value(full).unwrap(),
        json!([
            "view",
            "comment",
            "change_status",
            "complete",
            "snooze",
            "edit",
            "edit_reminders",
            "reassign",
            "change_visibility",
            "take",
            "delete"
        ])
    );
    let assignee = capabilities(&AccessContext {
        is_assignee: true,
        ..AccessContext::default()
    });
    assert_eq!(
        serde_json::to_value(assignee).unwrap(),
        json!(["view", "comment", "change_status", "complete", "snooze"])
    );
    assert!(assignee.contains(Capability::Snooze));
    assert!(!assignee.contains(Capability::Edit));
}

#[test]
fn db_strings_are_exact_and_unknown_values_fail_closed() {
    for (s, role) in [
        ("super_admin", Role::SuperAdmin),
        ("user", Role::User),
        ("dependent", Role::Dependent),
    ] {
        assert_eq!(s.parse::<Role>().unwrap(), role);
        assert_eq!(role.as_str(), s);
        assert_eq!(serde_json::to_value(role).unwrap(), json!(s));
    }
    for (s, visibility) in [
        ("household", Visibility::Household),
        ("private", Visibility::Private),
        ("selected", Visibility::Selected),
    ] {
        assert_eq!(s.parse::<Visibility>().unwrap(), visibility);
        assert_eq!(visibility.as_str(), s);
        assert_eq!(serde_json::to_value(visibility).unwrap(), json!(s));
    }
    assert!("admin".parse::<Role>().is_err());
    assert!("public".parse::<Visibility>().is_err());
}

#[test]
fn permissions_document_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/specs/permissions-matrix.md");
    if let Err(error) = document_check::check_document(&path, &permissions_markdown()) {
        panic!("{error}");
    }
}
