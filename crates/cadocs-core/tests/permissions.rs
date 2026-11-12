//! Every cell of `docs/PERMISSIONS.md`. The expected tables are copied from the document. Their
//! exhaustive matches stop a new action from compiling until its row is written here, but the
//! `*_ACTIONS` lists are by hand: add the action there too, or its row is never checked.

use cadocs_core::permissions::*;

const SPACE_ROLES: [SpaceRole; 4] = [
    SpaceRole::Viewer,
    SpaceRole::Commenter,
    SpaceRole::Editor,
    SpaceRole::Admin,
];
const WORKSPACE_ROLES: [WorkspaceRole; 3] = [
    WorkspaceRole::Member,
    WorkspaceRole::Admin,
    WorkspaceRole::Owner,
];
const INSTANCE_ROLES: [InstanceRole; 2] = [InstanceRole::User, InstanceRole::Admin];

const Y: bool = true;
const N: bool = false;

/// Columns: `viewer`, `commenter`, `editor`, `admin`.
fn space_row(action: SpaceAction) -> [bool; 4] {
    use SpaceAction::*;
    match action {
        ReadSpace | ReadPage | ReadComments | OpenSocketReadOnly => [Y, Y, Y, Y],
        WriteComment | EditOwnComment => [N, Y, Y, Y],
        EditOtherComment => [N, N, N, Y],
        OpenSocketReadWrite | EditPage | RestorePage | ListTrash | RestoreRevision
        | ManageAttachments | ManageShares => [N, N, Y, Y],
        PurgePage | ManageSpace | TrashSpace | RestoreSpace => [N, N, N, Y],
    }
}

const SPACE_ACTIONS: [SpaceAction; 18] = {
    use SpaceAction::*;
    [
        ReadSpace,
        ReadPage,
        ReadComments,
        OpenSocketReadOnly,
        WriteComment,
        EditOwnComment,
        EditOtherComment,
        OpenSocketReadWrite,
        EditPage,
        RestorePage,
        ListTrash,
        RestoreRevision,
        ManageAttachments,
        ManageShares,
        PurgePage,
        ManageSpace,
        TrashSpace,
        RestoreSpace,
    ]
};

/// Columns: `member`, `admin`, `owner`.
fn workspace_row(action: WorkspaceAction) -> [bool; 3] {
    use WorkspaceAction::*;
    match action {
        Read => [Y, Y, Y],
        Leave { last_owner: false } => [Y, Y, Y],
        Leave { last_owner: true } => [Y, Y, N],
        Update | CreateSpace | ManageInvitations | ManageMember | ManageGroups => [N, Y, Y],
        ManageOwner | Delete => [N, N, Y],
    }
}

const WORKSPACE_ACTIONS: [WorkspaceAction; 10] = {
    use WorkspaceAction::*;
    [
        Read,
        Leave { last_owner: false },
        Leave { last_owner: true },
        Update,
        CreateSpace,
        ManageInvitations,
        ManageMember,
        ManageGroups,
        ManageOwner,
        Delete,
    ]
};

/// Columns: user, instance admin.
fn instance_row(action: InstanceAction) -> [bool; 2] {
    use InstanceAction::*;
    match action {
        ManageOwnAccount => [Y, Y],
        DeleteOwnAccount { last_owner: false } => [Y, Y],
        DeleteOwnAccount { last_owner: true } => [N, N],
        CreateWorkspace | ManageUsers | GenerateResetLink => [N, Y],
        DeleteUser { last_owner: false } => [N, Y],
        DeleteUser { last_owner: true } => [N, N],
    }
}

const INSTANCE_ACTIONS: [InstanceAction; 8] = {
    use InstanceAction::*;
    [
        ManageOwnAccount,
        DeleteOwnAccount { last_owner: false },
        DeleteOwnAccount { last_owner: true },
        CreateWorkspace,
        ManageUsers,
        DeleteUser { last_owner: false },
        DeleteUser { last_owner: true },
        GenerateResetLink,
    ]
};

