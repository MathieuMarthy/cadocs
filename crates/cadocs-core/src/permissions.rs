//! The access rules of `docs/PERMISSIONS.md` as pure functions. Anything not listed is denied.

/// Sent when a recompute leaves the page unreadable.
pub const CLOSE_FORBIDDEN: u16 = 4403;
/// Sent when the page or its space is in the trash.
pub const CLOSE_NOT_FOUND: u16 = 4404;
/// Sent when the page stays readable in another mode. The client reconnects.
pub const CLOSE_MODE_CHANGED: u16 = 4409;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceRole {
    User,
    Admin,
}

/// Ordered from least to most access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkspaceRole {
    Member,
    Admin,
    Owner,
}

/// Ordered from least to most access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpaceRole {
    Viewer,
    Commenter,
    Editor,
    Admin,
}

/// The roles a space may give by default: `admin` always needs a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultRole {
    Viewer,
    Commenter,
    Editor,
}

impl From<DefaultRole> for SpaceRole {
    fn from(role: DefaultRole) -> Self {
        match role {
            DefaultRole::Viewer => SpaceRole::Viewer,
            DefaultRole::Commenter => SpaceRole::Commenter,
            DefaultRole::Editor => SpaceRole::Editor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceAction {
    /// Read or update one's own account.
    ManageOwnAccount,
    DeleteOwnAccount {
        last_owner: bool,
    },
    CreateWorkspace,
    /// List or disable users, grant instance admin.
    ManageUsers,
    DeleteUser {
        last_owner: bool,
    },
    GenerateResetLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Read the workspace, list members, search.
    Read,
    Leave {
        last_owner: bool,
    },
    Update,
    CreateSpace,
    /// Create or revoke an invitation.
    ManageInvitations,
    /// Change or remove a `member` or `admin`.
    ManageMember,
    ManageGroups,
    /// Change or remove an `owner`, or grant `owner`.
    ManageOwner,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceAction {
    /// Read the space, tree and grants.
    ReadSpace,
    /// Read a page, export it, read its revisions, download attachments.
    ReadPage,
    ReadComments,
    OpenSocketReadOnly,
    /// Create a comment or reply, resolve or reopen.
    WriteComment,
    EditOwnComment,
    EditOtherComment,
    OpenSocketReadWrite,
    /// Create, update, move or trash a page.
    EditPage,
    RestorePage,
    ListTrash,
    /// Restore a revision, import.
    RestoreRevision,
    /// Upload or delete an attachment.
    ManageAttachments,
    /// List, create or revoke public shares.
    ManageShares,
    PurgePage,
    /// Update the space, edit grants.
    ManageSpace,
    TrashSpace,
    RestoreSpace,
}

/// What resolution needs to know about one user and one space.
#[derive(Debug, Clone, Copy)]
pub struct SpaceAccess<'a> {
    /// False for a disabled user or a session that is not valid.
    pub account_active: bool,
    /// `None` for a non-member of the workspace.
    pub workspace_role: Option<WorkspaceRole>,
    /// Direct and group grants of the user on this space.
    pub grants: &'a [SpaceRole],
    pub default_role: Option<DefaultRole>,
}

/// A role on a space, and the trash state of the space and the page acted on.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scope {
    pub role: Option<SpaceRole>,
    pub space_trashed: bool,
    pub page_trashed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketOutcome {
    Keep,
    Close(u16),
}

pub fn instance_role(account_active: bool, is_instance_admin: bool) -> Option<InstanceRole> {
    match (account_active, is_instance_admin) {
        (false, _) => None,
        (true, false) => Some(InstanceRole::User),
        (true, true) => Some(InstanceRole::Admin),
    }
}

pub fn workspace_role(
    account_active: bool,
    membership: Option<WorkspaceRole>,
) -> Option<WorkspaceRole> {
    membership.filter(|_| account_active)
}

/// The resolution of `docs/PERMISSIONS.md`: the first rule that applies decides.
pub fn space_role(access: &SpaceAccess<'_>) -> Option<SpaceRole> {
    match workspace_role(access.account_active, access.workspace_role)? {
        WorkspaceRole::Owner | WorkspaceRole::Admin => Some(SpaceRole::Admin),
        WorkspaceRole::Member => {
            let granted = access.grants.iter().copied().max();
            granted.or(access.default_role.map(SpaceRole::from))
        }
    }
}

pub fn instance_allows(role: Option<InstanceRole>, action: InstanceAction) -> bool {
    use InstanceAction::*;
    let Some(role) = role else { return false };
    match action {
        ManageOwnAccount => true,
        DeleteOwnAccount { last_owner } => !last_owner,
        CreateWorkspace | ManageUsers | GenerateResetLink => role == InstanceRole::Admin,
        DeleteUser { last_owner } => role == InstanceRole::Admin && !last_owner,
    }
}

pub fn workspace_allows(role: Option<WorkspaceRole>, action: WorkspaceAction) -> bool {
    use WorkspaceAction::*;
    let Some(role) = role else { return false };
    match action {
        Read => true,
        Leave { last_owner } => !(last_owner && role == WorkspaceRole::Owner),
        Update | CreateSpace | ManageInvitations | ManageMember | ManageGroups => {
            role >= WorkspaceRole::Admin
        }
        ManageOwner | Delete => role == WorkspaceRole::Owner,
    }
}

impl SpaceAction {
    fn required_role(self) -> SpaceRole {
        use SpaceAction::*;
        match self {
            ReadSpace | ReadPage | ReadComments | OpenSocketReadOnly => SpaceRole::Viewer,
            WriteComment | EditOwnComment => SpaceRole::Commenter,
            OpenSocketReadWrite | EditPage | RestorePage | ListTrash | RestoreRevision
            | ManageAttachments | ManageShares => SpaceRole::Editor,
            EditOtherComment | PurgePage | ManageSpace | TrashSpace | RestoreSpace => {
                SpaceRole::Admin
            }
        }
    }
}

impl Scope {
    /// A live page in a live space.
    pub fn live(role: Option<SpaceRole>) -> Self {
        Self {
            role,
            ..Self::default()
        }
    }

    pub fn allows(self, action: SpaceAction) -> bool {
        use SpaceAction::*;
        if self.role.is_none_or(|role| role < action.required_role()) {
            return false;
        }
        if self.space_trashed {
            return action == RestoreSpace;
        }
        match action {
            RestoreSpace => false,
            RestorePage | PurgePage => self.page_trashed,
            ReadSpace | ListTrash | ManageSpace | TrashSpace => true,
            _ => !self.page_trashed,
        }
    }
}

/// The action a change of workspace role needs. `next` is `None` for a removal.
pub fn member_change_action(
    current: WorkspaceRole,
    next: Option<WorkspaceRole>,
) -> WorkspaceAction {
    if current == WorkspaceRole::Owner || next == Some(WorkspaceRole::Owner) {
        WorkspaceAction::ManageOwner
    } else {
        WorkspaceAction::ManageMember
    }
}

/// A workspace always keeps one owner. `next` is `None` for a removal.
pub fn would_remove_last_owner(
    current: WorkspaceRole,
    next: Option<WorkspaceRole>,
    owner_count: usize,
) -> bool {
    current == WorkspaceRole::Owner && next != Some(WorkspaceRole::Owner) && owner_count <= 1
}

/// An instance always keeps one enabled instance admin. Call it before disabling, deleting or
/// demoting a user.
pub fn would_remove_last_instance_admin(
    target_is_enabled_admin: bool,
    enabled_admin_count: usize,
) -> bool {
    target_is_enabled_admin && enabled_admin_count <= 1
}

pub fn move_allowed(source: Scope, destination: Scope, same_workspace: bool) -> bool {
    same_workspace
        && source.allows(SpaceAction::EditPage)
        && destination.allows(SpaceAction::EditPage)
}

/// A download needs read access to at least one page that references the file.
pub fn download_allowed(referencing_pages: impl IntoIterator<Item = Scope>) -> bool {
    referencing_pages
        .into_iter()
        .any(|page| page.allows(SpaceAction::ReadPage))
}

/// The mode a page socket opens in, `None` when it must not open.
pub fn page_socket_mode(scope: Scope) -> Option<SocketMode> {
    if scope.allows(SpaceAction::OpenSocketReadWrite) {
        Some(SocketMode::ReadWrite)
    } else if scope.allows(SpaceAction::OpenSocketReadOnly) {
        Some(SocketMode::ReadOnly)
    } else {
        None
    }
}

/// What happens to an open page socket after a change that may lower access. A session that
/// ended closes with `4401` before reaching this check. Only a user who keeps a role learns that
/// the page is in the trash.
pub fn recompute_page_socket(current: SocketMode, scope: Scope) -> SocketOutcome {
    if scope.role.is_some() && (scope.space_trashed || scope.page_trashed) {
        return SocketOutcome::Close(CLOSE_NOT_FOUND);
    }
    match page_socket_mode(scope) {
        None => SocketOutcome::Close(CLOSE_FORBIDDEN),
        Some(mode) if mode == current => SocketOutcome::Keep,
        Some(_) => SocketOutcome::Close(CLOSE_MODE_CHANGED),
    }
}
