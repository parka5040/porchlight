use porchlight_core::access::{self, AccessContext, CallerKind, ListCapability, Role, Visibility};

#[test]
fn standalone_owner_viewer_admin_and_agent_rules() {
    for visibility in [
        Visibility::Household,
        Visibility::Private,
        Visibility::Selected,
    ] {
        for role in [Role::User, Role::SuperAdmin, Role::Dependent] {
            for owner in [false, true] {
                for viewer in [false, true] {
                    let ctx = access::standalone_list_context(
                        CallerKind::AppMember(role),
                        visibility,
                        owner,
                        viewer,
                    );
                    let visible = role != Role::Dependent
                        && (owner
                            || visibility == Visibility::Household
                            || (visibility == Visibility::Selected && viewer));
                    assert_eq!(
                        access::standalone_list_can(&ctx, ListCapability::View),
                        visible
                    );
                    for cap in [
                        ListCapability::Check,
                        ListCapability::Manage,
                        ListCapability::Edit,
                        ListCapability::Reassign,
                        ListCapability::Delete,
                        ListCapability::ChangeVisibility,
                    ] {
                        assert_eq!(
                            access::standalone_list_can(&ctx, cap),
                            visible
                                && (matches!(cap, ListCapability::Check | ListCapability::Manage)
                                    || owner
                                    || role == Role::SuperAdmin)
                        );
                    }
                }
            }
        }
        let ctx = access::standalone_list_context(CallerKind::Agent, visibility, false, false);
        for cap in ListCapability::ALL {
            assert!(access::standalone_list_can(&ctx, cap));
        }
    }
}

#[test]
fn attached_assignee_can_check_but_viewer_cannot_and_subject_never_grants_access() {
    let mut ctx = AccessContext {
        visibility: Visibility::Selected,
        is_assignee: true,
        ..Default::default()
    };
    assert!(access::list_can(&ctx, ListCapability::Check));
    assert!(!access::list_can(&ctx, ListCapability::Manage));
    ctx.is_assignee = false;
    ctx.is_viewer = true;
    assert!(access::list_can(&ctx, ListCapability::View));
    assert!(!access::list_can(&ctx, ListCapability::Check));
    ctx.is_viewer = false;
    ctx.is_subject = true;
    assert!(!access::list_can(&ctx, ListCapability::View));
}