/// The state in which an action applies: restoring needs something in the trash.
fn scope_for(role: Option<SpaceRole>, action: SpaceAction) -> Scope {
    Scope {
        role,
        space_trashed: action == SpaceAction::RestoreSpace,
        page_trashed: matches!(action, SpaceAction::RestorePage | SpaceAction::PurgePage),
    }
}

fn member(grants: &[SpaceRole], default_role: Option<DefaultRole>) -> SpaceAccess<'_> {
    SpaceAccess {
        account_active: true,
        workspace_role: Some(WorkspaceRole::Member),
        grants,
        default_role,
    }
}

#[test]
fn space_matrix() {
    for action in SPACE_ACTIONS {
        for (role, expected) in SPACE_ROLES.into_iter().zip(space_row(action)) {
            assert_eq!(
                scope_for(Some(role), action).allows(action),
                expected,
                "{role:?} {action:?}"
            );
        }
    }
}

#[test]
fn workspace_matrix() {
    for action in WORKSPACE_ACTIONS {
        for (role, expected) in WORKSPACE_ROLES.into_iter().zip(workspace_row(action)) {
            assert_eq!(
                workspace_allows(Some(role), action),
                expected,
                "{role:?} {action:?}"
            );
        }
    }
}

#[test]
fn instance_matrix() {
    for action in INSTANCE_ACTIONS {
        for (role, expected) in INSTANCE_ROLES.into_iter().zip(instance_row(action)) {
            assert_eq!(
                instance_allows(Some(role), action),
                expected,
                "{role:?} {action:?}"
            );
        }
    }
}

#[test]
fn no_role_denies_everything() {
    for action in SPACE_ACTIONS {
        assert!(!scope_for(None, action).allows(action), "{action:?}");
    }
    for action in WORKSPACE_ACTIONS {
        assert!(!workspace_allows(None, action), "{action:?}");
    }
    for action in INSTANCE_ACTIONS {
        assert!(!instance_allows(None, action), "{action:?}");
    }
}

#[test]
fn inactive_account_has_no_role() {
    assert_eq!(instance_role(false, true), None);
    assert_eq!(workspace_role(false, Some(WorkspaceRole::Owner)), None);
    let access = SpaceAccess {
        account_active: false,
        workspace_role: Some(WorkspaceRole::Owner),
        grants: &[SpaceRole::Admin],
        default_role: Some(DefaultRole::Editor),
    };
    assert_eq!(space_role(&access), None);
}

#[test]
fn active_account_keeps_its_roles() {
    assert_eq!(instance_role(true, false), Some(InstanceRole::User));
    assert_eq!(instance_role(true, true), Some(InstanceRole::Admin));
    assert_eq!(
        workspace_role(true, Some(WorkspaceRole::Admin)),
        Some(WorkspaceRole::Admin)
    );
    assert_eq!(workspace_role(true, None), None);
}

#[test]
fn non_member_has_no_access_whatever_the_grants() {
    let access = SpaceAccess {
        account_active: true,
        workspace_role: None,
        grants: &[SpaceRole::Admin],
        default_role: Some(DefaultRole::Editor),
    };
    assert_eq!(space_role(&access), None);
}

#[test]
fn workspace_owner_and_admin_are_admin_of_every_space() {
    for role in [WorkspaceRole::Owner, WorkspaceRole::Admin] {
        let access = SpaceAccess {
            account_active: true,
            workspace_role: Some(role),
            grants: &[SpaceRole::Viewer],
            default_role: None,
        };
        assert_eq!(space_role(&access), Some(SpaceRole::Admin));
    }
}

#[test]
fn highest_grant_wins() {
    let grants = [SpaceRole::Viewer, SpaceRole::Editor, SpaceRole::Commenter];
    assert_eq!(space_role(&member(&grants, None)), Some(SpaceRole::Editor));
}

#[test]
fn a_grant_replaces_a_higher_default_role() {
    let access = member(&[SpaceRole::Viewer], Some(DefaultRole::Editor));
    assert_eq!(space_role(&access), Some(SpaceRole::Viewer));
}

#[test]
fn default_role_applies_without_a_grant() {
    for (default_role, expected) in [
        (DefaultRole::Viewer, SpaceRole::Viewer),
        (DefaultRole::Commenter, SpaceRole::Commenter),
        (DefaultRole::Editor, SpaceRole::Editor),
    ] {
        assert_eq!(space_role(&member(&[], Some(default_role))), Some(expected));
    }
}

#[test]
fn no_grant_and_no_default_role_means_no_access() {
    assert_eq!(space_role(&member(&[], None)), None);
}

#[test]
fn trashed_space_denies_everything_but_its_restoration() {
    for role in SPACE_ROLES {
        for action in SPACE_ACTIONS {
            for page_trashed in [false, true] {
                let scope = Scope {
                    role: Some(role),
                    space_trashed: true,
                    page_trashed,
                };
                let expected = action == SpaceAction::RestoreSpace && role == SpaceRole::Admin;
                assert_eq!(scope.allows(action), expected, "{role:?} {action:?}");
            }
        }
    }
}

#[test]
fn live_space_cannot_be_restored() {
    assert!(!Scope::live(Some(SpaceRole::Admin)).allows(SpaceAction::RestoreSpace));
}

#[test]
fn trashed_page_is_absent_except_for_trash_and_space_actions() {
    use SpaceAction::*;
    for action in SPACE_ACTIONS {
        let scope = Scope {
            role: Some(SpaceRole::Admin),
            space_trashed: false,
            page_trashed: true,
        };
        let expected = matches!(
            action,
            ListTrash | RestorePage | PurgePage | ReadSpace | ManageSpace | TrashSpace
        );
        assert_eq!(scope.allows(action), expected, "{action:?}");
    }
}

#[test]
fn live_page_cannot_be_restored_or_purged() {
    let scope = Scope::live(Some(SpaceRole::Admin));
    assert!(!scope.allows(SpaceAction::RestorePage));
    assert!(!scope.allows(SpaceAction::PurgePage));
}

#[test]
fn move_needs_editor_on_both_spaces_of_one_workspace() {
    let editor = Scope::live(Some(SpaceRole::Editor));
    let commenter = Scope::live(Some(SpaceRole::Commenter));
    let absent = Scope::live(None);
    let trashed = Scope {
        space_trashed: true,
        ..editor
    };

    assert!(move_allowed(editor, editor, true));
    assert!(!move_allowed(editor, commenter, true));
    assert!(!move_allowed(commenter, editor, true));
    assert!(!move_allowed(editor, absent, true));
    assert!(!move_allowed(editor, trashed, true));
    assert!(!move_allowed(editor, editor, false));
}

#[test]
fn download_needs_one_readable_referencing_page() {
    let readable = Scope::live(Some(SpaceRole::Viewer));
    let hidden = Scope::live(None);
    let trashed = Scope {
        page_trashed: true,
        ..readable
    };

    assert!(download_allowed([hidden, trashed, readable]));
    assert!(!download_allowed([hidden, trashed]));
    assert!(!download_allowed([]));
}

#[test]
fn delete_needs_an_editor_of_every_referencing_page() {
    let editable = Scope::live(Some(SpaceRole::Editor));
    let readable = Scope::live(Some(SpaceRole::Viewer));
    let hidden = Scope::live(None);
    let trashed = Scope {
        page_trashed: true,
        ..editable
    };
    let trashed_space = Scope {
        space_trashed: true,
        ..editable
    };

    assert!(delete_allowed([editable]));
    assert!(delete_allowed([editable, trashed, trashed_space]));
    assert!(delete_allowed([Scope::live(Some(SpaceRole::Admin)), editable]));
    assert!(!delete_allowed([editable, readable]));
    assert!(!delete_allowed([editable, hidden]));
    assert!(!delete_allowed([]));
}

#[test]
fn owner_rows_are_needed_to_touch_an_owner() {
    use WorkspaceRole::*;
    let manage_member = WorkspaceAction::ManageMember;
    let manage_owner = WorkspaceAction::ManageOwner;

    assert_eq!(member_change_action(Member, Some(Admin)), manage_member);
    assert_eq!(member_change_action(Admin, Some(Member)), manage_member);
    assert_eq!(member_change_action(Admin, None), manage_member);
    assert_eq!(member_change_action(Member, Some(Owner)), manage_owner);
    assert_eq!(member_change_action(Owner, Some(Admin)), manage_owner);
    assert_eq!(member_change_action(Owner, None), manage_owner);
}

#[test]
fn a_workspace_keeps_one_owner() {
    use WorkspaceRole::*;
    assert!(would_remove_last_owner(Owner, None, 1));
    assert!(would_remove_last_owner(Owner, Some(Admin), 1));
    assert!(!would_remove_last_owner(Owner, None, 2));
    assert!(!would_remove_last_owner(Owner, Some(Owner), 1));
    assert!(!would_remove_last_owner(Admin, None, 1));
}

#[test]
fn an_instance_keeps_one_enabled_admin() {
    assert!(would_remove_last_instance_admin(true, 1));
    assert!(!would_remove_last_instance_admin(true, 2));
    assert!(!would_remove_last_instance_admin(false, 1));
}

#[test]
fn socket_mode_follows_the_role() {
    use SocketMode::*;
    let mode = |role| page_socket_mode(Scope::live(role));

    assert_eq!(mode(None), None);
    assert_eq!(mode(Some(SpaceRole::Viewer)), Some(ReadOnly));
    assert_eq!(mode(Some(SpaceRole::Commenter)), Some(ReadOnly));
    assert_eq!(mode(Some(SpaceRole::Editor)), Some(ReadWrite));
    assert_eq!(mode(Some(SpaceRole::Admin)), Some(ReadWrite));
}

#[test]
fn recompute_keeps_or_closes_the_socket() {
    use SocketMode::*;
    use SocketOutcome::*;
    let editor = Scope::live(Some(SpaceRole::Editor));
    let viewer = Scope::live(Some(SpaceRole::Viewer));

    assert_eq!(recompute_page_socket(ReadWrite, editor), Keep);
    assert_eq!(recompute_page_socket(ReadOnly, viewer), Keep);
    assert_eq!(
        recompute_page_socket(ReadWrite, viewer),
        Close(CLOSE_MODE_CHANGED)
    );
    assert_eq!(
        recompute_page_socket(ReadOnly, editor),
        Close(CLOSE_MODE_CHANGED)
    );
    assert_eq!(
        recompute_page_socket(ReadOnly, Scope::live(None)),
        Close(CLOSE_FORBIDDEN)
    );
}

#[test]
fn recompute_closes_sockets_on_trashed_content() {
    let editor = Scope::live(Some(SpaceRole::Editor));
    for scope in [
        Scope {
            page_trashed: true,
            ..editor
        },
        Scope {
            space_trashed: true,
            ..editor
        },
    ] {
        assert_eq!(
            recompute_page_socket(SocketMode::ReadWrite, scope),
            SocketOutcome::Close(CLOSE_NOT_FOUND)
        );
    }
}

#[test]
fn recompute_does_not_reveal_the_trash_to_a_revoked_user() {
    for scope in [
        Scope {
            role: None,
            space_trashed: false,
            page_trashed: true,
        },
        Scope {
            role: None,
            space_trashed: true,
            page_trashed: false,
        },
    ] {
        assert_eq!(
            recompute_page_socket(SocketMode::ReadOnly, scope),
            SocketOutcome::Close(CLOSE_FORBIDDEN)
        );
    }
}

#[test]
fn a_workspace_role_reads_back_what_it_writes() {
    for role in [
        WorkspaceRole::Member,
        WorkspaceRole::Admin,
        WorkspaceRole::Owner,
    ] {
        assert_eq!(WorkspaceRole::parse(role.as_str()), Some(role));
    }
    assert_eq!(WorkspaceRole::parse("Owner"), None);
}

#[test]
fn space_roles_read_back_what_they_write() {
    use SpaceRole::*;
    for role in [Viewer, Commenter, Editor, Admin] {
        assert_eq!(SpaceRole::parse(role.as_str()), Some(role));
    }
    assert_eq!(SpaceRole::parse("owner"), None);
}

#[test]
fn a_default_role_is_never_admin() {
    use DefaultRole::*;
    for role in [Viewer, Commenter, Editor] {
        assert_eq!(DefaultRole::parse(role.as_str()), Some(role));
    }
    assert_eq!(DefaultRole::parse("admin"), None);
}
