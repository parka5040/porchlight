# Permissions matrix

Generated from `porchlight-core::access`; do not edit by hand. From the repository root, regenerate with:

```sh
CARGO_NET_OFFLINE=true cargo run --manifest-path server/Cargo.toml -p porchlight-core --example permissions-matrix
```

The caller is already authenticated as a live member of the item's household or an agent.
Subjects gain no implicit access; app super admins obey visibility; dependents cannot call the app.
Agents see everything and hold all item capabilities (agent token scopes are enforced separately).
`UD` means the subject is a dependent and there are **no assignees**. Every combination of flags
is listed, including impossible states such as an assignee on an unassigned item, to test a total policy.
Capability arrays use the stable API order. The HTTP endpoint dimension is added by S4.

| Caller | Visibility | Owner | Assignee | Viewer | Subject | UD | Capabilities |
| --- | --- | --- | --- | --- | --- | --- | --- |
| super_admin | household | no | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | no | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | no | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | no | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | no | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | no | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | no | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | no | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | household | no | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | no | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | no | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | no | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | no | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | household | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | no | no | no | no | no | — |
| super_admin | private | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | no | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | no | no | yes | no | no | — |
| super_admin | private | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | no | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | no | no | no | yes | no | — |
| super_admin | private | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | no | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | no | no | yes | yes | no | — |
| super_admin | private | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | no | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | private | no | no | no | no | yes | — |
| super_admin | private | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | no | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | no | no | yes | no | yes | — |
| super_admin | private | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | no | no | no | yes | yes | — |
| super_admin | private | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | no | no | yes | yes | yes | — |
| super_admin | private | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | private | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | no | no | no | no | no | — |
| super_admin | selected | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | no | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | no | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | no | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | no | no | no | yes | no | — |
| super_admin | selected | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | no | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | no | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | no | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| super_admin | selected | no | no | no | no | yes | — |
| super_admin | selected | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | no | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | no | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | no | no | no | yes | yes | — |
| super_admin | selected | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | no | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| super_admin | selected | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | household | no | no | no | no | no | view, comment |
| user | household | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | household | no | yes | no | no | no | view, comment, change_status, complete, snooze |
| user | household | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | household | no | no | yes | no | no | view, comment |
| user | household | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | household | no | yes | yes | no | no | view, comment, change_status, complete, snooze |
| user | household | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | household | no | no | no | yes | no | view, comment |
| user | household | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | household | no | yes | no | yes | no | view, comment, change_status, complete, snooze |
| user | household | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | household | no | no | yes | yes | no | view, comment |
| user | household | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | household | no | yes | yes | yes | no | view, comment, change_status, complete, snooze |
| user | household | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | household | no | no | no | no | yes | view, comment, take |
| user | household | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | household | no | yes | no | no | yes | view, comment, change_status, complete, snooze, take |
| user | household | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | household | no | no | yes | no | yes | view, comment, take |
| user | household | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | household | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, take |
| user | household | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | household | no | no | no | yes | yes | view, comment, take |
| user | household | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | household | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, take |
| user | household | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | household | no | no | yes | yes | yes | view, comment, take |
| user | household | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | household | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, take |
| user | household | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | private | no | no | no | no | no | — |
| user | private | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | private | no | yes | no | no | no | view, comment, change_status, complete, snooze |
| user | private | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | private | no | no | yes | no | no | — |
| user | private | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | private | no | yes | yes | no | no | view, comment, change_status, complete, snooze |
| user | private | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | private | no | no | no | yes | no | — |
| user | private | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | private | no | yes | no | yes | no | view, comment, change_status, complete, snooze |
| user | private | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | private | no | no | yes | yes | no | — |
| user | private | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | private | no | yes | yes | yes | no | view, comment, change_status, complete, snooze |
| user | private | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | private | no | no | no | no | yes | — |
| user | private | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | private | no | yes | no | no | yes | view, comment, change_status, complete, snooze, take |
| user | private | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | private | no | no | yes | no | yes | — |
| user | private | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | private | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, take |
| user | private | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | private | no | no | no | yes | yes | — |
| user | private | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | private | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, take |
| user | private | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | private | no | no | yes | yes | yes | — |
| user | private | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | private | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, take |
| user | private | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | selected | no | no | no | no | no | — |
| user | selected | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | selected | no | yes | no | no | no | view, comment, change_status, complete, snooze |
| user | selected | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | selected | no | no | yes | no | no | view, comment |
| user | selected | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | selected | no | yes | yes | no | no | view, comment, change_status, complete, snooze |
| user | selected | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | selected | no | no | no | yes | no | — |
| user | selected | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | selected | no | yes | no | yes | no | view, comment, change_status, complete, snooze |
| user | selected | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | selected | no | no | yes | yes | no | view, comment |
| user | selected | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | selected | no | yes | yes | yes | no | view, comment, change_status, complete, snooze |
| user | selected | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, delete |
| user | selected | no | no | no | no | yes | — |
| user | selected | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | selected | no | yes | no | no | yes | view, comment, change_status, complete, snooze, take |
| user | selected | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | selected | no | no | yes | no | yes | view, comment, take |
| user | selected | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | selected | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, take |
| user | selected | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | selected | no | no | no | yes | yes | — |
| user | selected | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | selected | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, take |
| user | selected | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | selected | no | no | yes | yes | yes | view, comment, take |
| user | selected | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| user | selected | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, take |
| user | selected | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| dependent | household | no | no | no | no | no | — |
| dependent | household | yes | no | no | no | no | — |
| dependent | household | no | yes | no | no | no | — |
| dependent | household | yes | yes | no | no | no | — |
| dependent | household | no | no | yes | no | no | — |
| dependent | household | yes | no | yes | no | no | — |
| dependent | household | no | yes | yes | no | no | — |
| dependent | household | yes | yes | yes | no | no | — |
| dependent | household | no | no | no | yes | no | — |
| dependent | household | yes | no | no | yes | no | — |
| dependent | household | no | yes | no | yes | no | — |
| dependent | household | yes | yes | no | yes | no | — |
| dependent | household | no | no | yes | yes | no | — |
| dependent | household | yes | no | yes | yes | no | — |
| dependent | household | no | yes | yes | yes | no | — |
| dependent | household | yes | yes | yes | yes | no | — |
| dependent | household | no | no | no | no | yes | — |
| dependent | household | yes | no | no | no | yes | — |
| dependent | household | no | yes | no | no | yes | — |
| dependent | household | yes | yes | no | no | yes | — |
| dependent | household | no | no | yes | no | yes | — |
| dependent | household | yes | no | yes | no | yes | — |
| dependent | household | no | yes | yes | no | yes | — |
| dependent | household | yes | yes | yes | no | yes | — |
| dependent | household | no | no | no | yes | yes | — |
| dependent | household | yes | no | no | yes | yes | — |
| dependent | household | no | yes | no | yes | yes | — |
| dependent | household | yes | yes | no | yes | yes | — |
| dependent | household | no | no | yes | yes | yes | — |
| dependent | household | yes | no | yes | yes | yes | — |
| dependent | household | no | yes | yes | yes | yes | — |
| dependent | household | yes | yes | yes | yes | yes | — |
| dependent | private | no | no | no | no | no | — |
| dependent | private | yes | no | no | no | no | — |
| dependent | private | no | yes | no | no | no | — |
| dependent | private | yes | yes | no | no | no | — |
| dependent | private | no | no | yes | no | no | — |
| dependent | private | yes | no | yes | no | no | — |
| dependent | private | no | yes | yes | no | no | — |
| dependent | private | yes | yes | yes | no | no | — |
| dependent | private | no | no | no | yes | no | — |
| dependent | private | yes | no | no | yes | no | — |
| dependent | private | no | yes | no | yes | no | — |
| dependent | private | yes | yes | no | yes | no | — |
| dependent | private | no | no | yes | yes | no | — |
| dependent | private | yes | no | yes | yes | no | — |
| dependent | private | no | yes | yes | yes | no | — |
| dependent | private | yes | yes | yes | yes | no | — |
| dependent | private | no | no | no | no | yes | — |
| dependent | private | yes | no | no | no | yes | — |
| dependent | private | no | yes | no | no | yes | — |
| dependent | private | yes | yes | no | no | yes | — |
| dependent | private | no | no | yes | no | yes | — |
| dependent | private | yes | no | yes | no | yes | — |
| dependent | private | no | yes | yes | no | yes | — |
| dependent | private | yes | yes | yes | no | yes | — |
| dependent | private | no | no | no | yes | yes | — |
| dependent | private | yes | no | no | yes | yes | — |
| dependent | private | no | yes | no | yes | yes | — |
| dependent | private | yes | yes | no | yes | yes | — |
| dependent | private | no | no | yes | yes | yes | — |
| dependent | private | yes | no | yes | yes | yes | — |
| dependent | private | no | yes | yes | yes | yes | — |
| dependent | private | yes | yes | yes | yes | yes | — |
| dependent | selected | no | no | no | no | no | — |
| dependent | selected | yes | no | no | no | no | — |
| dependent | selected | no | yes | no | no | no | — |
| dependent | selected | yes | yes | no | no | no | — |
| dependent | selected | no | no | yes | no | no | — |
| dependent | selected | yes | no | yes | no | no | — |
| dependent | selected | no | yes | yes | no | no | — |
| dependent | selected | yes | yes | yes | no | no | — |
| dependent | selected | no | no | no | yes | no | — |
| dependent | selected | yes | no | no | yes | no | — |
| dependent | selected | no | yes | no | yes | no | — |
| dependent | selected | yes | yes | no | yes | no | — |
| dependent | selected | no | no | yes | yes | no | — |
| dependent | selected | yes | no | yes | yes | no | — |
| dependent | selected | no | yes | yes | yes | no | — |
| dependent | selected | yes | yes | yes | yes | no | — |
| dependent | selected | no | no | no | no | yes | — |
| dependent | selected | yes | no | no | no | yes | — |
| dependent | selected | no | yes | no | no | yes | — |
| dependent | selected | yes | yes | no | no | yes | — |
| dependent | selected | no | no | yes | no | yes | — |
| dependent | selected | yes | no | yes | no | yes | — |
| dependent | selected | no | yes | yes | no | yes | — |
| dependent | selected | yes | yes | yes | no | yes | — |
| dependent | selected | no | no | no | yes | yes | — |
| dependent | selected | yes | no | no | yes | yes | — |
| dependent | selected | no | yes | no | yes | yes | — |
| dependent | selected | yes | yes | no | yes | yes | — |
| dependent | selected | no | no | yes | yes | yes | — |
| dependent | selected | yes | no | yes | yes | yes | — |
| dependent | selected | no | yes | yes | yes | yes | — |
| dependent | selected | yes | yes | yes | yes | yes | — |
| agent | household | no | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | household | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | private | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | no | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | yes | no | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | no | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | yes | yes | no | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | no | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | yes | no | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | no | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | yes | yes | yes | no | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | no | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | yes | no | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | no | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | yes | yes | no | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | no | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | yes | no | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | no | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | no | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |
| agent | selected | yes | yes | yes | yes | yes | view, comment, change_status, complete, snooze, edit, edit_reminders, reassign, change_visibility, take, delete |

<!-- HTTP endpoint matrix -->

## HTTP endpoint dimension

Generated from `porchlight_api::permissions`; regenerate both dimensions with `CARGO_NET_OFFLINE=true cargo run --manifest-path server/Cargo.toml -p porchlight-api --example endpoint-matrix`.

Domain rows use live session+mTLS OR a PAT on the agent socket. `/v1/admin/…` rows are agent-only (app callers receive 404); `/v1/feeds…` rows are app-only with separate `/v1/admin/feeds…` counterparts. `/v1/stream` is session+mTLS only (agent rows are 404); SSE results mean allowed tickle or filtered frame, not HTTP statuses. PAT scopes form the hierarchy **admin ⊇ write ⊇ read**: every live token can read; mutations require write or admin; admin-only endpoints require admin. Endpoint-owned pluggable policies describe item, member self-only, list, settings and admin rules. UD records whether an item is unassigned and about a dependent; take requires UD=true even for agents. Additional cases cover request-specific permission branches. App creation sets caller as owner; agents may choose any live household owner. PATCH details/overrides require edit, owner transfer additionally reassign, visibility/viewers change_visibility; status/board/reopen change_status; complete complete. Versioned mutations require If-Match and may return 204 when access is removed. Collection reads omit invisible items. Conflict masking nulls both counterpart ids and replaces its title with Busy. Idempotency binds method, path/query and body, not If-Match; rotate the key after version-conflict.

Non-item rows cross caller kind and PAT scope with **self / other** target members independently of item ACLs. For `/v1/me/settings` the route always resolves the caller's settings; other-target rows demonstrate that an unrelated member context cannot retarget it. Read PATs cannot mutate; write/admin PATs may patch their owner's member, but cannot request `make_zone_authority:true` (no device). Item rows retain their item ACL dimension and use write-scoped PATs; custom item policies additionally cross independent self/other target members and all PAT scopes. Collaboration rows likewise cross the full item ACL dimension with all PAT scopes and independent resource self/other (author or uploader), not item ownership. The comment-attachment delete case uses comment authorship/moderation rather than the item-editor rule. Notification SSE rows likewise retain the full item relationship/visibility dimension while independently crossing recipient self/other and all PAT scopes; item ACL never grants another member’s notification. List rows retain private/household/selected and owner/viewer/inherited item relationships, independently crossed with self/other and all three PAT scopes; self never changes list ACLs.

| Method | Endpoint | Case | Relationship / role | PAT scope | Target member / resource | Visibility | UD | Policy | Result |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| GET | `/v1/items` |  | owner | — | — | household | false | authenticated | 200 |
| GET | `/v1/items` |  | assignee | — | — | household | false | authenticated | 200 |
| GET | `/v1/items` |  | viewer | — | — | household | false | authenticated | 200 |
| GET | `/v1/items` |  | subject_only | — | — | household | false | authenticated | 200 |
| GET | `/v1/items` |  | other_user | — | — | household | false | authenticated | 200 |
| GET | `/v1/items` |  | super_admin_app | — | — | household | false | authenticated | 200 |
| GET | `/v1/items` |  | agent | write | — | household | false | authenticated | 200 |
| GET | `/v1/items` |  | owner | — | — | private | false | authenticated | 200 |
| GET | `/v1/items` |  | assignee | — | — | private | false | authenticated | 200 |
| GET | `/v1/items` |  | viewer | — | — | private | false | authenticated | 200 |
| GET | `/v1/items` |  | subject_only | — | — | private | false | authenticated | 200 |
| GET | `/v1/items` |  | other_user | — | — | private | false | authenticated | 200 |
| GET | `/v1/items` |  | super_admin_app | — | — | private | false | authenticated | 200 |
| GET | `/v1/items` |  | agent | write | — | private | false | authenticated | 200 |
| GET | `/v1/items` |  | owner | — | — | selected | false | authenticated | 200 |
| GET | `/v1/items` |  | assignee | — | — | selected | false | authenticated | 200 |
| GET | `/v1/items` |  | viewer | — | — | selected | false | authenticated | 200 |
| GET | `/v1/items` |  | subject_only | — | — | selected | false | authenticated | 200 |
| GET | `/v1/items` |  | other_user | — | — | selected | false | authenticated | 200 |
| GET | `/v1/items` |  | super_admin_app | — | — | selected | false | authenticated | 200 |
| GET | `/v1/items` |  | agent | write | — | selected | false | authenticated | 200 |
| POST | `/v1/items` |  | owner | — | — | household | false | authenticated | 201 |
| POST | `/v1/items` |  | assignee | — | — | household | false | authenticated | 201 |
| POST | `/v1/items` |  | viewer | — | — | household | false | authenticated | 201 |
| POST | `/v1/items` |  | subject_only | — | — | household | false | authenticated | 201 |
| POST | `/v1/items` |  | other_user | — | — | household | false | authenticated | 201 |
| POST | `/v1/items` |  | super_admin_app | — | — | household | false | authenticated | 201 |
| POST | `/v1/items` |  | agent | write | — | household | false | authenticated | 201 |
| POST | `/v1/items` |  | owner | — | — | private | false | authenticated | 201 |
| POST | `/v1/items` |  | assignee | — | — | private | false | authenticated | 201 |
| POST | `/v1/items` |  | viewer | — | — | private | false | authenticated | 201 |
| POST | `/v1/items` |  | subject_only | — | — | private | false | authenticated | 201 |
| POST | `/v1/items` |  | other_user | — | — | private | false | authenticated | 201 |
| POST | `/v1/items` |  | super_admin_app | — | — | private | false | authenticated | 201 |
| POST | `/v1/items` |  | agent | write | — | private | false | authenticated | 201 |
| POST | `/v1/items` |  | owner | — | — | selected | false | authenticated | 201 |
| POST | `/v1/items` |  | assignee | — | — | selected | false | authenticated | 201 |
| POST | `/v1/items` |  | viewer | — | — | selected | false | authenticated | 201 |
| POST | `/v1/items` |  | subject_only | — | — | selected | false | authenticated | 201 |
| POST | `/v1/items` |  | other_user | — | — | selected | false | authenticated | 201 |
| POST | `/v1/items` |  | super_admin_app | — | — | selected | false | authenticated | 201 |
| POST | `/v1/items` |  | agent | write | — | selected | false | authenticated | 201 |
| GET | `/v1/items/{id}` |  | owner | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}` |  | assignee | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}` |  | viewer | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}` |  | subject_only | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}` |  | other_user | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}` |  | super_admin_app | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}` |  | agent | write | — | household | false | view | 200 |
| GET | `/v1/items/{id}` |  | owner | — | — | private | false | view | 200 |
| GET | `/v1/items/{id}` |  | assignee | — | — | private | false | view | 200 |
| GET | `/v1/items/{id}` |  | viewer | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}` |  | subject_only | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}` |  | other_user | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}` |  | super_admin_app | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}` |  | agent | write | — | private | false | view | 200 |
| GET | `/v1/items/{id}` |  | owner | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}` |  | assignee | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}` |  | viewer | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}` |  | subject_only | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}` |  | other_user | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}` |  | super_admin_app | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}` |  | agent | write | — | selected | false | view | 200 |
| PATCH | `/v1/items/{id}` |  | owner | — | — | household | false | edit | 200 |
| PATCH | `/v1/items/{id}` |  | assignee | — | — | household | false | edit | 403 |
| PATCH | `/v1/items/{id}` |  | viewer | — | — | household | false | edit | 403 |
| PATCH | `/v1/items/{id}` |  | subject_only | — | — | household | false | edit | 403 |
| PATCH | `/v1/items/{id}` |  | other_user | — | — | household | false | edit | 403 |
| PATCH | `/v1/items/{id}` |  | super_admin_app | — | — | household | false | edit | 200 |
| PATCH | `/v1/items/{id}` |  | agent | write | — | household | false | edit | 200 |
| PATCH | `/v1/items/{id}` |  | owner | — | — | private | false | edit | 200 |
| PATCH | `/v1/items/{id}` |  | assignee | — | — | private | false | edit | 403 |
| PATCH | `/v1/items/{id}` |  | viewer | — | — | private | false | edit | 404 |
| PATCH | `/v1/items/{id}` |  | subject_only | — | — | private | false | edit | 404 |
| PATCH | `/v1/items/{id}` |  | other_user | — | — | private | false | edit | 404 |
| PATCH | `/v1/items/{id}` |  | super_admin_app | — | — | private | false | edit | 404 |
| PATCH | `/v1/items/{id}` |  | agent | write | — | private | false | edit | 200 |
| PATCH | `/v1/items/{id}` |  | owner | — | — | selected | false | edit | 200 |
| PATCH | `/v1/items/{id}` |  | assignee | — | — | selected | false | edit | 403 |
| PATCH | `/v1/items/{id}` |  | viewer | — | — | selected | false | edit | 403 |
| PATCH | `/v1/items/{id}` |  | subject_only | — | — | selected | false | edit | 404 |
| PATCH | `/v1/items/{id}` |  | other_user | — | — | selected | false | edit | 404 |
| PATCH | `/v1/items/{id}` |  | super_admin_app | — | — | selected | false | edit | 404 |
| PATCH | `/v1/items/{id}` |  | agent | write | — | selected | false | edit | 200 |
| DELETE | `/v1/items/{id}` |  | owner | — | — | household | false | delete | 200 |
| DELETE | `/v1/items/{id}` |  | assignee | — | — | household | false | delete | 403 |
| DELETE | `/v1/items/{id}` |  | viewer | — | — | household | false | delete | 403 |
| DELETE | `/v1/items/{id}` |  | subject_only | — | — | household | false | delete | 403 |
| DELETE | `/v1/items/{id}` |  | other_user | — | — | household | false | delete | 403 |
| DELETE | `/v1/items/{id}` |  | super_admin_app | — | — | household | false | delete | 200 |
| DELETE | `/v1/items/{id}` |  | agent | write | — | household | false | delete | 200 |
| DELETE | `/v1/items/{id}` |  | owner | — | — | private | false | delete | 200 |
| DELETE | `/v1/items/{id}` |  | assignee | — | — | private | false | delete | 403 |
| DELETE | `/v1/items/{id}` |  | viewer | — | — | private | false | delete | 404 |
| DELETE | `/v1/items/{id}` |  | subject_only | — | — | private | false | delete | 404 |
| DELETE | `/v1/items/{id}` |  | other_user | — | — | private | false | delete | 404 |
| DELETE | `/v1/items/{id}` |  | super_admin_app | — | — | private | false | delete | 404 |
| DELETE | `/v1/items/{id}` |  | agent | write | — | private | false | delete | 200 |
| DELETE | `/v1/items/{id}` |  | owner | — | — | selected | false | delete | 200 |
| DELETE | `/v1/items/{id}` |  | assignee | — | — | selected | false | delete | 403 |
| DELETE | `/v1/items/{id}` |  | viewer | — | — | selected | false | delete | 403 |
| DELETE | `/v1/items/{id}` |  | subject_only | — | — | selected | false | delete | 404 |
| DELETE | `/v1/items/{id}` |  | other_user | — | — | selected | false | delete | 404 |
| DELETE | `/v1/items/{id}` |  | super_admin_app | — | — | selected | false | delete | 404 |
| DELETE | `/v1/items/{id}` |  | agent | write | — | selected | false | delete | 200 |
| PUT | `/v1/items/{id}/assignees` |  | owner | — | — | household | false | reassign | 200 |
| PUT | `/v1/items/{id}/assignees` |  | assignee | — | — | household | false | reassign | 403 |
| PUT | `/v1/items/{id}/assignees` |  | viewer | — | — | household | false | reassign | 403 |
| PUT | `/v1/items/{id}/assignees` |  | subject_only | — | — | household | false | reassign | 403 |
| PUT | `/v1/items/{id}/assignees` |  | other_user | — | — | household | false | reassign | 403 |
| PUT | `/v1/items/{id}/assignees` |  | super_admin_app | — | — | household | false | reassign | 200 |
| PUT | `/v1/items/{id}/assignees` |  | agent | write | — | household | false | reassign | 200 |
| PUT | `/v1/items/{id}/assignees` |  | owner | — | — | private | false | reassign | 200 |
| PUT | `/v1/items/{id}/assignees` |  | assignee | — | — | private | false | reassign | 403 |
| PUT | `/v1/items/{id}/assignees` |  | viewer | — | — | private | false | reassign | 404 |
| PUT | `/v1/items/{id}/assignees` |  | subject_only | — | — | private | false | reassign | 404 |
| PUT | `/v1/items/{id}/assignees` |  | other_user | — | — | private | false | reassign | 404 |
| PUT | `/v1/items/{id}/assignees` |  | super_admin_app | — | — | private | false | reassign | 404 |
| PUT | `/v1/items/{id}/assignees` |  | agent | write | — | private | false | reassign | 200 |
| PUT | `/v1/items/{id}/assignees` |  | owner | — | — | selected | false | reassign | 200 |
| PUT | `/v1/items/{id}/assignees` |  | assignee | — | — | selected | false | reassign | 403 |
| PUT | `/v1/items/{id}/assignees` |  | viewer | — | — | selected | false | reassign | 403 |
| PUT | `/v1/items/{id}/assignees` |  | subject_only | — | — | selected | false | reassign | 404 |
| PUT | `/v1/items/{id}/assignees` |  | other_user | — | — | selected | false | reassign | 404 |
| PUT | `/v1/items/{id}/assignees` |  | super_admin_app | — | — | selected | false | reassign | 404 |
| PUT | `/v1/items/{id}/assignees` |  | agent | write | — | selected | false | reassign | 200 |
| PUT | `/v1/items/{id}/viewers` |  | owner | — | — | household | false | change_visibility | 200 |
| PUT | `/v1/items/{id}/viewers` |  | assignee | — | — | household | false | change_visibility | 403 |
| PUT | `/v1/items/{id}/viewers` |  | viewer | — | — | household | false | change_visibility | 403 |
| PUT | `/v1/items/{id}/viewers` |  | subject_only | — | — | household | false | change_visibility | 403 |
| PUT | `/v1/items/{id}/viewers` |  | other_user | — | — | household | false | change_visibility | 403 |
| PUT | `/v1/items/{id}/viewers` |  | super_admin_app | — | — | household | false | change_visibility | 200 |
| PUT | `/v1/items/{id}/viewers` |  | agent | write | — | household | false | change_visibility | 200 |
| PUT | `/v1/items/{id}/viewers` |  | owner | — | — | private | false | change_visibility | 200 |
| PUT | `/v1/items/{id}/viewers` |  | assignee | — | — | private | false | change_visibility | 403 |
| PUT | `/v1/items/{id}/viewers` |  | viewer | — | — | private | false | change_visibility | 404 |
| PUT | `/v1/items/{id}/viewers` |  | subject_only | — | — | private | false | change_visibility | 404 |
| PUT | `/v1/items/{id}/viewers` |  | other_user | — | — | private | false | change_visibility | 404 |
| PUT | `/v1/items/{id}/viewers` |  | super_admin_app | — | — | private | false | change_visibility | 404 |
| PUT | `/v1/items/{id}/viewers` |  | agent | write | — | private | false | change_visibility | 200 |
| PUT | `/v1/items/{id}/viewers` |  | owner | — | — | selected | false | change_visibility | 200 |
| PUT | `/v1/items/{id}/viewers` |  | assignee | — | — | selected | false | change_visibility | 403 |
| PUT | `/v1/items/{id}/viewers` |  | viewer | — | — | selected | false | change_visibility | 403 |
| PUT | `/v1/items/{id}/viewers` |  | subject_only | — | — | selected | false | change_visibility | 404 |
| PUT | `/v1/items/{id}/viewers` |  | other_user | — | — | selected | false | change_visibility | 404 |
| PUT | `/v1/items/{id}/viewers` |  | super_admin_app | — | — | selected | false | change_visibility | 404 |
| PUT | `/v1/items/{id}/viewers` |  | agent | write | — | selected | false | change_visibility | 200 |
| PUT | `/v1/items/{id}/reminders` |  | owner | — | — | household | false | edit_reminders | 200 |
| PUT | `/v1/items/{id}/reminders` |  | assignee | — | — | household | false | edit_reminders | 403 |
| PUT | `/v1/items/{id}/reminders` |  | viewer | — | — | household | false | edit_reminders | 403 |
| PUT | `/v1/items/{id}/reminders` |  | subject_only | — | — | household | false | edit_reminders | 403 |
| PUT | `/v1/items/{id}/reminders` |  | other_user | — | — | household | false | edit_reminders | 403 |
| PUT | `/v1/items/{id}/reminders` |  | super_admin_app | — | — | household | false | edit_reminders | 200 |
| PUT | `/v1/items/{id}/reminders` |  | agent | write | — | household | false | edit_reminders | 200 |
| PUT | `/v1/items/{id}/reminders` |  | owner | — | — | private | false | edit_reminders | 200 |
| PUT | `/v1/items/{id}/reminders` |  | assignee | — | — | private | false | edit_reminders | 403 |
| PUT | `/v1/items/{id}/reminders` |  | viewer | — | — | private | false | edit_reminders | 404 |
| PUT | `/v1/items/{id}/reminders` |  | subject_only | — | — | private | false | edit_reminders | 404 |
| PUT | `/v1/items/{id}/reminders` |  | other_user | — | — | private | false | edit_reminders | 404 |
| PUT | `/v1/items/{id}/reminders` |  | super_admin_app | — | — | private | false | edit_reminders | 404 |
| PUT | `/v1/items/{id}/reminders` |  | agent | write | — | private | false | edit_reminders | 200 |
| PUT | `/v1/items/{id}/reminders` |  | owner | — | — | selected | false | edit_reminders | 200 |
| PUT | `/v1/items/{id}/reminders` |  | assignee | — | — | selected | false | edit_reminders | 403 |
| PUT | `/v1/items/{id}/reminders` |  | viewer | — | — | selected | false | edit_reminders | 403 |
| PUT | `/v1/items/{id}/reminders` |  | subject_only | — | — | selected | false | edit_reminders | 404 |
| PUT | `/v1/items/{id}/reminders` |  | other_user | — | — | selected | false | edit_reminders | 404 |
| PUT | `/v1/items/{id}/reminders` |  | super_admin_app | — | — | selected | false | edit_reminders | 404 |
| PUT | `/v1/items/{id}/reminders` |  | agent | write | — | selected | false | edit_reminders | 200 |
| POST | `/v1/items/{id}/take` |  | owner | — | — | household | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | assignee | — | — | household | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | viewer | — | — | household | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | subject_only | — | — | household | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | other_user | — | — | household | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | super_admin_app | — | — | household | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | agent | write | — | household | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | owner | — | — | private | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | assignee | — | — | private | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | viewer | — | — | private | false | take | 404 |
| POST | `/v1/items/{id}/take` |  | subject_only | — | — | private | false | take | 404 |
| POST | `/v1/items/{id}/take` |  | other_user | — | — | private | false | take | 404 |
| POST | `/v1/items/{id}/take` |  | super_admin_app | — | — | private | false | take | 404 |
| POST | `/v1/items/{id}/take` |  | agent | write | — | private | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | owner | — | — | selected | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | assignee | — | — | selected | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | viewer | — | — | selected | false | take | 403 |
| POST | `/v1/items/{id}/take` |  | subject_only | — | — | selected | false | take | 404 |
| POST | `/v1/items/{id}/take` |  | other_user | — | — | selected | false | take | 404 |
| POST | `/v1/items/{id}/take` |  | super_admin_app | — | — | selected | false | take | 404 |
| POST | `/v1/items/{id}/take` |  | agent | write | — | selected | false | take | 403 |
| GET | `/v1/items/{id}/occurrences` |  | owner | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | assignee | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | viewer | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | subject_only | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | other_user | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | super_admin_app | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | agent | write | — | household | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | owner | — | — | private | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | assignee | — | — | private | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | viewer | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}/occurrences` |  | subject_only | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}/occurrences` |  | other_user | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}/occurrences` |  | super_admin_app | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}/occurrences` |  | agent | write | — | private | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | owner | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | assignee | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | viewer | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}/occurrences` |  | subject_only | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}/occurrences` |  | other_user | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}/occurrences` |  | super_admin_app | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}/occurrences` |  | agent | write | — | selected | false | view | 200 |
| PATCH | `/v1/occurrences/{id}` |  | owner | — | — | household | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | assignee | — | — | household | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | viewer | — | — | household | false | change_status | 403 |
| PATCH | `/v1/occurrences/{id}` |  | subject_only | — | — | household | false | change_status | 403 |
| PATCH | `/v1/occurrences/{id}` |  | other_user | — | — | household | false | change_status | 403 |
| PATCH | `/v1/occurrences/{id}` |  | super_admin_app | — | — | household | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | agent | write | — | household | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | owner | — | — | private | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | assignee | — | — | private | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | viewer | — | — | private | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` |  | subject_only | — | — | private | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` |  | other_user | — | — | private | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` |  | super_admin_app | — | — | private | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` |  | agent | write | — | private | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | owner | — | — | selected | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | assignee | — | — | selected | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` |  | viewer | — | — | selected | false | change_status | 403 |
| PATCH | `/v1/occurrences/{id}` |  | subject_only | — | — | selected | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` |  | other_user | — | — | selected | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` |  | super_admin_app | — | — | selected | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` |  | agent | write | — | selected | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | override | owner | — | — | household | false | edit | 200 |
| PATCH | `/v1/occurrences/{id}` | override | assignee | — | — | household | false | edit | 403 |
| PATCH | `/v1/occurrences/{id}` | override | viewer | — | — | household | false | edit | 403 |
| PATCH | `/v1/occurrences/{id}` | override | subject_only | — | — | household | false | edit | 403 |
| PATCH | `/v1/occurrences/{id}` | override | other_user | — | — | household | false | edit | 403 |
| PATCH | `/v1/occurrences/{id}` | override | super_admin_app | — | — | household | false | edit | 200 |
| PATCH | `/v1/occurrences/{id}` | override | agent | write | — | household | false | edit | 200 |
| PATCH | `/v1/occurrences/{id}` | override | owner | — | — | private | false | edit | 200 |
| PATCH | `/v1/occurrences/{id}` | override | assignee | — | — | private | false | edit | 403 |
| PATCH | `/v1/occurrences/{id}` | override | viewer | — | — | private | false | edit | 404 |
| PATCH | `/v1/occurrences/{id}` | override | subject_only | — | — | private | false | edit | 404 |
| PATCH | `/v1/occurrences/{id}` | override | other_user | — | — | private | false | edit | 404 |
| PATCH | `/v1/occurrences/{id}` | override | super_admin_app | — | — | private | false | edit | 404 |
| PATCH | `/v1/occurrences/{id}` | override | agent | write | — | private | false | edit | 200 |
| PATCH | `/v1/occurrences/{id}` | override | owner | — | — | selected | false | edit | 200 |
| PATCH | `/v1/occurrences/{id}` | override | assignee | — | — | selected | false | edit | 403 |
| PATCH | `/v1/occurrences/{id}` | override | viewer | — | — | selected | false | edit | 403 |
| PATCH | `/v1/occurrences/{id}` | override | subject_only | — | — | selected | false | edit | 404 |
| PATCH | `/v1/occurrences/{id}` | override | other_user | — | — | selected | false | edit | 404 |
| PATCH | `/v1/occurrences/{id}` | override | super_admin_app | — | — | selected | false | edit | 404 |
| PATCH | `/v1/occurrences/{id}` | override | agent | write | — | selected | false | edit | 200 |
| PATCH | `/v1/occurrences/{id}` | board | owner | — | — | household | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | assignee | — | — | household | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | viewer | — | — | household | false | change_status | 403 |
| PATCH | `/v1/occurrences/{id}` | board | subject_only | — | — | household | false | change_status | 403 |
| PATCH | `/v1/occurrences/{id}` | board | other_user | — | — | household | false | change_status | 403 |
| PATCH | `/v1/occurrences/{id}` | board | super_admin_app | — | — | household | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | agent | write | — | household | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | owner | — | — | private | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | assignee | — | — | private | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | viewer | — | — | private | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` | board | subject_only | — | — | private | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` | board | other_user | — | — | private | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` | board | super_admin_app | — | — | private | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` | board | agent | write | — | private | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | owner | — | — | selected | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | assignee | — | — | selected | false | change_status | 200 |
| PATCH | `/v1/occurrences/{id}` | board | viewer | — | — | selected | false | change_status | 403 |
| PATCH | `/v1/occurrences/{id}` | board | subject_only | — | — | selected | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` | board | other_user | — | — | selected | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` | board | super_admin_app | — | — | selected | false | change_status | 404 |
| PATCH | `/v1/occurrences/{id}` | board | agent | write | — | selected | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | owner | — | — | household | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | assignee | — | — | household | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | viewer | — | — | household | false | complete | 403 |
| POST | `/v1/occurrences/{id}/complete` |  | subject_only | — | — | household | false | complete | 403 |
| POST | `/v1/occurrences/{id}/complete` |  | other_user | — | — | household | false | complete | 403 |
| POST | `/v1/occurrences/{id}/complete` |  | super_admin_app | — | — | household | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | agent | write | — | household | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | owner | — | — | private | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | assignee | — | — | private | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | viewer | — | — | private | false | complete | 404 |
| POST | `/v1/occurrences/{id}/complete` |  | subject_only | — | — | private | false | complete | 404 |
| POST | `/v1/occurrences/{id}/complete` |  | other_user | — | — | private | false | complete | 404 |
| POST | `/v1/occurrences/{id}/complete` |  | super_admin_app | — | — | private | false | complete | 404 |
| POST | `/v1/occurrences/{id}/complete` |  | agent | write | — | private | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | owner | — | — | selected | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | assignee | — | — | selected | false | complete | 200 |
| POST | `/v1/occurrences/{id}/complete` |  | viewer | — | — | selected | false | complete | 403 |
| POST | `/v1/occurrences/{id}/complete` |  | subject_only | — | — | selected | false | complete | 404 |
| POST | `/v1/occurrences/{id}/complete` |  | other_user | — | — | selected | false | complete | 404 |
| POST | `/v1/occurrences/{id}/complete` |  | super_admin_app | — | — | selected | false | complete | 404 |
| POST | `/v1/occurrences/{id}/complete` |  | agent | write | — | selected | false | complete | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | owner | — | — | household | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | assignee | — | — | household | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | viewer | — | — | household | false | change_status | 403 |
| POST | `/v1/occurrences/{id}/reopen` |  | subject_only | — | — | household | false | change_status | 403 |
| POST | `/v1/occurrences/{id}/reopen` |  | other_user | — | — | household | false | change_status | 403 |
| POST | `/v1/occurrences/{id}/reopen` |  | super_admin_app | — | — | household | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | agent | write | — | household | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | owner | — | — | private | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | assignee | — | — | private | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | viewer | — | — | private | false | change_status | 404 |
| POST | `/v1/occurrences/{id}/reopen` |  | subject_only | — | — | private | false | change_status | 404 |
| POST | `/v1/occurrences/{id}/reopen` |  | other_user | — | — | private | false | change_status | 404 |
| POST | `/v1/occurrences/{id}/reopen` |  | super_admin_app | — | — | private | false | change_status | 404 |
| POST | `/v1/occurrences/{id}/reopen` |  | agent | write | — | private | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | owner | — | — | selected | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | assignee | — | — | selected | false | change_status | 200 |
| POST | `/v1/occurrences/{id}/reopen` |  | viewer | — | — | selected | false | change_status | 403 |
| POST | `/v1/occurrences/{id}/reopen` |  | subject_only | — | — | selected | false | change_status | 404 |
| POST | `/v1/occurrences/{id}/reopen` |  | other_user | — | — | selected | false | change_status | 404 |
| POST | `/v1/occurrences/{id}/reopen` |  | super_admin_app | — | — | selected | false | change_status | 404 |
| POST | `/v1/occurrences/{id}/reopen` |  | agent | write | — | selected | false | change_status | 200 |
| POST | `/v1/items/{id}/take` |  | owner | — | — | household | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | viewer | — | — | household | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | other_user | — | — | household | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | super_admin_app | — | — | household | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | agent | write | — | household | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | owner | — | — | private | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | viewer | — | — | private | true | take | 404 |
| POST | `/v1/items/{id}/take` |  | other_user | — | — | private | true | take | 404 |
| POST | `/v1/items/{id}/take` |  | super_admin_app | — | — | private | true | take | 404 |
| POST | `/v1/items/{id}/take` |  | agent | write | — | private | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | owner | — | — | selected | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | viewer | — | — | selected | true | take | 200 |
| POST | `/v1/items/{id}/take` |  | other_user | — | — | selected | true | take | 404 |
| POST | `/v1/items/{id}/take` |  | super_admin_app | — | — | selected | true | take | 404 |
| POST | `/v1/items/{id}/take` |  | agent | write | — | selected | true | take | 200 |
| GET | `/v1/board` |  | user | — | self | — | — | authenticated | 200 |
| GET | `/v1/board` |  | user | — | other | — | — | authenticated | 200 |
| GET | `/v1/board` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| GET | `/v1/board` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| GET | `/v1/board` |  | agent | read | self | — | — | authenticated | 200 |
| GET | `/v1/board` |  | agent | read | other | — | — | authenticated | 200 |
| GET | `/v1/board` |  | agent | write | self | — | — | authenticated | 200 |
| GET | `/v1/board` |  | agent | write | other | — | — | authenticated | 200 |
| GET | `/v1/board` |  | agent | admin | self | — | — | authenticated | 200 |
| GET | `/v1/board` |  | agent | admin | other | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | user | — | self | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | user | — | other | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | agent | read | self | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | agent | read | other | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | agent | write | self | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | agent | write | other | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | agent | admin | self | — | — | authenticated | 200 |
| GET | `/v1/agenda` |  | agent | admin | other | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | user | — | self | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | user | — | other | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | agent | read | self | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | agent | read | other | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | agent | write | self | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | agent | write | other | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | agent | admin | self | — | — | authenticated | 200 |
| GET | `/v1/snapshot` |  | agent | admin | other | — | — | authenticated | 200 |
| GET | `/v1/search` |  | user | — | self | — | — | authenticated | 200 |
| GET | `/v1/search` |  | user | — | other | — | — | authenticated | 200 |
| GET | `/v1/search` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| GET | `/v1/search` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| GET | `/v1/search` |  | agent | read | self | — | — | authenticated | 200 |
| GET | `/v1/search` |  | agent | read | other | — | — | authenticated | 200 |
| GET | `/v1/search` |  | agent | write | self | — | — | authenticated | 200 |
| GET | `/v1/search` |  | agent | write | other | — | — | authenticated | 200 |
| GET | `/v1/search` |  | agent | admin | self | — | — | authenticated | 200 |
| GET | `/v1/search` |  | agent | admin | other | — | — | authenticated | 200 |
| GET | `/v1/members` |  | user | — | self | — | — | authenticated | 200 |
| GET | `/v1/members` |  | user | — | other | — | — | authenticated | 200 |
| GET | `/v1/members` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| GET | `/v1/members` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| GET | `/v1/members` |  | agent | read | self | — | — | authenticated | 200 |
| GET | `/v1/members` |  | agent | read | other | — | — | authenticated | 200 |
| GET | `/v1/members` |  | agent | write | self | — | — | authenticated | 200 |
| GET | `/v1/members` |  | agent | write | other | — | — | authenticated | 200 |
| GET | `/v1/members` |  | agent | admin | self | — | — | authenticated | 200 |
| GET | `/v1/members` |  | agent | admin | other | — | — | authenticated | 200 |
| PATCH | `/v1/members/{id}` |  | user | — | self | — | — | self-member | 200 |
| PATCH | `/v1/members/{id}` |  | user | — | other | — | — | self-member | 403 |
| PATCH | `/v1/members/{id}` |  | super_admin_app | — | self | — | — | self-member | 200 |
| PATCH | `/v1/members/{id}` |  | super_admin_app | — | other | — | — | self-member | 403 |
| PATCH | `/v1/members/{id}` |  | agent | read | self | — | — | self-member | 403 |
| PATCH | `/v1/members/{id}` |  | agent | read | other | — | — | self-member | 403 |
| PATCH | `/v1/members/{id}` |  | agent | write | self | — | — | self-member | 200 |
| PATCH | `/v1/members/{id}` |  | agent | write | other | — | — | self-member | 403 |
| PATCH | `/v1/members/{id}` |  | agent | admin | self | — | — | self-member | 200 |
| PATCH | `/v1/members/{id}` |  | agent | admin | other | — | — | self-member | 403 |
| PATCH | `/v1/members/{id}` | authority | user | — | self | — | — | self-device-authority | 200 |
| PATCH | `/v1/members/{id}` | authority | user | — | other | — | — | self-device-authority | 403 |
| PATCH | `/v1/members/{id}` | authority | super_admin_app | — | self | — | — | self-device-authority | 200 |
| PATCH | `/v1/members/{id}` | authority | super_admin_app | — | other | — | — | self-device-authority | 403 |
| PATCH | `/v1/members/{id}` | authority | agent | read | self | — | — | self-device-authority | 403 |
| PATCH | `/v1/members/{id}` | authority | agent | read | other | — | — | self-device-authority | 403 |
| PATCH | `/v1/members/{id}` | authority | agent | write | self | — | — | self-device-authority | 403 |
| PATCH | `/v1/members/{id}` | authority | agent | write | other | — | — | self-device-authority | 403 |
| PATCH | `/v1/members/{id}` | authority | agent | admin | self | — | — | self-device-authority | 403 |
| PATCH | `/v1/members/{id}` | authority | agent | admin | other | — | — | self-device-authority | 403 |
| GET | `/v1/me/settings` |  | user | — | self | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | user | — | other | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | agent | read | self | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | agent | read | other | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | agent | write | self | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | agent | write | other | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | agent | admin | self | — | — | authenticated | 200 |
| GET | `/v1/me/settings` |  | agent | admin | other | — | — | authenticated | 200 |
| PUT | `/v1/me/settings` |  | user | — | self | — | — | authenticated | 200 |
| PUT | `/v1/me/settings` |  | user | — | other | — | — | authenticated | 200 |
| PUT | `/v1/me/settings` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| PUT | `/v1/me/settings` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| PUT | `/v1/me/settings` |  | agent | read | self | — | — | authenticated | 403 |
| PUT | `/v1/me/settings` |  | agent | read | other | — | — | authenticated | 403 |
| PUT | `/v1/me/settings` |  | agent | write | self | — | — | authenticated | 200 |
| PUT | `/v1/me/settings` |  | agent | write | other | — | — | authenticated | 200 |
| PUT | `/v1/me/settings` |  | agent | admin | self | — | — | authenticated | 200 |
| PUT | `/v1/me/settings` |  | agent | admin | other | — | — | authenticated | 200 |
| GET | `/v1/export/member` |  | user | — | self | — | — | Own member ACL; admin PAT can select a member (no bypass) | 200 |
| GET | `/v1/export/member` |  | user | — | other | — | — | Own member ACL; admin PAT can select a member (no bypass) | 403 |
| GET | `/v1/export/member` |  | super_admin_app | — | self | — | — | Own member ACL; admin PAT can select a member (no bypass) | 200 |
| GET | `/v1/export/member` |  | super_admin_app | — | other | — | — | Own member ACL; admin PAT can select a member (no bypass) | 403 |
| GET | `/v1/export/member` |  | agent | read | self | — | — | Own member ACL; admin PAT can select a member (no bypass) | 403 |
| GET | `/v1/export/member` |  | agent | read | other | — | — | Own member ACL; admin PAT can select a member (no bypass) | 403 |
| GET | `/v1/export/member` |  | agent | write | self | — | — | Own member ACL; admin PAT can select a member (no bypass) | 403 |
| GET | `/v1/export/member` |  | agent | write | other | — | — | Own member ACL; admin PAT can select a member (no bypass) | 403 |
| GET | `/v1/export/member` |  | agent | admin | self | — | — | Own member ACL; admin PAT can select a member (no bypass) | 200 |
| GET | `/v1/export/member` |  | agent | admin | other | — | — | Own member ACL; admin PAT can select a member (no bypass) | 200 |
| GET | `/v1/admin/export/household` |  | user | — | self | — | — | Admin PAT or super-admin app plus household.export step-up | 403 |
| GET | `/v1/admin/export/household` |  | user | — | other | — | — | Admin PAT or super-admin app plus household.export step-up | 403 |
| GET | `/v1/admin/export/household` |  | super_admin_app | — | self | — | — | Admin PAT or super-admin app plus household.export step-up | 200 |
| GET | `/v1/admin/export/household` |  | super_admin_app | — | other | — | — | Admin PAT or super-admin app plus household.export step-up | 200 |
| GET | `/v1/admin/export/household` |  | agent | read | self | — | — | Admin PAT or super-admin app plus household.export step-up | 403 |
| GET | `/v1/admin/export/household` |  | agent | read | other | — | — | Admin PAT or super-admin app plus household.export step-up | 403 |
| GET | `/v1/admin/export/household` |  | agent | write | self | — | — | Admin PAT or super-admin app plus household.export step-up | 403 |
| GET | `/v1/admin/export/household` |  | agent | write | other | — | — | Admin PAT or super-admin app plus household.export step-up | 403 |
| GET | `/v1/admin/export/household` |  | agent | admin | self | — | — | Admin PAT or super-admin app plus household.export step-up | 200 |
| GET | `/v1/admin/export/household` |  | agent | admin | other | — | — | Admin PAT or super-admin app plus household.export step-up | 200 |
| GET | `/v1/backups` |  | user | — | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups` |  | user | — | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups` |  | super_admin_app | — | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups` |  | super_admin_app | — | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups` |  | agent | read | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups` |  | agent | read | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups` |  | agent | write | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups` |  | agent | write | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups` |  | agent | admin | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups` |  | agent | admin | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/admin/backups` |  | user | — | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/admin/backups` |  | user | — | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/admin/backups` |  | super_admin_app | — | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/admin/backups` |  | super_admin_app | — | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/admin/backups` |  | agent | read | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/admin/backups` |  | agent | read | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/admin/backups` |  | agent | write | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/admin/backups` |  | agent | write | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/admin/backups` |  | agent | admin | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/admin/backups` |  | agent | admin | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups/latest` |  | user | — | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest` |  | user | — | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest` |  | super_admin_app | — | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups/latest` |  | super_admin_app | — | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups/latest` |  | agent | read | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest` |  | agent | read | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest` |  | agent | write | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest` |  | agent | write | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest` |  | agent | admin | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups/latest` |  | agent | admin | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups/latest/download` |  | user | — | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest/download` |  | user | — | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest/download` |  | super_admin_app | — | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups/latest/download` |  | super_admin_app | — | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups/latest/download` |  | agent | read | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest/download` |  | agent | read | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest/download` |  | agent | write | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest/download` |  | agent | write | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 403 |
| GET | `/v1/backups/latest/download` |  | agent | admin | self | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/backups/latest/download` |  | agent | admin | other | — | — | Super-admin app (mTLS) or admin PAT only on agent status surface | 200 |
| GET | `/v1/client-release` |  | user | — | self | — | — | Enrolled device (mTLS), no session; version minimum exempt | 200 |
| GET | `/v1/client-release` |  | user | — | other | — | — | Enrolled device (mTLS), no session; version minimum exempt | 200 |
| GET | `/v1/client-release` |  | super_admin_app | — | self | — | — | Enrolled device (mTLS), no session; version minimum exempt | 200 |
| GET | `/v1/client-release` |  | super_admin_app | — | other | — | — | Enrolled device (mTLS), no session; version minimum exempt | 200 |
| GET | `/v1/client-release` |  | agent | read | self | — | — | Enrolled device (mTLS), no session; version minimum exempt | 404 |
| GET | `/v1/client-release` |  | agent | read | other | — | — | Enrolled device (mTLS), no session; version minimum exempt | 404 |
| GET | `/v1/client-release` |  | agent | write | self | — | — | Enrolled device (mTLS), no session; version minimum exempt | 404 |
| GET | `/v1/client-release` |  | agent | write | other | — | — | Enrolled device (mTLS), no session; version minimum exempt | 404 |
| GET | `/v1/client-release` |  | agent | admin | self | — | — | Enrolled device (mTLS), no session; version minimum exempt | 404 |
| GET | `/v1/client-release` |  | agent | admin | other | — | — | Enrolled device (mTLS), no session; version minimum exempt | 404 |
| GET | `/v1/client-release/files/{path}` |  | user | — | self | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 200 |
| GET | `/v1/client-release/files/{path}` |  | user | — | other | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 200 |
| GET | `/v1/client-release/files/{path}` |  | super_admin_app | — | self | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 200 |
| GET | `/v1/client-release/files/{path}` |  | super_admin_app | — | other | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 200 |
| GET | `/v1/client-release/files/{path}` |  | agent | read | self | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 404 |
| GET | `/v1/client-release/files/{path}` |  | agent | read | other | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 404 |
| GET | `/v1/client-release/files/{path}` |  | agent | write | self | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 404 |
| GET | `/v1/client-release/files/{path}` |  | agent | write | other | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 404 |
| GET | `/v1/client-release/files/{path}` |  | agent | admin | self | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 404 |
| GET | `/v1/client-release/files/{path}` |  | agent | admin | other | — | — | Enrolled device (mTLS), verified allowlist; version minimum exempt | 404 |
| GET | `/v1/server-release` |  | user | — | self | — | — | Super-admin app (mTLS) only | 403 |
| GET | `/v1/server-release` |  | user | — | other | — | — | Super-admin app (mTLS) only | 403 |
| GET | `/v1/server-release` |  | super_admin_app | — | self | — | — | Super-admin app (mTLS) only | 200 |
| GET | `/v1/server-release` |  | super_admin_app | — | other | — | — | Super-admin app (mTLS) only | 200 |
| GET | `/v1/server-release` |  | agent | read | self | — | — | Super-admin app (mTLS) only | 404 |
| GET | `/v1/server-release` |  | agent | read | other | — | — | Super-admin app (mTLS) only | 404 |
| GET | `/v1/server-release` |  | agent | write | self | — | — | Super-admin app (mTLS) only | 404 |
| GET | `/v1/server-release` |  | agent | write | other | — | — | Super-admin app (mTLS) only | 404 |
| GET | `/v1/server-release` |  | agent | admin | self | — | — | Super-admin app (mTLS) only | 404 |
| GET | `/v1/server-release` |  | agent | admin | other | — | — | Super-admin app (mTLS) only | 404 |
| GET | `/v1/admin/release` |  | user | — | self | — | — | Admin PAT only; agent status surface | 404 |
| GET | `/v1/admin/release` |  | user | — | other | — | — | Admin PAT only; agent status surface | 404 |
| GET | `/v1/admin/release` |  | super_admin_app | — | self | — | — | Admin PAT only; agent status surface | 404 |
| GET | `/v1/admin/release` |  | super_admin_app | — | other | — | — | Admin PAT only; agent status surface | 404 |
| GET | `/v1/admin/release` |  | agent | read | self | — | — | Admin PAT only; agent status surface | 403 |
| GET | `/v1/admin/release` |  | agent | read | other | — | — | Admin PAT only; agent status surface | 403 |
| GET | `/v1/admin/release` |  | agent | write | self | — | — | Admin PAT only; agent status surface | 403 |
| GET | `/v1/admin/release` |  | agent | write | other | — | — | Admin PAT only; agent status surface | 403 |
| GET | `/v1/admin/release` |  | agent | admin | self | — | — | Admin PAT only; agent status surface | 200 |
| GET | `/v1/admin/release` |  | agent | admin | other | — | — | Admin PAT only; agent status surface | 200 |
| POST | `/v1/imports/ics` |  | user | — | self | — | — | calendar_self_or_admin | 200 |
| POST | `/v1/imports/ics` |  | user | — | other | — | — | calendar_self_or_admin | 403 |
| POST | `/v1/imports/ics` |  | super_admin_app | — | self | — | — | calendar_self_or_admin | 200 |
| POST | `/v1/imports/ics` |  | super_admin_app | — | other | — | — | calendar_self_or_admin | 200 |
| POST | `/v1/imports/ics` |  | agent | read | self | — | — | calendar_self_or_admin | 403 |
| POST | `/v1/imports/ics` |  | agent | read | other | — | — | calendar_self_or_admin | 403 |
| POST | `/v1/imports/ics` |  | agent | write | self | — | — | calendar_self_or_admin | 200 |
| POST | `/v1/imports/ics` |  | agent | write | other | — | — | calendar_self_or_admin | 403 |
| POST | `/v1/imports/ics` |  | agent | admin | self | — | — | calendar_self_or_admin | 200 |
| POST | `/v1/imports/ics` |  | agent | admin | other | — | — | calendar_self_or_admin | 200 |
| GET | `/v1/feeds` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/feeds` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/feeds` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 200 |
| GET | `/v1/feeds` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 200 |
| GET | `/v1/feeds` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/feeds` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/feeds` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/feeds` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/feeds` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 200 |
| GET | `/v1/feeds` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 200 |
| POST | `/v1/feeds` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 201 |
| POST | `/v1/feeds` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 201 |
| POST | `/v1/feeds` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 201 |
| POST | `/v1/feeds` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 201 |
| PUT | `/v1/feeds/{id}` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/feeds/{id}` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/feeds/{id}` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 200 |
| PUT | `/v1/feeds/{id}` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 200 |
| PUT | `/v1/feeds/{id}` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/feeds/{id}` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/feeds/{id}` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/feeds/{id}` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/feeds/{id}` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 200 |
| PUT | `/v1/feeds/{id}` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 200 |
| DELETE | `/v1/feeds/{id}` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/feeds/{id}` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/feeds/{id}` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 200 |
| DELETE | `/v1/feeds/{id}` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 200 |
| DELETE | `/v1/feeds/{id}` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/feeds/{id}` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/feeds/{id}` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/feeds/{id}` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/feeds/{id}` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 200 |
| DELETE | `/v1/feeds/{id}` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 200 |
| POST | `/v1/feeds/{id}/refresh` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds/{id}/refresh` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds/{id}/refresh` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 202 |
| POST | `/v1/feeds/{id}/refresh` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 202 |
| POST | `/v1/feeds/{id}/refresh` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds/{id}/refresh` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds/{id}/refresh` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds/{id}/refresh` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/feeds/{id}/refresh` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 202 |
| POST | `/v1/feeds/{id}/refresh` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 202 |
| GET | `/v1/admin/feeds` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/admin/feeds` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/admin/feeds` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 200 |
| GET | `/v1/admin/feeds` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 200 |
| GET | `/v1/admin/feeds` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/admin/feeds` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/admin/feeds` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/admin/feeds` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| GET | `/v1/admin/feeds` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 200 |
| GET | `/v1/admin/feeds` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 200 |
| GET | `/v1/admin/feeds/target` |  | user | — | self | — | — | calendar_self_or_admin | 200 |
| GET | `/v1/admin/feeds/target` |  | user | — | other | — | — | calendar_self_or_admin | 403 |
| GET | `/v1/admin/feeds/target` |  | super_admin_app | — | self | — | — | calendar_self_or_admin | 200 |
| GET | `/v1/admin/feeds/target` |  | super_admin_app | — | other | — | — | calendar_self_or_admin | 200 |
| GET | `/v1/admin/feeds/target` |  | agent | read | self | — | — | calendar_self_or_admin | 200 |
| GET | `/v1/admin/feeds/target` |  | agent | read | other | — | — | calendar_self_or_admin | 403 |
| GET | `/v1/admin/feeds/target` |  | agent | write | self | — | — | calendar_self_or_admin | 200 |
| GET | `/v1/admin/feeds/target` |  | agent | write | other | — | — | calendar_self_or_admin | 403 |
| GET | `/v1/admin/feeds/target` |  | agent | admin | self | — | — | calendar_self_or_admin | 200 |
| GET | `/v1/admin/feeds/target` |  | agent | admin | other | — | — | calendar_self_or_admin | 200 |
| POST | `/v1/admin/feeds` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 201 |
| POST | `/v1/admin/feeds` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 201 |
| POST | `/v1/admin/feeds` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 201 |
| POST | `/v1/admin/feeds` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 201 |
| PUT | `/v1/admin/feeds/{id}` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/admin/feeds/{id}` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/admin/feeds/{id}` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 200 |
| PUT | `/v1/admin/feeds/{id}` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 200 |
| PUT | `/v1/admin/feeds/{id}` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/admin/feeds/{id}` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/admin/feeds/{id}` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/admin/feeds/{id}` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| PUT | `/v1/admin/feeds/{id}` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 200 |
| PUT | `/v1/admin/feeds/{id}` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 200 |
| DELETE | `/v1/admin/feeds/{id}` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/admin/feeds/{id}` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/admin/feeds/{id}` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 200 |
| DELETE | `/v1/admin/feeds/{id}` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 200 |
| DELETE | `/v1/admin/feeds/{id}` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/admin/feeds/{id}` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/admin/feeds/{id}` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/admin/feeds/{id}` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| DELETE | `/v1/admin/feeds/{id}` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 200 |
| DELETE | `/v1/admin/feeds/{id}` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 200 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | user | — | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | user | — | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | super_admin_app | — | self | — | — | super_admin_or_admin_pat | 202 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | super_admin_app | — | other | — | — | super_admin_or_admin_pat | 202 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | agent | read | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | agent | read | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | agent | write | self | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | agent | write | other | — | — | super_admin_or_admin_pat | 403 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | agent | admin | self | — | — | super_admin_or_admin_pat | 202 |
| POST | `/v1/admin/feeds/{id}/refresh` |  | agent | admin | other | — | — | super_admin_or_admin_pat | 202 |
| GET | `/v1/lists` |  | owner | — | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | owner | — | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | assignee | — | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | assignee | — | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | viewer | — | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | viewer | — | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | subject_only | — | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | subject_only | — | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | other_user | — | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | other_user | — | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | super_admin_app | — | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | super_admin_app | — | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | read | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | read | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | write | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | write | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | admin | self | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | admin | other | household | false | authenticated | 200 |
| GET | `/v1/lists` |  | owner | — | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | owner | — | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | assignee | — | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | assignee | — | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | viewer | — | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | viewer | — | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | subject_only | — | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | subject_only | — | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | other_user | — | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | other_user | — | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | super_admin_app | — | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | super_admin_app | — | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | read | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | read | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | write | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | write | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | admin | self | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | admin | other | private | false | authenticated | 200 |
| GET | `/v1/lists` |  | owner | — | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | owner | — | other | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | assignee | — | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | assignee | — | other | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | viewer | — | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | viewer | — | other | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | subject_only | — | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | subject_only | — | other | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | other_user | — | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | other_user | — | other | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | super_admin_app | — | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | super_admin_app | — | other | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | read | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | read | other | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | write | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | write | other | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | admin | self | selected | false | authenticated | 200 |
| GET | `/v1/lists` |  | agent | admin | other | selected | false | authenticated | 200 |
| POST | `/v1/lists` |  | owner | — | self | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | owner | — | other | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | assignee | — | self | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | assignee | — | other | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | viewer | — | self | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | viewer | — | other | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | subject_only | — | self | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | subject_only | — | other | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | other_user | — | self | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | other_user | — | other | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | super_admin_app | — | self | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | super_admin_app | — | other | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | read | self | household | false | authenticated | 403 |
| POST | `/v1/lists` |  | agent | read | other | household | false | authenticated | 403 |
| POST | `/v1/lists` |  | agent | write | self | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | write | other | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | admin | self | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | admin | other | household | false | authenticated | 201 |
| POST | `/v1/lists` |  | owner | — | self | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | owner | — | other | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | assignee | — | self | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | assignee | — | other | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | viewer | — | self | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | viewer | — | other | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | subject_only | — | self | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | subject_only | — | other | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | other_user | — | self | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | other_user | — | other | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | super_admin_app | — | self | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | super_admin_app | — | other | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | read | self | private | false | authenticated | 403 |
| POST | `/v1/lists` |  | agent | read | other | private | false | authenticated | 403 |
| POST | `/v1/lists` |  | agent | write | self | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | write | other | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | admin | self | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | admin | other | private | false | authenticated | 201 |
| POST | `/v1/lists` |  | owner | — | self | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | owner | — | other | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | assignee | — | self | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | assignee | — | other | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | viewer | — | self | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | viewer | — | other | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | subject_only | — | self | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | subject_only | — | other | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | other_user | — | self | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | other_user | — | other | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | super_admin_app | — | self | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | super_admin_app | — | other | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | read | self | selected | false | authenticated | 403 |
| POST | `/v1/lists` |  | agent | read | other | selected | false | authenticated | 403 |
| POST | `/v1/lists` |  | agent | write | self | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | write | other | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | admin | self | selected | false | authenticated | 201 |
| POST | `/v1/lists` |  | agent | admin | other | selected | false | authenticated | 201 |
| POST | `/v1/lists` | attached | owner | — | self | household | false | attached edit | 201 |
| POST | `/v1/lists` | attached | owner | — | other | household | false | attached edit | 201 |
| POST | `/v1/lists` | attached | assignee | — | self | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | assignee | — | other | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | viewer | — | self | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | viewer | — | other | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | subject_only | — | self | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | subject_only | — | other | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | other_user | — | self | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | other_user | — | other | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | super_admin_app | — | self | household | false | attached edit | 201 |
| POST | `/v1/lists` | attached | super_admin_app | — | other | household | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | read | self | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | agent | read | other | household | false | attached edit | 403 |
| POST | `/v1/lists` | attached | agent | write | self | household | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | write | other | household | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | admin | self | household | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | admin | other | household | false | attached edit | 201 |
| POST | `/v1/lists` | attached | owner | — | self | private | false | attached edit | 201 |
| POST | `/v1/lists` | attached | owner | — | other | private | false | attached edit | 201 |
| POST | `/v1/lists` | attached | assignee | — | self | private | false | attached edit | 403 |
| POST | `/v1/lists` | attached | assignee | — | other | private | false | attached edit | 403 |
| POST | `/v1/lists` | attached | viewer | — | self | private | false | attached edit | 422 |
| POST | `/v1/lists` | attached | viewer | — | other | private | false | attached edit | 422 |
| POST | `/v1/lists` | attached | subject_only | — | self | private | false | attached edit | 422 |
| POST | `/v1/lists` | attached | subject_only | — | other | private | false | attached edit | 422 |
| POST | `/v1/lists` | attached | other_user | — | self | private | false | attached edit | 422 |
| POST | `/v1/lists` | attached | other_user | — | other | private | false | attached edit | 422 |
| POST | `/v1/lists` | attached | super_admin_app | — | self | private | false | attached edit | 422 |
| POST | `/v1/lists` | attached | super_admin_app | — | other | private | false | attached edit | 422 |
| POST | `/v1/lists` | attached | agent | read | self | private | false | attached edit | 403 |
| POST | `/v1/lists` | attached | agent | read | other | private | false | attached edit | 403 |
| POST | `/v1/lists` | attached | agent | write | self | private | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | write | other | private | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | admin | self | private | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | admin | other | private | false | attached edit | 201 |
| POST | `/v1/lists` | attached | owner | — | self | selected | false | attached edit | 201 |
| POST | `/v1/lists` | attached | owner | — | other | selected | false | attached edit | 201 |
| POST | `/v1/lists` | attached | assignee | — | self | selected | false | attached edit | 403 |
| POST | `/v1/lists` | attached | assignee | — | other | selected | false | attached edit | 403 |
| POST | `/v1/lists` | attached | viewer | — | self | selected | false | attached edit | 403 |
| POST | `/v1/lists` | attached | viewer | — | other | selected | false | attached edit | 403 |
| POST | `/v1/lists` | attached | subject_only | — | self | selected | false | attached edit | 422 |
| POST | `/v1/lists` | attached | subject_only | — | other | selected | false | attached edit | 422 |
| POST | `/v1/lists` | attached | other_user | — | self | selected | false | attached edit | 422 |
| POST | `/v1/lists` | attached | other_user | — | other | selected | false | attached edit | 422 |
| POST | `/v1/lists` | attached | super_admin_app | — | self | selected | false | attached edit | 422 |
| POST | `/v1/lists` | attached | super_admin_app | — | other | selected | false | attached edit | 422 |
| POST | `/v1/lists` | attached | agent | read | self | selected | false | attached edit | 403 |
| POST | `/v1/lists` | attached | agent | read | other | selected | false | attached edit | 403 |
| POST | `/v1/lists` | attached | agent | write | self | selected | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | write | other | selected | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | admin | self | selected | false | attached edit | 201 |
| POST | `/v1/lists` | attached | agent | admin | other | selected | false | attached edit | 201 |
| GET | `/v1/lists/{id}` |  | owner | — | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | owner | — | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | assignee | — | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | assignee | — | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | viewer | — | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | viewer | — | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | subject_only | — | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | subject_only | — | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | other_user | — | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | other_user | — | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | super_admin_app | — | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | super_admin_app | — | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | read | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | read | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | write | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | write | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | admin | self | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | admin | other | household | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | owner | — | self | private | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | owner | — | other | private | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | assignee | — | self | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | assignee | — | other | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | viewer | — | self | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | viewer | — | other | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | subject_only | — | self | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | subject_only | — | other | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | other_user | — | self | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | other_user | — | other | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | super_admin_app | — | self | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | super_admin_app | — | other | private | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | agent | read | self | private | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | read | other | private | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | write | self | private | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | write | other | private | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | admin | self | private | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | admin | other | private | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | owner | — | self | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | owner | — | other | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | assignee | — | self | selected | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | assignee | — | other | selected | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | viewer | — | self | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | viewer | — | other | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | subject_only | — | self | selected | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | subject_only | — | other | selected | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | other_user | — | self | selected | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | other_user | — | other | selected | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | super_admin_app | — | self | selected | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | super_admin_app | — | other | selected | false | list view | 404 |
| GET | `/v1/lists/{id}` |  | agent | read | self | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | read | other | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | write | self | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | write | other | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | admin | self | selected | false | list view | 200 |
| GET | `/v1/lists/{id}` |  | agent | admin | other | selected | false | list view | 200 |
| PATCH | `/v1/lists/{id}` |  | owner | — | self | household | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | owner | — | other | household | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | assignee | — | self | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | assignee | — | other | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | viewer | — | self | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | viewer | — | other | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | subject_only | — | self | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | subject_only | — | other | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | other_user | — | self | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | other_user | — | other | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | super_admin_app | — | self | household | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | super_admin_app | — | other | household | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | read | self | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | agent | read | other | household | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | agent | write | self | household | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | write | other | household | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | admin | self | household | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | admin | other | household | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | owner | — | self | private | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | owner | — | other | private | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | assignee | — | self | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | assignee | — | other | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | viewer | — | self | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | viewer | — | other | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | subject_only | — | self | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | subject_only | — | other | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | other_user | — | self | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | other_user | — | other | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | super_admin_app | — | self | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | super_admin_app | — | other | private | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | agent | read | self | private | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | agent | read | other | private | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | agent | write | self | private | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | write | other | private | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | admin | self | private | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | admin | other | private | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | owner | — | self | selected | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | owner | — | other | selected | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | assignee | — | self | selected | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | assignee | — | other | selected | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | viewer | — | self | selected | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | viewer | — | other | selected | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | subject_only | — | self | selected | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | subject_only | — | other | selected | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | other_user | — | self | selected | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | other_user | — | other | selected | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | super_admin_app | — | self | selected | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | super_admin_app | — | other | selected | false | list edit | 404 |
| PATCH | `/v1/lists/{id}` |  | agent | read | self | selected | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | agent | read | other | selected | false | list edit | 403 |
| PATCH | `/v1/lists/{id}` |  | agent | write | self | selected | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | write | other | selected | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | admin | self | selected | false | list edit | 200 |
| PATCH | `/v1/lists/{id}` |  | agent | admin | other | selected | false | list edit | 200 |
| DELETE | `/v1/lists/{id}` |  | owner | — | self | household | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | owner | — | other | household | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | assignee | — | self | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | assignee | — | other | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | viewer | — | self | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | viewer | — | other | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | subject_only | — | self | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | subject_only | — | other | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | other_user | — | self | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | other_user | — | other | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | super_admin_app | — | self | household | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | super_admin_app | — | other | household | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | read | self | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | agent | read | other | household | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | agent | write | self | household | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | write | other | household | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | admin | self | household | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | admin | other | household | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | owner | — | self | private | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | owner | — | other | private | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | assignee | — | self | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | assignee | — | other | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | viewer | — | self | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | viewer | — | other | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | subject_only | — | self | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | subject_only | — | other | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | other_user | — | self | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | other_user | — | other | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | super_admin_app | — | self | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | super_admin_app | — | other | private | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | agent | read | self | private | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | agent | read | other | private | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | agent | write | self | private | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | write | other | private | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | admin | self | private | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | admin | other | private | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | owner | — | self | selected | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | owner | — | other | selected | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | assignee | — | self | selected | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | assignee | — | other | selected | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | viewer | — | self | selected | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | viewer | — | other | selected | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | subject_only | — | self | selected | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | subject_only | — | other | selected | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | other_user | — | self | selected | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | other_user | — | other | selected | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | super_admin_app | — | self | selected | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | super_admin_app | — | other | selected | false | list delete | 404 |
| DELETE | `/v1/lists/{id}` |  | agent | read | self | selected | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | agent | read | other | selected | false | list delete | 403 |
| DELETE | `/v1/lists/{id}` |  | agent | write | self | selected | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | write | other | selected | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | admin | self | selected | false | list delete | 200 |
| DELETE | `/v1/lists/{id}` |  | agent | admin | other | selected | false | list delete | 200 |
| POST | `/v1/lists/{id}/entries` |  | owner | — | self | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | owner | — | other | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | assignee | — | self | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | assignee | — | other | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | viewer | — | self | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | viewer | — | other | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | subject_only | — | self | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | subject_only | — | other | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | other_user | — | self | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | other_user | — | other | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | super_admin_app | — | self | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | super_admin_app | — | other | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | read | self | household | false | list manage | 403 |
| POST | `/v1/lists/{id}/entries` |  | agent | read | other | household | false | list manage | 403 |
| POST | `/v1/lists/{id}/entries` |  | agent | write | self | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | write | other | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | admin | self | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | admin | other | household | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | owner | — | self | private | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | owner | — | other | private | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | assignee | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | assignee | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | viewer | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | viewer | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | subject_only | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | subject_only | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | other_user | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | other_user | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | super_admin_app | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | super_admin_app | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | agent | read | self | private | false | list manage | 403 |
| POST | `/v1/lists/{id}/entries` |  | agent | read | other | private | false | list manage | 403 |
| POST | `/v1/lists/{id}/entries` |  | agent | write | self | private | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | write | other | private | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | admin | self | private | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | admin | other | private | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | owner | — | self | selected | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | owner | — | other | selected | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | assignee | — | self | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | assignee | — | other | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | viewer | — | self | selected | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | viewer | — | other | selected | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | subject_only | — | self | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | subject_only | — | other | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | other_user | — | self | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | other_user | — | other | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | super_admin_app | — | self | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | super_admin_app | — | other | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/entries` |  | agent | read | self | selected | false | list manage | 403 |
| POST | `/v1/lists/{id}/entries` |  | agent | read | other | selected | false | list manage | 403 |
| POST | `/v1/lists/{id}/entries` |  | agent | write | self | selected | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | write | other | selected | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | admin | self | selected | false | list manage | 201 |
| POST | `/v1/lists/{id}/entries` |  | agent | admin | other | selected | false | list manage | 201 |
| PATCH | `/v1/list-entries/{id}` |  | owner | — | self | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | owner | — | other | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | assignee | — | self | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | assignee | — | other | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | viewer | — | self | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | viewer | — | other | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | subject_only | — | self | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | subject_only | — | other | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | other_user | — | self | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | other_user | — | other | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | super_admin_app | — | self | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | super_admin_app | — | other | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | read | self | household | false | list manage | 403 |
| PATCH | `/v1/list-entries/{id}` |  | agent | read | other | household | false | list manage | 403 |
| PATCH | `/v1/list-entries/{id}` |  | agent | write | self | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | write | other | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | admin | self | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | admin | other | household | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | owner | — | self | private | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | owner | — | other | private | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | assignee | — | self | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | assignee | — | other | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | viewer | — | self | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | viewer | — | other | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | subject_only | — | self | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | subject_only | — | other | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | other_user | — | self | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | other_user | — | other | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | super_admin_app | — | self | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | super_admin_app | — | other | private | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | agent | read | self | private | false | list manage | 403 |
| PATCH | `/v1/list-entries/{id}` |  | agent | read | other | private | false | list manage | 403 |
| PATCH | `/v1/list-entries/{id}` |  | agent | write | self | private | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | write | other | private | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | admin | self | private | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | admin | other | private | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | owner | — | self | selected | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | owner | — | other | selected | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | assignee | — | self | selected | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | assignee | — | other | selected | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | viewer | — | self | selected | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | viewer | — | other | selected | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | subject_only | — | self | selected | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | subject_only | — | other | selected | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | other_user | — | self | selected | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | other_user | — | other | selected | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | super_admin_app | — | self | selected | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | super_admin_app | — | other | selected | false | list manage | 404 |
| PATCH | `/v1/list-entries/{id}` |  | agent | read | self | selected | false | list manage | 403 |
| PATCH | `/v1/list-entries/{id}` |  | agent | read | other | selected | false | list manage | 403 |
| PATCH | `/v1/list-entries/{id}` |  | agent | write | self | selected | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | write | other | selected | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | admin | self | selected | false | list manage | 200 |
| PATCH | `/v1/list-entries/{id}` |  | agent | admin | other | selected | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | owner | — | self | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | owner | — | other | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | assignee | — | self | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | assignee | — | other | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | viewer | — | self | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | viewer | — | other | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | subject_only | — | self | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | subject_only | — | other | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | other_user | — | self | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | other_user | — | other | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | super_admin_app | — | self | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | super_admin_app | — | other | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | read | self | household | false | list manage | 403 |
| DELETE | `/v1/list-entries/{id}` |  | agent | read | other | household | false | list manage | 403 |
| DELETE | `/v1/list-entries/{id}` |  | agent | write | self | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | write | other | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | admin | self | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | admin | other | household | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | owner | — | self | private | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | owner | — | other | private | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | assignee | — | self | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | assignee | — | other | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | viewer | — | self | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | viewer | — | other | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | subject_only | — | self | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | subject_only | — | other | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | other_user | — | self | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | other_user | — | other | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | super_admin_app | — | self | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | super_admin_app | — | other | private | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | agent | read | self | private | false | list manage | 403 |
| DELETE | `/v1/list-entries/{id}` |  | agent | read | other | private | false | list manage | 403 |
| DELETE | `/v1/list-entries/{id}` |  | agent | write | self | private | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | write | other | private | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | admin | self | private | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | admin | other | private | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | owner | — | self | selected | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | owner | — | other | selected | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | assignee | — | self | selected | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | assignee | — | other | selected | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | viewer | — | self | selected | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | viewer | — | other | selected | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | subject_only | — | self | selected | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | subject_only | — | other | selected | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | other_user | — | self | selected | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | other_user | — | other | selected | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | super_admin_app | — | self | selected | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | super_admin_app | — | other | selected | false | list manage | 404 |
| DELETE | `/v1/list-entries/{id}` |  | agent | read | self | selected | false | list manage | 403 |
| DELETE | `/v1/list-entries/{id}` |  | agent | read | other | selected | false | list manage | 403 |
| DELETE | `/v1/list-entries/{id}` |  | agent | write | self | selected | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | write | other | selected | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | admin | self | selected | false | list manage | 200 |
| DELETE | `/v1/list-entries/{id}` |  | agent | admin | other | selected | false | list manage | 200 |
| POST | `/v1/list-entries/{id}/check` |  | owner | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | owner | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | assignee | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | assignee | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | viewer | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | viewer | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | subject_only | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | subject_only | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | other_user | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | other_user | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | super_admin_app | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | super_admin_app | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | read | self | household | false | list check | 403 |
| POST | `/v1/list-entries/{id}/check` |  | agent | read | other | household | false | list check | 403 |
| POST | `/v1/list-entries/{id}/check` |  | agent | write | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | write | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | admin | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | admin | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | owner | — | self | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | owner | — | other | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | assignee | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | assignee | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | viewer | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | viewer | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | subject_only | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | subject_only | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | other_user | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | other_user | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | super_admin_app | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | super_admin_app | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | agent | read | self | private | false | list check | 403 |
| POST | `/v1/list-entries/{id}/check` |  | agent | read | other | private | false | list check | 403 |
| POST | `/v1/list-entries/{id}/check` |  | agent | write | self | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | write | other | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | admin | self | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | admin | other | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | owner | — | self | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | owner | — | other | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | assignee | — | self | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | assignee | — | other | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | viewer | — | self | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | viewer | — | other | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | subject_only | — | self | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | subject_only | — | other | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | other_user | — | self | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | other_user | — | other | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | super_admin_app | — | self | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | super_admin_app | — | other | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/check` |  | agent | read | self | selected | false | list check | 403 |
| POST | `/v1/list-entries/{id}/check` |  | agent | read | other | selected | false | list check | 403 |
| POST | `/v1/list-entries/{id}/check` |  | agent | write | self | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | write | other | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | admin | self | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/check` |  | agent | admin | other | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | owner | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | owner | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | assignee | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | assignee | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | viewer | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | viewer | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | subject_only | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | subject_only | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | other_user | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | other_user | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | super_admin_app | — | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | super_admin_app | — | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | read | self | household | false | list check | 403 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | read | other | household | false | list check | 403 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | write | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | write | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | admin | self | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | admin | other | household | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | owner | — | self | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | owner | — | other | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | assignee | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | assignee | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | viewer | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | viewer | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | subject_only | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | subject_only | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | other_user | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | other_user | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | super_admin_app | — | self | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | super_admin_app | — | other | private | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | read | self | private | false | list check | 403 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | read | other | private | false | list check | 403 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | write | self | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | write | other | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | admin | self | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | admin | other | private | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | owner | — | self | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | owner | — | other | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | assignee | — | self | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | assignee | — | other | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | viewer | — | self | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | viewer | — | other | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | subject_only | — | self | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | subject_only | — | other | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | other_user | — | self | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | other_user | — | other | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | super_admin_app | — | self | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | super_admin_app | — | other | selected | false | list check | 404 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | read | self | selected | false | list check | 403 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | read | other | selected | false | list check | 403 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | write | self | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | write | other | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | admin | self | selected | false | list check | 200 |
| POST | `/v1/list-entries/{id}/uncheck` |  | agent | admin | other | selected | false | list check | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | owner | — | self | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | owner | — | other | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | assignee | — | self | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | assignee | — | other | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | viewer | — | self | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | viewer | — | other | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | subject_only | — | self | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | subject_only | — | other | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | other_user | — | self | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | other_user | — | other | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | super_admin_app | — | self | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | super_admin_app | — | other | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | read | self | household | false | list manage | 403 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | read | other | household | false | list manage | 403 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | write | self | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | write | other | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | admin | self | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | admin | other | household | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | owner | — | self | private | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | owner | — | other | private | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | assignee | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | assignee | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | viewer | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | viewer | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | subject_only | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | subject_only | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | other_user | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | other_user | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | super_admin_app | — | self | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | super_admin_app | — | other | private | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | read | self | private | false | list manage | 403 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | read | other | private | false | list manage | 403 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | write | self | private | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | write | other | private | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | admin | self | private | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | admin | other | private | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | owner | — | self | selected | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | owner | — | other | selected | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | assignee | — | self | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | assignee | — | other | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | viewer | — | self | selected | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | viewer | — | other | selected | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | subject_only | — | self | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | subject_only | — | other | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | other_user | — | self | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | other_user | — | other | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | super_admin_app | — | self | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | super_admin_app | — | other | selected | false | list manage | 404 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | read | self | selected | false | list manage | 403 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | read | other | selected | false | list manage | 403 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | write | self | selected | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | write | other | selected | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | admin | self | selected | false | list manage | 200 |
| POST | `/v1/lists/{id}/clear-checked` |  | agent | admin | other | selected | false | list manage | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | owner | — | self | household | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | owner | — | other | household | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | assignee | — | self | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | assignee | — | other | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | viewer | — | self | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | viewer | — | other | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | subject_only | — | self | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | subject_only | — | other | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | other_user | — | self | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | other_user | — | other | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | super_admin_app | — | self | household | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | super_admin_app | — | other | household | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | read | self | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | read | other | household | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | write | self | household | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | write | other | household | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | admin | self | household | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | admin | other | household | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | owner | — | self | private | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | owner | — | other | private | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | assignee | — | self | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | assignee | — | other | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | viewer | — | self | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | viewer | — | other | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | subject_only | — | self | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | subject_only | — | other | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | other_user | — | self | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | other_user | — | other | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | super_admin_app | — | self | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | super_admin_app | — | other | private | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | read | self | private | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | read | other | private | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | write | self | private | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | write | other | private | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | admin | self | private | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | admin | other | private | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | owner | — | self | selected | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | owner | — | other | selected | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | assignee | — | self | selected | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | assignee | — | other | selected | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | viewer | — | self | selected | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | viewer | — | other | selected | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | subject_only | — | self | selected | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | subject_only | — | other | selected | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | other_user | — | self | selected | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | other_user | — | other | selected | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | super_admin_app | — | self | selected | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | super_admin_app | — | other | selected | false | list change_visibility | 404 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | read | self | selected | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | read | other | selected | false | list change_visibility | 403 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | write | self | selected | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | write | other | selected | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | admin | self | selected | false | list change_visibility | 200 |
| PUT | `/v1/lists/{id}/viewers` |  | agent | admin | other | selected | false | list change_visibility | 200 |
| GET | `/v1/lists/{id}` | attached | owner | — | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | owner | — | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | assignee | — | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | assignee | — | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | viewer | — | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | viewer | — | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | subject_only | — | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | subject_only | — | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | other_user | — | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | other_user | — | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | super_admin_app | — | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | super_admin_app | — | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | read | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | read | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | write | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | write | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | admin | self | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | admin | other | household | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | owner | — | self | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | owner | — | other | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | assignee | — | self | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | assignee | — | other | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | viewer | — | self | private | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | viewer | — | other | private | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | subject_only | — | self | private | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | subject_only | — | other | private | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | other_user | — | self | private | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | other_user | — | other | private | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | super_admin_app | — | self | private | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | super_admin_app | — | other | private | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | agent | read | self | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | read | other | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | write | self | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | write | other | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | admin | self | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | admin | other | private | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | owner | — | self | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | owner | — | other | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | assignee | — | self | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | assignee | — | other | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | viewer | — | self | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | viewer | — | other | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | subject_only | — | self | selected | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | subject_only | — | other | selected | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | other_user | — | self | selected | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | other_user | — | other | selected | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | super_admin_app | — | self | selected | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | super_admin_app | — | other | selected | false | view | 404 |
| GET | `/v1/lists/{id}` | attached | agent | read | self | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | read | other | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | write | self | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | write | other | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | admin | self | selected | false | view | 200 |
| GET | `/v1/lists/{id}` | attached | agent | admin | other | selected | false | view | 200 |
| PATCH | `/v1/lists/{id}` | attached | owner | — | self | household | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | owner | — | other | household | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | assignee | — | self | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | assignee | — | other | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | viewer | — | self | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | viewer | — | other | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | subject_only | — | self | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | subject_only | — | other | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | other_user | — | self | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | other_user | — | other | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | super_admin_app | — | self | household | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | super_admin_app | — | other | household | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | read | self | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | agent | read | other | household | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | agent | write | self | household | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | write | other | household | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | admin | self | household | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | admin | other | household | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | owner | — | self | private | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | owner | — | other | private | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | assignee | — | self | private | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | assignee | — | other | private | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | viewer | — | self | private | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | viewer | — | other | private | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | subject_only | — | self | private | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | subject_only | — | other | private | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | other_user | — | self | private | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | other_user | — | other | private | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | super_admin_app | — | self | private | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | super_admin_app | — | other | private | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | agent | read | self | private | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | agent | read | other | private | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | agent | write | self | private | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | write | other | private | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | admin | self | private | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | admin | other | private | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | owner | — | self | selected | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | owner | — | other | selected | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | assignee | — | self | selected | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | assignee | — | other | selected | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | viewer | — | self | selected | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | viewer | — | other | selected | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | subject_only | — | self | selected | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | subject_only | — | other | selected | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | other_user | — | self | selected | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | other_user | — | other | selected | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | super_admin_app | — | self | selected | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | super_admin_app | — | other | selected | false | edit | 404 |
| PATCH | `/v1/lists/{id}` | attached | agent | read | self | selected | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | agent | read | other | selected | false | edit | 403 |
| PATCH | `/v1/lists/{id}` | attached | agent | write | self | selected | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | write | other | selected | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | admin | self | selected | false | edit | 200 |
| PATCH | `/v1/lists/{id}` | attached | agent | admin | other | selected | false | edit | 200 |
| DELETE | `/v1/lists/{id}` | attached | owner | — | self | household | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | owner | — | other | household | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | assignee | — | self | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | assignee | — | other | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | viewer | — | self | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | viewer | — | other | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | subject_only | — | self | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | subject_only | — | other | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | other_user | — | self | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | other_user | — | other | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | super_admin_app | — | self | household | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | super_admin_app | — | other | household | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | read | self | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | agent | read | other | household | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | agent | write | self | household | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | write | other | household | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | admin | self | household | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | admin | other | household | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | owner | — | self | private | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | owner | — | other | private | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | assignee | — | self | private | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | assignee | — | other | private | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | viewer | — | self | private | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | viewer | — | other | private | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | subject_only | — | self | private | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | subject_only | — | other | private | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | other_user | — | self | private | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | other_user | — | other | private | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | super_admin_app | — | self | private | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | super_admin_app | — | other | private | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | agent | read | self | private | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | agent | read | other | private | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | agent | write | self | private | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | write | other | private | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | admin | self | private | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | admin | other | private | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | owner | — | self | selected | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | owner | — | other | selected | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | assignee | — | self | selected | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | assignee | — | other | selected | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | viewer | — | self | selected | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | viewer | — | other | selected | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | subject_only | — | self | selected | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | subject_only | — | other | selected | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | other_user | — | self | selected | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | other_user | — | other | selected | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | super_admin_app | — | self | selected | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | super_admin_app | — | other | selected | false | delete | 404 |
| DELETE | `/v1/lists/{id}` | attached | agent | read | self | selected | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | agent | read | other | selected | false | delete | 403 |
| DELETE | `/v1/lists/{id}` | attached | agent | write | self | selected | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | write | other | selected | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | admin | self | selected | false | delete | 200 |
| DELETE | `/v1/lists/{id}` | attached | agent | admin | other | selected | false | delete | 200 |
| POST | `/v1/lists/{id}/entries` | attached | owner | — | self | household | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | owner | — | other | household | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | assignee | — | self | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | assignee | — | other | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | viewer | — | self | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | viewer | — | other | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | subject_only | — | self | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | subject_only | — | other | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | other_user | — | self | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | other_user | — | other | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | super_admin_app | — | self | household | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | super_admin_app | — | other | household | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | read | self | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | agent | read | other | household | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | agent | write | self | household | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | write | other | household | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | admin | self | household | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | admin | other | household | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | owner | — | self | private | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | owner | — | other | private | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | assignee | — | self | private | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | assignee | — | other | private | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | viewer | — | self | private | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | viewer | — | other | private | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | subject_only | — | self | private | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | subject_only | — | other | private | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | other_user | — | self | private | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | other_user | — | other | private | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | super_admin_app | — | self | private | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | super_admin_app | — | other | private | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | agent | read | self | private | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | agent | read | other | private | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | agent | write | self | private | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | write | other | private | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | admin | self | private | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | admin | other | private | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | owner | — | self | selected | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | owner | — | other | selected | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | assignee | — | self | selected | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | assignee | — | other | selected | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | viewer | — | self | selected | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | viewer | — | other | selected | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | subject_only | — | self | selected | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | subject_only | — | other | selected | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | other_user | — | self | selected | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | other_user | — | other | selected | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | super_admin_app | — | self | selected | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | super_admin_app | — | other | selected | false | attached edit | 404 |
| POST | `/v1/lists/{id}/entries` | attached | agent | read | self | selected | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | agent | read | other | selected | false | attached edit | 403 |
| POST | `/v1/lists/{id}/entries` | attached | agent | write | self | selected | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | write | other | selected | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | admin | self | selected | false | attached edit | 201 |
| POST | `/v1/lists/{id}/entries` | attached | agent | admin | other | selected | false | attached edit | 201 |
| PATCH | `/v1/list-entries/{id}` | attached | owner | — | self | household | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | owner | — | other | household | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | assignee | — | self | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | assignee | — | other | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | viewer | — | self | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | viewer | — | other | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | subject_only | — | self | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | subject_only | — | other | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | other_user | — | self | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | other_user | — | other | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | super_admin_app | — | self | household | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | super_admin_app | — | other | household | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | read | self | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | read | other | household | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | write | self | household | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | write | other | household | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | admin | self | household | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | admin | other | household | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | owner | — | self | private | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | owner | — | other | private | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | assignee | — | self | private | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | assignee | — | other | private | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | viewer | — | self | private | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | viewer | — | other | private | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | subject_only | — | self | private | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | subject_only | — | other | private | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | other_user | — | self | private | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | other_user | — | other | private | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | super_admin_app | — | self | private | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | super_admin_app | — | other | private | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | read | self | private | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | read | other | private | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | write | self | private | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | write | other | private | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | admin | self | private | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | admin | other | private | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | owner | — | self | selected | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | owner | — | other | selected | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | assignee | — | self | selected | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | assignee | — | other | selected | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | viewer | — | self | selected | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | viewer | — | other | selected | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | subject_only | — | self | selected | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | subject_only | — | other | selected | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | other_user | — | self | selected | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | other_user | — | other | selected | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | super_admin_app | — | self | selected | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | super_admin_app | — | other | selected | false | edit | 404 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | read | self | selected | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | read | other | selected | false | edit | 403 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | write | self | selected | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | write | other | selected | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | admin | self | selected | false | edit | 200 |
| PATCH | `/v1/list-entries/{id}` | attached | agent | admin | other | selected | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | owner | — | self | household | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | owner | — | other | household | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | assignee | — | self | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | assignee | — | other | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | viewer | — | self | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | viewer | — | other | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | subject_only | — | self | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | subject_only | — | other | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | other_user | — | self | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | other_user | — | other | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | super_admin_app | — | self | household | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | super_admin_app | — | other | household | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | read | self | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | read | other | household | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | write | self | household | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | write | other | household | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | admin | self | household | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | admin | other | household | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | owner | — | self | private | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | owner | — | other | private | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | assignee | — | self | private | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | assignee | — | other | private | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | viewer | — | self | private | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | viewer | — | other | private | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | subject_only | — | self | private | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | subject_only | — | other | private | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | other_user | — | self | private | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | other_user | — | other | private | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | super_admin_app | — | self | private | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | super_admin_app | — | other | private | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | read | self | private | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | read | other | private | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | write | self | private | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | write | other | private | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | admin | self | private | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | admin | other | private | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | owner | — | self | selected | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | owner | — | other | selected | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | assignee | — | self | selected | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | assignee | — | other | selected | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | viewer | — | self | selected | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | viewer | — | other | selected | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | subject_only | — | self | selected | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | subject_only | — | other | selected | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | other_user | — | self | selected | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | other_user | — | other | selected | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | super_admin_app | — | self | selected | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | super_admin_app | — | other | selected | false | edit | 404 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | read | self | selected | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | read | other | selected | false | edit | 403 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | write | self | selected | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | write | other | selected | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | admin | self | selected | false | edit | 200 |
| DELETE | `/v1/list-entries/{id}` | attached | agent | admin | other | selected | false | edit | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | owner | — | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | owner | — | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | assignee | — | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | assignee | — | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | viewer | — | self | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | viewer | — | other | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | subject_only | — | self | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | subject_only | — | other | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | other_user | — | self | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | other_user | — | other | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | super_admin_app | — | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | super_admin_app | — | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | read | self | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | read | other | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | write | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | write | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | admin | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | admin | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | owner | — | self | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | owner | — | other | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | assignee | — | self | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | assignee | — | other | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | viewer | — | self | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | viewer | — | other | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | subject_only | — | self | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | subject_only | — | other | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | other_user | — | self | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | other_user | — | other | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | super_admin_app | — | self | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | super_admin_app | — | other | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | read | self | private | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | read | other | private | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | write | self | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | write | other | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | admin | self | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | admin | other | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | owner | — | self | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | owner | — | other | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | assignee | — | self | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | assignee | — | other | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | viewer | — | self | selected | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | viewer | — | other | selected | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | subject_only | — | self | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | subject_only | — | other | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | other_user | — | self | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | other_user | — | other | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | super_admin_app | — | self | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | super_admin_app | — | other | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | read | self | selected | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | read | other | selected | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | write | self | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | write | other | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | admin | self | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/check` | attached | agent | admin | other | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | owner | — | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | owner | — | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | assignee | — | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | assignee | — | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | viewer | — | self | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | viewer | — | other | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | subject_only | — | self | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | subject_only | — | other | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | other_user | — | self | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | other_user | — | other | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | super_admin_app | — | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | super_admin_app | — | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | read | self | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | read | other | household | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | write | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | write | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | admin | self | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | admin | other | household | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | owner | — | self | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | owner | — | other | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | assignee | — | self | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | assignee | — | other | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | viewer | — | self | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | viewer | — | other | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | subject_only | — | self | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | subject_only | — | other | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | other_user | — | self | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | other_user | — | other | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | super_admin_app | — | self | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | super_admin_app | — | other | private | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | read | self | private | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | read | other | private | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | write | self | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | write | other | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | admin | self | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | admin | other | private | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | owner | — | self | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | owner | — | other | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | assignee | — | self | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | assignee | — | other | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | viewer | — | self | selected | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | viewer | — | other | selected | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | subject_only | — | self | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | subject_only | — | other | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | other_user | — | self | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | other_user | — | other | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | super_admin_app | — | self | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | super_admin_app | — | other | selected | false | change_status | 404 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | read | self | selected | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | read | other | selected | false | change_status | 403 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | write | self | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | write | other | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | admin | self | selected | false | change_status | 200 |
| POST | `/v1/list-entries/{id}/uncheck` | attached | agent | admin | other | selected | false | change_status | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | owner | — | self | household | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | owner | — | other | household | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | assignee | — | self | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | assignee | — | other | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | viewer | — | self | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | viewer | — | other | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | subject_only | — | self | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | subject_only | — | other | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | other_user | — | self | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | other_user | — | other | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | super_admin_app | — | self | household | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | super_admin_app | — | other | household | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | read | self | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | read | other | household | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | write | self | household | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | write | other | household | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | admin | self | household | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | admin | other | household | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | owner | — | self | private | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | owner | — | other | private | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | assignee | — | self | private | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | assignee | — | other | private | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | viewer | — | self | private | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | viewer | — | other | private | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | subject_only | — | self | private | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | subject_only | — | other | private | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | other_user | — | self | private | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | other_user | — | other | private | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | super_admin_app | — | self | private | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | super_admin_app | — | other | private | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | read | self | private | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | read | other | private | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | write | self | private | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | write | other | private | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | admin | self | private | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | admin | other | private | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | owner | — | self | selected | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | owner | — | other | selected | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | assignee | — | self | selected | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | assignee | — | other | selected | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | viewer | — | self | selected | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | viewer | — | other | selected | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | subject_only | — | self | selected | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | subject_only | — | other | selected | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | other_user | — | self | selected | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | other_user | — | other | selected | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | super_admin_app | — | self | selected | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | super_admin_app | — | other | selected | false | edit | 404 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | read | self | selected | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | read | other | selected | false | edit | 403 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | write | self | selected | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | write | other | selected | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | admin | self | selected | false | edit | 200 |
| POST | `/v1/lists/{id}/clear-checked` | attached | agent | admin | other | selected | false | edit | 200 |
| PUT | `/v1/lists/{id}/viewers` | attached | owner | — | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | owner | — | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | assignee | — | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | assignee | — | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | viewer | — | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | viewer | — | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | subject_only | — | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | subject_only | — | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | other_user | — | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | other_user | — | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | super_admin_app | — | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | super_admin_app | — | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | read | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | read | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | write | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | write | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | admin | self | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | admin | other | household | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | owner | — | self | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | owner | — | other | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | assignee | — | self | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | assignee | — | other | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | viewer | — | self | private | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | viewer | — | other | private | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | subject_only | — | self | private | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | subject_only | — | other | private | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | other_user | — | self | private | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | other_user | — | other | private | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | super_admin_app | — | self | private | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | super_admin_app | — | other | private | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | read | self | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | read | other | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | write | self | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | write | other | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | admin | self | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | admin | other | private | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | owner | — | self | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | owner | — | other | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | assignee | — | self | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | assignee | — | other | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | viewer | — | self | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | viewer | — | other | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | subject_only | — | self | selected | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | subject_only | — | other | selected | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | other_user | — | self | selected | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | other_user | — | other | selected | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | super_admin_app | — | self | selected | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | super_admin_app | — | other | selected | false | inherits item viewers | 404 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | read | self | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | read | other | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | write | self | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | write | other | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | admin | self | selected | false | inherits item viewers | 403 |
| PUT | `/v1/lists/{id}/viewers` | attached | agent | admin | other | selected | false | inherits item viewers | 403 |
| GET | `/v1/admin/users` |  | user | — | self | — | — | admin PAT | 404 |
| GET | `/v1/admin/users` |  | user | — | other | — | — | admin PAT | 404 |
| GET | `/v1/admin/users` |  | super_admin_app | — | self | — | — | admin PAT | 404 |
| GET | `/v1/admin/users` |  | super_admin_app | — | other | — | — | admin PAT | 404 |
| GET | `/v1/admin/users` |  | agent | read | self | — | — | admin PAT | 403 |
| GET | `/v1/admin/users` |  | agent | read | other | — | — | admin PAT | 403 |
| GET | `/v1/admin/users` |  | agent | write | self | — | — | admin PAT | 403 |
| GET | `/v1/admin/users` |  | agent | write | other | — | — | admin PAT | 403 |
| GET | `/v1/admin/users` |  | agent | admin | self | — | — | admin PAT | 200 |
| GET | `/v1/admin/users` |  | agent | admin | other | — | — | admin PAT | 200 |
| POST | `/v1/admin/users` |  | user | — | self | — | — | admin PAT | 404 |
| POST | `/v1/admin/users` |  | user | — | other | — | — | admin PAT | 404 |
| POST | `/v1/admin/users` |  | super_admin_app | — | self | — | — | admin PAT | 404 |
| POST | `/v1/admin/users` |  | super_admin_app | — | other | — | — | admin PAT | 404 |
| POST | `/v1/admin/users` |  | agent | read | self | — | — | admin PAT | 403 |
| POST | `/v1/admin/users` |  | agent | read | other | — | — | admin PAT | 403 |
| POST | `/v1/admin/users` |  | agent | write | self | — | — | admin PAT | 403 |
| POST | `/v1/admin/users` |  | agent | write | other | — | — | admin PAT | 403 |
| POST | `/v1/admin/users` |  | agent | admin | self | — | — | admin PAT | 201 |
| POST | `/v1/admin/users` |  | agent | admin | other | — | — | admin PAT | 201 |
| PATCH | `/v1/admin/users/{id}` |  | user | — | self | — | — | admin PAT | 404 |
| PATCH | `/v1/admin/users/{id}` |  | user | — | other | — | — | admin PAT | 404 |
| PATCH | `/v1/admin/users/{id}` |  | super_admin_app | — | self | — | — | admin PAT | 404 |
| PATCH | `/v1/admin/users/{id}` |  | super_admin_app | — | other | — | — | admin PAT | 404 |
| PATCH | `/v1/admin/users/{id}` |  | agent | read | self | — | — | admin PAT | 403 |
| PATCH | `/v1/admin/users/{id}` |  | agent | read | other | — | — | admin PAT | 403 |
| PATCH | `/v1/admin/users/{id}` |  | agent | write | self | — | — | admin PAT | 403 |
| PATCH | `/v1/admin/users/{id}` |  | agent | write | other | — | — | admin PAT | 403 |
| PATCH | `/v1/admin/users/{id}` |  | agent | admin | self | — | — | admin PAT | 200 |
| PATCH | `/v1/admin/users/{id}` |  | agent | admin | other | — | — | admin PAT | 200 |
| DELETE | `/v1/admin/users/{id}` |  | user | — | self | — | — | admin PAT | 404 |
| DELETE | `/v1/admin/users/{id}` |  | user | — | other | — | — | admin PAT | 404 |
| DELETE | `/v1/admin/users/{id}` |  | super_admin_app | — | self | — | — | admin PAT | 404 |
| DELETE | `/v1/admin/users/{id}` |  | super_admin_app | — | other | — | — | admin PAT | 404 |
| DELETE | `/v1/admin/users/{id}` |  | agent | read | self | — | — | admin PAT | 403 |
| DELETE | `/v1/admin/users/{id}` |  | agent | read | other | — | — | admin PAT | 403 |
| DELETE | `/v1/admin/users/{id}` |  | agent | write | self | — | — | admin PAT | 403 |
| DELETE | `/v1/admin/users/{id}` |  | agent | write | other | — | — | admin PAT | 403 |
| DELETE | `/v1/admin/users/{id}` |  | agent | admin | self | — | — | admin PAT | 200 |
| DELETE | `/v1/admin/users/{id}` |  | agent | admin | other | — | — | admin PAT | 200 |
| POST | `/v1/admin/items/{id}/revert` |  | user | — | self | — | — | write PAT | 404 |
| POST | `/v1/admin/items/{id}/revert` |  | user | — | other | — | — | write PAT | 404 |
| POST | `/v1/admin/items/{id}/revert` |  | super_admin_app | — | self | — | — | write PAT | 404 |
| POST | `/v1/admin/items/{id}/revert` |  | super_admin_app | — | other | — | — | write PAT | 404 |
| POST | `/v1/admin/items/{id}/revert` |  | agent | read | self | — | — | write PAT | 403 |
| POST | `/v1/admin/items/{id}/revert` |  | agent | read | other | — | — | write PAT | 403 |
| POST | `/v1/admin/items/{id}/revert` |  | agent | write | self | — | — | write PAT | 200 |
| POST | `/v1/admin/items/{id}/revert` |  | agent | write | other | — | — | write PAT | 200 |
| POST | `/v1/admin/items/{id}/revert` |  | agent | admin | self | — | — | write PAT | 200 |
| POST | `/v1/admin/items/{id}/revert` |  | agent | admin | other | — | — | write PAT | 200 |
| GET | `/v1/admin/items/{id}/revisions` |  | user | — | self | — | — | read PAT | 404 |
| GET | `/v1/admin/items/{id}/revisions` |  | user | — | other | — | — | read PAT | 404 |
| GET | `/v1/admin/items/{id}/revisions` |  | super_admin_app | — | self | — | — | read PAT | 404 |
| GET | `/v1/admin/items/{id}/revisions` |  | super_admin_app | — | other | — | — | read PAT | 404 |
| GET | `/v1/admin/items/{id}/revisions` |  | agent | read | self | — | — | read PAT | 200 |
| GET | `/v1/admin/items/{id}/revisions` |  | agent | read | other | — | — | read PAT | 200 |
| GET | `/v1/admin/items/{id}/revisions` |  | agent | write | self | — | — | read PAT | 200 |
| GET | `/v1/admin/items/{id}/revisions` |  | agent | write | other | — | — | read PAT | 200 |
| GET | `/v1/admin/items/{id}/revisions` |  | agent | admin | self | — | — | read PAT | 200 |
| GET | `/v1/admin/items/{id}/revisions` |  | agent | admin | other | — | — | read PAT | 200 |
| POST | `/v1/admin/purge` |  | user | — | self | — | — | admin PAT | 404 |
| POST | `/v1/admin/purge` |  | user | — | other | — | — | admin PAT | 404 |
| POST | `/v1/admin/purge` |  | super_admin_app | — | self | — | — | admin PAT | 404 |
| POST | `/v1/admin/purge` |  | super_admin_app | — | other | — | — | admin PAT | 404 |
| POST | `/v1/admin/purge` |  | agent | read | self | — | — | admin PAT | 403 |
| POST | `/v1/admin/purge` |  | agent | read | other | — | — | admin PAT | 403 |
| POST | `/v1/admin/purge` |  | agent | write | self | — | — | admin PAT | 403 |
| POST | `/v1/admin/purge` |  | agent | write | other | — | — | admin PAT | 403 |
| POST | `/v1/admin/purge` |  | agent | admin | self | — | — | admin PAT | 200 |
| POST | `/v1/admin/purge` |  | agent | admin | other | — | — | admin PAT | 200 |
| GET | `/v1/admin/derive` |  | user | — | self | — | — | read PAT | 404 |
| GET | `/v1/admin/derive` |  | user | — | other | — | — | read PAT | 404 |
| GET | `/v1/admin/derive` |  | super_admin_app | — | self | — | — | read PAT | 404 |
| GET | `/v1/admin/derive` |  | super_admin_app | — | other | — | — | read PAT | 404 |
| GET | `/v1/admin/derive` |  | agent | read | self | — | — | read PAT | 200 |
| GET | `/v1/admin/derive` |  | agent | read | other | — | — | read PAT | 200 |
| GET | `/v1/admin/derive` |  | agent | write | self | — | — | read PAT | 200 |
| GET | `/v1/admin/derive` |  | agent | write | other | — | — | read PAT | 200 |
| GET | `/v1/admin/derive` |  | agent | admin | self | — | — | read PAT | 200 |
| GET | `/v1/admin/derive` |  | agent | admin | other | — | — | read PAT | 200 |
| POST | `/v1/admin/derive/{id}/retry` |  | user | — | self | — | — | write PAT | 404 |
| POST | `/v1/admin/derive/{id}/retry` |  | user | — | other | — | — | write PAT | 404 |
| POST | `/v1/admin/derive/{id}/retry` |  | super_admin_app | — | self | — | — | write PAT | 404 |
| POST | `/v1/admin/derive/{id}/retry` |  | super_admin_app | — | other | — | — | write PAT | 404 |
| POST | `/v1/admin/derive/{id}/retry` |  | agent | read | self | — | — | write PAT | 403 |
| POST | `/v1/admin/derive/{id}/retry` |  | agent | read | other | — | — | write PAT | 403 |
| POST | `/v1/admin/derive/{id}/retry` |  | agent | write | self | — | — | write PAT | 200 |
| POST | `/v1/admin/derive/{id}/retry` |  | agent | write | other | — | — | write PAT | 200 |
| POST | `/v1/admin/derive/{id}/retry` |  | agent | admin | self | — | — | write PAT | 200 |
| POST | `/v1/admin/derive/{id}/retry` |  | agent | admin | other | — | — | write PAT | 200 |
| GET | `/v1/notifications` |  | user | — | self | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | user | — | other | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | agent | read | self | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | agent | read | other | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | agent | write | self | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | agent | write | other | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | agent | admin | self | — | — | authenticated | 200 |
| GET | `/v1/notifications` |  | agent | admin | other | — | — | authenticated | 200 |
| POST | `/v1/notifications/{id}/read` |  | user | — | self | — | — | recipient-private | 200 |
| POST | `/v1/notifications/{id}/read` |  | user | — | other | — | — | recipient-private | 404 |
| POST | `/v1/notifications/{id}/read` |  | super_admin_app | — | self | — | — | recipient-private | 200 |
| POST | `/v1/notifications/{id}/read` |  | super_admin_app | — | other | — | — | recipient-private | 404 |
| POST | `/v1/notifications/{id}/read` |  | agent | read | self | — | — | recipient-private | 403 |
| POST | `/v1/notifications/{id}/read` |  | agent | read | other | — | — | recipient-private | 403 |
| POST | `/v1/notifications/{id}/read` |  | agent | write | self | — | — | recipient-private | 200 |
| POST | `/v1/notifications/{id}/read` |  | agent | write | other | — | — | recipient-private | 404 |
| POST | `/v1/notifications/{id}/read` |  | agent | admin | self | — | — | recipient-private | 200 |
| POST | `/v1/notifications/{id}/read` |  | agent | admin | other | — | — | recipient-private | 404 |
| POST | `/v1/notifications/read-all` |  | user | — | self | — | — | authenticated | 200 |
| POST | `/v1/notifications/read-all` |  | user | — | other | — | — | authenticated | 200 |
| POST | `/v1/notifications/read-all` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| POST | `/v1/notifications/read-all` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| POST | `/v1/notifications/read-all` |  | agent | read | self | — | — | authenticated | 403 |
| POST | `/v1/notifications/read-all` |  | agent | read | other | — | — | authenticated | 403 |
| POST | `/v1/notifications/read-all` |  | agent | write | self | — | — | authenticated | 200 |
| POST | `/v1/notifications/read-all` |  | agent | write | other | — | — | authenticated | 200 |
| POST | `/v1/notifications/read-all` |  | agent | admin | self | — | — | authenticated | 200 |
| POST | `/v1/notifications/read-all` |  | agent | admin | other | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | user | — | self | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | user | — | other | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | agent | read | self | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | agent | read | other | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | agent | write | self | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | agent | write | other | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | agent | admin | self | — | — | authenticated | 200 |
| GET | `/v1/me/notification-prefs` |  | agent | admin | other | — | — | authenticated | 200 |
| PUT | `/v1/me/notification-prefs` |  | user | — | self | — | — | authenticated | 200 |
| PUT | `/v1/me/notification-prefs` |  | user | — | other | — | — | authenticated | 200 |
| PUT | `/v1/me/notification-prefs` |  | super_admin_app | — | self | — | — | authenticated | 200 |
| PUT | `/v1/me/notification-prefs` |  | super_admin_app | — | other | — | — | authenticated | 200 |
| PUT | `/v1/me/notification-prefs` |  | agent | read | self | — | — | authenticated | 403 |
| PUT | `/v1/me/notification-prefs` |  | agent | read | other | — | — | authenticated | 403 |
| PUT | `/v1/me/notification-prefs` |  | agent | write | self | — | — | authenticated | 200 |
| PUT | `/v1/me/notification-prefs` |  | agent | write | other | — | — | authenticated | 200 |
| PUT | `/v1/me/notification-prefs` |  | agent | admin | self | — | — | authenticated | 200 |
| PUT | `/v1/me/notification-prefs` |  | agent | admin | other | — | — | authenticated | 200 |
| GET | `/v1/admin/notifications` |  | user | — | self | — | — | admin PAT inspection (agent-only surface) | 404 |
| GET | `/v1/admin/notifications` |  | user | — | other | — | — | admin PAT inspection (agent-only surface) | 404 |
| GET | `/v1/admin/notifications` |  | super_admin_app | — | self | — | — | admin PAT inspection (agent-only surface) | 404 |
| GET | `/v1/admin/notifications` |  | super_admin_app | — | other | — | — | admin PAT inspection (agent-only surface) | 404 |
| GET | `/v1/admin/notifications` |  | agent | read | self | — | — | admin PAT inspection (agent-only surface) | 403 |
| GET | `/v1/admin/notifications` |  | agent | read | other | — | — | admin PAT inspection (agent-only surface) | 403 |
| GET | `/v1/admin/notifications` |  | agent | write | self | — | — | admin PAT inspection (agent-only surface) | 403 |
| GET | `/v1/admin/notifications` |  | agent | write | other | — | — | admin PAT inspection (agent-only surface) | 403 |
| GET | `/v1/admin/notifications` |  | agent | admin | self | — | — | admin PAT inspection (agent-only surface) | 200 |
| GET | `/v1/admin/notifications` |  | agent | admin | other | — | — | admin PAT inspection (agent-only surface) | 200 |
| GET | `/v1/items/{id}/comments` |  | owner | — | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | owner | — | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | assignee | — | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | assignee | — | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | viewer | — | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | viewer | — | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | subject_only | — | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | subject_only | — | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | other_user | — | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | other_user | — | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | super_admin_app | — | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | super_admin_app | — | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | read | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | read | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | write | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | write | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | admin | self | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | admin | other | household | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | owner | — | self | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | owner | — | other | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | assignee | — | self | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | assignee | — | other | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | viewer | — | self | private | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | viewer | — | other | private | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | subject_only | — | self | private | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | subject_only | — | other | private | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | other_user | — | self | private | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | other_user | — | other | private | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | super_admin_app | — | self | private | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | super_admin_app | — | other | private | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | agent | read | self | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | read | other | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | write | self | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | write | other | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | admin | self | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | admin | other | private | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | owner | — | self | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | owner | — | other | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | assignee | — | self | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | assignee | — | other | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | viewer | — | self | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | viewer | — | other | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | subject_only | — | self | selected | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | subject_only | — | other | selected | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | other_user | — | self | selected | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | other_user | — | other | selected | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | super_admin_app | — | self | selected | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | super_admin_app | — | other | selected | false | view | 404 |
| GET | `/v1/items/{id}/comments` |  | agent | read | self | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | read | other | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | write | self | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | write | other | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | admin | self | selected | false | view | 200 |
| GET | `/v1/items/{id}/comments` |  | agent | admin | other | selected | false | view | 200 |
| POST | `/v1/items/{id}/comments` |  | owner | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | owner | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | assignee | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | assignee | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | viewer | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | viewer | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | subject_only | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | subject_only | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | other_user | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | other_user | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | super_admin_app | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | super_admin_app | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | read | self | household | false | comment | 403 |
| POST | `/v1/items/{id}/comments` |  | agent | read | other | household | false | comment | 403 |
| POST | `/v1/items/{id}/comments` |  | agent | write | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | write | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | admin | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | admin | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | owner | — | self | private | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | owner | — | other | private | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | assignee | — | self | private | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | assignee | — | other | private | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | viewer | — | self | private | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | viewer | — | other | private | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | subject_only | — | self | private | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | subject_only | — | other | private | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | other_user | — | self | private | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | other_user | — | other | private | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | super_admin_app | — | self | private | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | super_admin_app | — | other | private | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | agent | read | self | private | false | comment | 403 |
| POST | `/v1/items/{id}/comments` |  | agent | read | other | private | false | comment | 403 |
| POST | `/v1/items/{id}/comments` |  | agent | write | self | private | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | write | other | private | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | admin | self | private | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | admin | other | private | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | owner | — | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | owner | — | other | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | assignee | — | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | assignee | — | other | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | viewer | — | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | viewer | — | other | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | subject_only | — | self | selected | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | subject_only | — | other | selected | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | other_user | — | self | selected | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | other_user | — | other | selected | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | super_admin_app | — | self | selected | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | super_admin_app | — | other | selected | false | comment | 404 |
| POST | `/v1/items/{id}/comments` |  | agent | read | self | selected | false | comment | 403 |
| POST | `/v1/items/{id}/comments` |  | agent | read | other | selected | false | comment | 403 |
| POST | `/v1/items/{id}/comments` |  | agent | write | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | write | other | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | admin | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/comments` |  | agent | admin | other | selected | false | comment | 201 |
| PATCH | `/v1/comments/{id}` |  | owner | — | self | household | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | owner | — | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | assignee | — | self | household | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | assignee | — | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | viewer | — | self | household | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | viewer | — | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | subject_only | — | self | household | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | subject_only | — | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | other_user | — | self | household | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | other_user | — | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | super_admin_app | — | self | household | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | super_admin_app | — | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | read | self | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | read | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | write | self | household | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | agent | write | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | admin | self | household | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | agent | admin | other | household | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | owner | — | self | private | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | owner | — | other | private | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | assignee | — | self | private | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | assignee | — | other | private | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | viewer | — | self | private | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | viewer | — | other | private | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | subject_only | — | self | private | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | subject_only | — | other | private | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | other_user | — | self | private | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | other_user | — | other | private | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | super_admin_app | — | self | private | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | super_admin_app | — | other | private | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | agent | read | self | private | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | read | other | private | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | write | self | private | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | agent | write | other | private | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | admin | self | private | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | agent | admin | other | private | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | owner | — | self | selected | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | owner | — | other | selected | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | assignee | — | self | selected | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | assignee | — | other | selected | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | viewer | — | self | selected | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | viewer | — | other | selected | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | subject_only | — | self | selected | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | subject_only | — | other | selected | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | other_user | — | self | selected | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | other_user | — | other | selected | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | super_admin_app | — | self | selected | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | super_admin_app | — | other | selected | false | author within 15 minutes | 404 |
| PATCH | `/v1/comments/{id}` |  | agent | read | self | selected | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | read | other | selected | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | write | self | selected | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | agent | write | other | selected | false | author within 15 minutes | 403 |
| PATCH | `/v1/comments/{id}` |  | agent | admin | self | selected | false | author within 15 minutes | 200 |
| PATCH | `/v1/comments/{id}` |  | agent | admin | other | selected | false | author within 15 minutes | 403 |
| DELETE | `/v1/comments/{id}` |  | owner | — | self | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | owner | — | other | household | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | assignee | — | self | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | assignee | — | other | household | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | viewer | — | self | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | viewer | — | other | household | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | subject_only | — | self | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | subject_only | — | other | household | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | other_user | — | self | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | other_user | — | other | household | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | super_admin_app | — | self | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | super_admin_app | — | other | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | read | self | household | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | agent | read | other | household | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | agent | write | self | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | write | other | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | admin | self | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | admin | other | household | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | owner | — | self | private | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | owner | — | other | private | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | assignee | — | self | private | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | assignee | — | other | private | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | viewer | — | self | private | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | viewer | — | other | private | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | subject_only | — | self | private | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | subject_only | — | other | private | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | other_user | — | self | private | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | other_user | — | other | private | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | super_admin_app | — | self | private | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | super_admin_app | — | other | private | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | agent | read | self | private | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | agent | read | other | private | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | agent | write | self | private | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | write | other | private | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | admin | self | private | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | admin | other | private | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | owner | — | self | selected | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | owner | — | other | selected | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | assignee | — | self | selected | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | assignee | — | other | selected | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | viewer | — | self | selected | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | viewer | — | other | selected | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | subject_only | — | self | selected | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | subject_only | — | other | selected | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | other_user | — | self | selected | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | other_user | — | other | selected | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | super_admin_app | — | self | selected | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | super_admin_app | — | other | selected | false | author or visible super-admin | 404 |
| DELETE | `/v1/comments/{id}` |  | agent | read | self | selected | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | agent | read | other | selected | false | author or visible super-admin | 403 |
| DELETE | `/v1/comments/{id}` |  | agent | write | self | selected | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | write | other | selected | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | admin | self | selected | false | author or visible super-admin | 204 |
| DELETE | `/v1/comments/{id}` |  | agent | admin | other | selected | false | author or visible super-admin | 204 |
| POST | `/v1/items/{id}/attachments` |  | owner | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | owner | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | assignee | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | assignee | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | viewer | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | viewer | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | subject_only | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | subject_only | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | other_user | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | other_user | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | super_admin_app | — | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | super_admin_app | — | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | read | self | household | false | comment | 403 |
| POST | `/v1/items/{id}/attachments` |  | agent | read | other | household | false | comment | 403 |
| POST | `/v1/items/{id}/attachments` |  | agent | write | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | write | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | admin | self | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | admin | other | household | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | owner | — | self | private | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | owner | — | other | private | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | assignee | — | self | private | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | assignee | — | other | private | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | viewer | — | self | private | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | viewer | — | other | private | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | subject_only | — | self | private | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | subject_only | — | other | private | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | other_user | — | self | private | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | other_user | — | other | private | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | super_admin_app | — | self | private | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | super_admin_app | — | other | private | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | agent | read | self | private | false | comment | 403 |
| POST | `/v1/items/{id}/attachments` |  | agent | read | other | private | false | comment | 403 |
| POST | `/v1/items/{id}/attachments` |  | agent | write | self | private | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | write | other | private | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | admin | self | private | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | admin | other | private | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | owner | — | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | owner | — | other | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | assignee | — | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | assignee | — | other | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | viewer | — | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | viewer | — | other | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | subject_only | — | self | selected | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | subject_only | — | other | selected | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | other_user | — | self | selected | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | other_user | — | other | selected | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | super_admin_app | — | self | selected | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | super_admin_app | — | other | selected | false | comment | 404 |
| POST | `/v1/items/{id}/attachments` |  | agent | read | self | selected | false | comment | 403 |
| POST | `/v1/items/{id}/attachments` |  | agent | read | other | selected | false | comment | 403 |
| POST | `/v1/items/{id}/attachments` |  | agent | write | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | write | other | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | admin | self | selected | false | comment | 201 |
| POST | `/v1/items/{id}/attachments` |  | agent | admin | other | selected | false | comment | 201 |
| POST | `/v1/comments/{id}/attachments` |  | owner | — | self | household | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | owner | — | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | assignee | — | self | household | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | assignee | — | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | viewer | — | self | household | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | viewer | — | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | subject_only | — | self | household | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | subject_only | — | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | other_user | — | self | household | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | other_user | — | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | super_admin_app | — | self | household | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | super_admin_app | — | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | read | self | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | read | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | write | self | household | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | agent | write | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | admin | self | household | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | agent | admin | other | household | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | owner | — | self | private | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | owner | — | other | private | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | assignee | — | self | private | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | assignee | — | other | private | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | viewer | — | self | private | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | viewer | — | other | private | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | subject_only | — | self | private | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | subject_only | — | other | private | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | other_user | — | self | private | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | other_user | — | other | private | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | super_admin_app | — | self | private | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | super_admin_app | — | other | private | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | agent | read | self | private | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | read | other | private | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | write | self | private | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | agent | write | other | private | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | admin | self | private | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | agent | admin | other | private | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | owner | — | self | selected | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | owner | — | other | selected | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | assignee | — | self | selected | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | assignee | — | other | selected | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | viewer | — | self | selected | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | viewer | — | other | selected | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | subject_only | — | self | selected | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | subject_only | — | other | selected | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | other_user | — | self | selected | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | other_user | — | other | selected | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | super_admin_app | — | self | selected | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | super_admin_app | — | other | selected | false | author within 15 minutes | 404 |
| POST | `/v1/comments/{id}/attachments` |  | agent | read | self | selected | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | read | other | selected | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | write | self | selected | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | agent | write | other | selected | false | author within 15 minutes | 403 |
| POST | `/v1/comments/{id}/attachments` |  | agent | admin | self | selected | false | author within 15 minutes | 201 |
| POST | `/v1/comments/{id}/attachments` |  | agent | admin | other | selected | false | author within 15 minutes | 403 |
| GET | `/v1/attachments/{id}` |  | owner | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | owner | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | assignee | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | assignee | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | viewer | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | viewer | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | subject_only | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | subject_only | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | other_user | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | other_user | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | super_admin_app | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | super_admin_app | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | read | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | read | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | write | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | write | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | admin | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | admin | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | owner | — | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | owner | — | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | assignee | — | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | assignee | — | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | viewer | — | self | private | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | viewer | — | other | private | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | subject_only | — | self | private | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | subject_only | — | other | private | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | other_user | — | self | private | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | other_user | — | other | private | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | super_admin_app | — | self | private | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | super_admin_app | — | other | private | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | agent | read | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | read | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | write | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | write | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | admin | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | admin | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | owner | — | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | owner | — | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | assignee | — | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | assignee | — | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | viewer | — | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | viewer | — | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | subject_only | — | self | selected | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | subject_only | — | other | selected | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | other_user | — | self | selected | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | other_user | — | other | selected | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | super_admin_app | — | self | selected | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | super_admin_app | — | other | selected | false | view | 404 |
| GET | `/v1/attachments/{id}` |  | agent | read | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | read | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | write | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | write | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | admin | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}` |  | agent | admin | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | owner | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | owner | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | assignee | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | assignee | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | viewer | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | viewer | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | subject_only | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | subject_only | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | other_user | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | other_user | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | super_admin_app | — | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | super_admin_app | — | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | read | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | read | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | write | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | write | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | admin | self | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | admin | other | household | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | owner | — | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | owner | — | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | assignee | — | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | assignee | — | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | viewer | — | self | private | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | viewer | — | other | private | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | subject_only | — | self | private | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | subject_only | — | other | private | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | other_user | — | self | private | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | other_user | — | other | private | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | super_admin_app | — | self | private | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | super_admin_app | — | other | private | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | read | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | read | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | write | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | write | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | admin | self | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | admin | other | private | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | owner | — | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | owner | — | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | assignee | — | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | assignee | — | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | viewer | — | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | viewer | — | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | subject_only | — | self | selected | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | subject_only | — | other | selected | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | other_user | — | self | selected | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | other_user | — | other | selected | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | super_admin_app | — | self | selected | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | super_admin_app | — | other | selected | false | view | 404 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | read | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | read | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | write | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | write | other | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | admin | self | selected | false | view | 200 |
| GET | `/v1/attachments/{id}/thumbnail` |  | agent | admin | other | selected | false | view | 200 |
| DELETE | `/v1/attachments/{id}` |  | owner | — | self | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | owner | — | other | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | assignee | — | self | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | assignee | — | other | household | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | viewer | — | self | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | viewer | — | other | household | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | subject_only | — | self | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | subject_only | — | other | household | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | other_user | — | self | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | other_user | — | other | household | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | super_admin_app | — | self | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | super_admin_app | — | other | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | read | self | household | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | agent | read | other | household | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | agent | write | self | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | write | other | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | admin | self | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | admin | other | household | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | owner | — | self | private | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | owner | — | other | private | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | assignee | — | self | private | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | assignee | — | other | private | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | viewer | — | self | private | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | viewer | — | other | private | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | subject_only | — | self | private | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | subject_only | — | other | private | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | other_user | — | self | private | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | other_user | — | other | private | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | super_admin_app | — | self | private | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | super_admin_app | — | other | private | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | agent | read | self | private | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | agent | read | other | private | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | agent | write | self | private | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | write | other | private | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | admin | self | private | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | admin | other | private | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | owner | — | self | selected | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | owner | — | other | selected | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | assignee | — | self | selected | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | assignee | — | other | selected | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | viewer | — | self | selected | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | viewer | — | other | selected | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | subject_only | — | self | selected | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | subject_only | — | other | selected | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | other_user | — | self | selected | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | other_user | — | other | selected | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | super_admin_app | — | self | selected | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | super_admin_app | — | other | selected | false | item attachment: uploader or editor | 404 |
| DELETE | `/v1/attachments/{id}` |  | agent | read | self | selected | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | agent | read | other | selected | false | item attachment: uploader or editor | 403 |
| DELETE | `/v1/attachments/{id}` |  | agent | write | self | selected | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | write | other | selected | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | admin | self | selected | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` |  | agent | admin | other | selected | false | item attachment: uploader or editor | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | owner | — | self | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | owner | — | other | household | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | assignee | — | self | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | assignee | — | other | household | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | viewer | — | self | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | viewer | — | other | household | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | subject_only | — | self | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | subject_only | — | other | household | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | other_user | — | self | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | other_user | — | other | household | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | super_admin_app | — | self | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | super_admin_app | — | other | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | read | self | household | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | read | other | household | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | write | self | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | write | other | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | admin | self | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | admin | other | household | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | owner | — | self | private | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | owner | — | other | private | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | assignee | — | self | private | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | assignee | — | other | private | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | viewer | — | self | private | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | viewer | — | other | private | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | subject_only | — | self | private | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | subject_only | — | other | private | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | other_user | — | self | private | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | other_user | — | other | private | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | super_admin_app | — | self | private | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | super_admin_app | — | other | private | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | read | self | private | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | read | other | private | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | write | self | private | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | write | other | private | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | admin | self | private | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | admin | other | private | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | owner | — | self | selected | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | owner | — | other | selected | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | assignee | — | self | selected | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | assignee | — | other | selected | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | viewer | — | self | selected | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | viewer | — | other | selected | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | subject_only | — | self | selected | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | subject_only | — | other | selected | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | other_user | — | self | selected | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | other_user | — | other | selected | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | super_admin_app | — | self | selected | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | super_admin_app | — | other | selected | false | comment attachment: author within 15 minutes or super admin | 404 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | read | self | selected | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | read | other | selected | false | comment attachment: author within 15 minutes or super admin | 403 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | write | self | selected | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | write | other | selected | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | admin | self | selected | false | comment attachment: author within 15 minutes or super admin | 204 |
| DELETE | `/v1/attachments/{id}` | comment-attachment | agent | admin | other | selected | false | comment attachment: author within 15 minutes or super admin | 204 |
| GET | `/v1/stream` |  | user | — | self | — | — | session-only SSE | allowed |
| GET | `/v1/stream` |  | user | — | other | — | — | session-only SSE | allowed |
| GET | `/v1/stream` |  | super_admin_app | — | self | — | — | session-only SSE | allowed |
| GET | `/v1/stream` |  | super_admin_app | — | other | — | — | session-only SSE | allowed |
| GET | `/v1/stream` |  | agent | read | self | — | — | session-only SSE | 404 |
| GET | `/v1/stream` |  | agent | read | other | — | — | session-only SSE | 404 |
| GET | `/v1/stream` |  | agent | write | self | — | — | session-only SSE | 404 |
| GET | `/v1/stream` |  | agent | write | other | — | — | session-only SSE | 404 |
| GET | `/v1/stream` |  | agent | admin | self | — | — | session-only SSE | 404 |
| GET | `/v1/stream` |  | agent | admin | other | — | — | session-only SSE | 404 |
| GET | `/v1/stream` | SSE item.changed | owner | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | assignee | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | viewer | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | subject_only | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | other_user | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | super_admin_app | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | agent | write | — | household | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE item.changed | owner | — | — | private | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | assignee | — | — | private | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | viewer | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.changed | subject_only | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.changed | other_user | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.changed | super_admin_app | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.changed | agent | write | — | private | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE item.changed | owner | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | assignee | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | viewer | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.changed | subject_only | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.changed | other_user | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.changed | super_admin_app | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.changed | agent | write | — | selected | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE item.access_changed | owner | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | assignee | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | viewer | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | subject_only | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | other_user | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | super_admin_app | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | agent | write | — | household | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE item.access_changed | owner | — | — | private | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | assignee | — | — | private | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | viewer | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.access_changed | subject_only | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.access_changed | other_user | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.access_changed | super_admin_app | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.access_changed | agent | write | — | private | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE item.access_changed | owner | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | assignee | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | viewer | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE item.access_changed | subject_only | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.access_changed | other_user | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.access_changed | super_admin_app | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE item.access_changed | agent | write | — | selected | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | owner | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | assignee | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | viewer | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | subject_only | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | other_user | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | super_admin_app | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | agent | write | — | household | false | eviction, no content | 404 |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | owner | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | assignee | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | viewer | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | subject_only | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | other_user | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | super_admin_app | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | agent | write | — | private | false | eviction, no content | 404 |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | owner | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | assignee | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | viewer | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | subject_only | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | other_user | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | super_admin_app | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE item.access_changed (former reader) | agent | write | — | selected | false | eviction, no content | 404 |
| GET | `/v1/stream` | SSE occurrence.changed | owner | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | assignee | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | viewer | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | subject_only | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | other_user | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | super_admin_app | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | agent | write | — | household | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE occurrence.changed | owner | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | assignee | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | viewer | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE occurrence.changed | subject_only | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE occurrence.changed | other_user | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE occurrence.changed | super_admin_app | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE occurrence.changed | agent | write | — | private | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE occurrence.changed | owner | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | assignee | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | viewer | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE occurrence.changed | subject_only | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE occurrence.changed | other_user | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE occurrence.changed | super_admin_app | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE occurrence.changed | agent | write | — | selected | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE derived | owner | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | assignee | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | viewer | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | subject_only | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | other_user | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | super_admin_app | — | — | household | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | agent | write | — | household | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE derived | owner | — | — | private | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | assignee | — | — | private | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | viewer | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE derived | subject_only | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE derived | other_user | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE derived | super_admin_app | — | — | private | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE derived | agent | write | — | private | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE derived | owner | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | assignee | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | viewer | — | — | selected | false | current item ACL | allowed |
| GET | `/v1/stream` | SSE derived | subject_only | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE derived | other_user | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE derived | super_admin_app | — | — | selected | false | current item ACL | filtered |
| GET | `/v1/stream` | SSE derived | agent | write | — | selected | false | current item ACL | 404 |
| GET | `/v1/stream` | SSE list.changed (attached) | owner | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | assignee | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | viewer | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | subject_only | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | other_user | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | super_admin_app | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | agent | write | — | household | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE list.changed (attached) | owner | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | assignee | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | viewer | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (attached) | subject_only | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (attached) | other_user | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (attached) | super_admin_app | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (attached) | agent | write | — | private | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE list.changed (attached) | owner | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | assignee | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | viewer | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (attached) | subject_only | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (attached) | other_user | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (attached) | super_admin_app | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (attached) | agent | write | — | selected | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | owner | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | assignee | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | viewer | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | subject_only | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | other_user | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | super_admin_app | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | agent | write | — | household | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | owner | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | assignee | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | viewer | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | subject_only | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | other_user | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | super_admin_app | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | agent | write | — | private | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | owner | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | assignee | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | viewer | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | subject_only | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | other_user | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | super_admin_app | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (attached) | agent | write | — | selected | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE list.changed (standalone) | owner | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | assignee | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | viewer | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | subject_only | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | other_user | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | super_admin_app | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | agent | write | — | household | false | standalone list ACL | 404 |
| GET | `/v1/stream` | SSE list.changed (standalone) | owner | — | — | private | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | assignee | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | viewer | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | subject_only | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | other_user | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | super_admin_app | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | agent | write | — | private | false | standalone list ACL | 404 |
| GET | `/v1/stream` | SSE list.changed (standalone) | owner | — | — | selected | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | assignee | — | — | selected | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | viewer | — | — | selected | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list.changed (standalone) | subject_only | — | — | selected | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | other_user | — | — | selected | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | super_admin_app | — | — | selected | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list.changed (standalone) | agent | write | — | selected | false | standalone list ACL | 404 |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | owner | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | assignee | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | viewer | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | subject_only | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | other_user | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | super_admin_app | — | — | household | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | agent | write | — | household | false | standalone list ACL | 404 |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | owner | — | — | private | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | assignee | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | viewer | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | subject_only | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | other_user | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | super_admin_app | — | — | private | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | agent | write | — | private | false | standalone list ACL | 404 |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | owner | — | — | selected | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | assignee | — | — | selected | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | viewer | — | — | selected | false | standalone list ACL | allowed |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | subject_only | — | — | selected | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | other_user | — | — | selected | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | super_admin_app | — | — | selected | false | standalone list ACL | filtered |
| GET | `/v1/stream` | SSE list_entry.changed (standalone) | agent | write | — | selected | false | standalone list ACL | 404 |
| GET | `/v1/stream` | SSE list.changed (former reader) | owner | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | assignee | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | viewer | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | subject_only | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | other_user | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | super_admin_app | — | — | household | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | agent | write | — | household | false | eviction, no content | 404 |
| GET | `/v1/stream` | SSE list.changed (former reader) | owner | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | assignee | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | viewer | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | subject_only | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | other_user | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | super_admin_app | — | — | private | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | agent | write | — | private | false | eviction, no content | 404 |
| GET | `/v1/stream` | SSE list.changed (former reader) | owner | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | assignee | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | viewer | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | subject_only | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | other_user | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | super_admin_app | — | — | selected | false | eviction, no content | allowed |
| GET | `/v1/stream` | SSE list.changed (former reader) | agent | write | — | selected | false | eviction, no content | 404 |
| GET | `/v1/stream` | SSE comment.changed | owner | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | assignee | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | viewer | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | subject_only | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | other_user | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | super_admin_app | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | agent | write | — | household | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE comment.changed | owner | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | assignee | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | viewer | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE comment.changed | subject_only | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE comment.changed | other_user | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE comment.changed | super_admin_app | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE comment.changed | agent | write | — | private | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE comment.changed | owner | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | assignee | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | viewer | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE comment.changed | subject_only | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE comment.changed | other_user | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE comment.changed | super_admin_app | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE comment.changed | agent | write | — | selected | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE attachment.changed | owner | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | assignee | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | viewer | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | subject_only | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | other_user | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | super_admin_app | — | — | household | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | agent | write | — | household | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE attachment.changed | owner | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | assignee | — | — | private | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | viewer | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE attachment.changed | subject_only | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE attachment.changed | other_user | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE attachment.changed | super_admin_app | — | — | private | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE attachment.changed | agent | write | — | private | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE attachment.changed | owner | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | assignee | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | viewer | — | — | selected | false | current parent ACL | allowed |
| GET | `/v1/stream` | SSE attachment.changed | subject_only | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE attachment.changed | other_user | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE attachment.changed | super_admin_app | — | — | selected | false | current parent ACL | filtered |
| GET | `/v1/stream` | SSE attachment.changed | agent | write | — | selected | false | current parent ACL | 404 |
| GET | `/v1/stream` | SSE notification.created | owner | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | owner | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | assignee | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | assignee | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | viewer | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | viewer | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | subject_only | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | subject_only | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | other_user | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | other_user | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | super_admin_app | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | super_admin_app | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | agent | read | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | read | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | write | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | write | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | admin | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | admin | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | owner | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | owner | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | assignee | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | assignee | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | viewer | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | viewer | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | subject_only | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | subject_only | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | other_user | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | other_user | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | super_admin_app | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | super_admin_app | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | agent | read | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | read | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | write | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | write | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | admin | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | admin | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | owner | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | owner | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | assignee | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | assignee | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | viewer | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | viewer | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | subject_only | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | subject_only | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | other_user | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | other_user | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | super_admin_app | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.created | super_admin_app | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.created | agent | read | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | read | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | write | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | write | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | admin | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.created | agent | admin | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | owner | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | owner | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | assignee | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | assignee | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | viewer | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | viewer | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | subject_only | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | subject_only | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | other_user | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | other_user | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | super_admin_app | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | super_admin_app | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | agent | read | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | read | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | write | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | write | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | admin | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | admin | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | owner | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | owner | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | assignee | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | assignee | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | viewer | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | viewer | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | subject_only | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | subject_only | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | other_user | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | other_user | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | super_admin_app | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | super_admin_app | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | agent | read | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | read | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | write | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | write | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | admin | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | admin | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | owner | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | owner | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | assignee | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | assignee | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | viewer | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | viewer | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | subject_only | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | subject_only | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | other_user | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | other_user | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | super_admin_app | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.updated | super_admin_app | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.updated | agent | read | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | read | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | write | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | write | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | admin | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.updated | agent | admin | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | owner | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | owner | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | assignee | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | assignee | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | viewer | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | viewer | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | subject_only | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | subject_only | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | other_user | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | other_user | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | super_admin_app | — | self | household | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | super_admin_app | — | other | household | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | agent | read | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | read | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | write | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | write | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | admin | self | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | admin | other | household | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | owner | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | owner | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | assignee | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | assignee | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | viewer | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | viewer | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | subject_only | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | subject_only | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | other_user | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | other_user | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | super_admin_app | — | self | private | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | super_admin_app | — | other | private | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | agent | read | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | read | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | write | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | write | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | admin | self | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | admin | other | private | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | owner | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | owner | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | assignee | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | assignee | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | viewer | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | viewer | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | subject_only | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | subject_only | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | other_user | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | other_user | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | super_admin_app | — | self | selected | false | recipient equality, IDs only | allowed |
| GET | `/v1/stream` | SSE notification.dismissed | super_admin_app | — | other | selected | false | recipient equality, IDs only | filtered |
| GET | `/v1/stream` | SSE notification.dismissed | agent | read | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | read | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | write | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | write | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | admin | self | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE notification.dismissed | agent | admin | other | selected | false | recipient equality, IDs only | 404 |
| GET | `/v1/stream` | SSE settings.changed | user | — | self | — | — | member equality | allowed |
| GET | `/v1/stream` | SSE settings.changed | user | — | other | — | — | member equality | filtered |
| GET | `/v1/stream` | SSE settings.changed | super_admin_app | — | self | — | — | member equality | allowed |
| GET | `/v1/stream` | SSE settings.changed | super_admin_app | — | other | — | — | member equality | filtered |
| GET | `/v1/stream` | SSE settings.changed | agent | read | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE settings.changed | agent | read | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE settings.changed | agent | write | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE settings.changed | agent | write | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE settings.changed | agent | admin | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE settings.changed | agent | admin | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE ics_source.changed | user | — | self | — | — | member equality | allowed |
| GET | `/v1/stream` | SSE ics_source.changed | user | — | other | — | — | member equality | filtered |
| GET | `/v1/stream` | SSE ics_source.changed | super_admin_app | — | self | — | — | member equality | allowed |
| GET | `/v1/stream` | SSE ics_source.changed | super_admin_app | — | other | — | — | member equality | filtered |
| GET | `/v1/stream` | SSE ics_source.changed | agent | read | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE ics_source.changed | agent | read | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE ics_source.changed | agent | write | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE ics_source.changed | agent | write | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE ics_source.changed | agent | admin | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE ics_source.changed | agent | admin | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE suggestion.changed | user | — | self | — | — | member equality | allowed |
| GET | `/v1/stream` | SSE suggestion.changed | user | — | other | — | — | member equality | filtered |
| GET | `/v1/stream` | SSE suggestion.changed | super_admin_app | — | self | — | — | member equality | allowed |
| GET | `/v1/stream` | SSE suggestion.changed | super_admin_app | — | other | — | — | member equality | filtered |
| GET | `/v1/stream` | SSE suggestion.changed | agent | read | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE suggestion.changed | agent | read | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE suggestion.changed | agent | write | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE suggestion.changed | agent | write | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE suggestion.changed | agent | admin | self | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE suggestion.changed | agent | admin | other | — | — | member equality | 404 |
| GET | `/v1/stream` | SSE member.changed | user | — | self | — | — | household broadcast, IDs only | allowed |
| GET | `/v1/stream` | SSE member.changed | user | — | other | — | — | household broadcast, IDs only | allowed |
| GET | `/v1/stream` | SSE member.changed | super_admin_app | — | self | — | — | household broadcast, IDs only | allowed |
| GET | `/v1/stream` | SSE member.changed | super_admin_app | — | other | — | — | household broadcast, IDs only | allowed |
| GET | `/v1/stream` | SSE member.changed | agent | read | self | — | — | household broadcast, IDs only | 404 |
| GET | `/v1/stream` | SSE member.changed | agent | read | other | — | — | household broadcast, IDs only | 404 |
| GET | `/v1/stream` | SSE member.changed | agent | write | self | — | — | household broadcast, IDs only | 404 |
| GET | `/v1/stream` | SSE member.changed | agent | write | other | — | — | household broadcast, IDs only | 404 |
| GET | `/v1/stream` | SSE member.changed | agent | admin | self | — | — | household broadcast, IDs only | 404 |
| GET | `/v1/stream` | SSE member.changed | agent | admin | other | — | — | household broadcast, IDs only | 404 |
| GET | `/v1/members/{id}/availability` |  | user | — | self | — | — | self/admin; independent of item visibility | 200 |
| GET | `/v1/members/{id}/availability` |  | user | — | other | — | — | self/admin; independent of item visibility | 403 |
| GET | `/v1/members/{id}/availability` |  | super_admin_app | — | self | — | — | self/admin; independent of item visibility | 200 |
| GET | `/v1/members/{id}/availability` |  | super_admin_app | — | other | — | — | self/admin; independent of item visibility | 200 |
| GET | `/v1/members/{id}/availability` |  | agent | read | self | — | — | self/admin; independent of item visibility | 200 |
| GET | `/v1/members/{id}/availability` |  | agent | read | other | — | — | self/admin; independent of item visibility | 200 |
| GET | `/v1/members/{id}/availability` |  | agent | write | self | — | — | self/admin; independent of item visibility | 200 |
| GET | `/v1/members/{id}/availability` |  | agent | write | other | — | — | self/admin; independent of item visibility | 200 |
| GET | `/v1/members/{id}/availability` |  | agent | admin | self | — | — | self/admin; independent of item visibility | 200 |
| GET | `/v1/members/{id}/availability` |  | agent | admin | other | — | — | self/admin; independent of item visibility | 200 |
| PUT | `/v1/members/{id}/availability` |  | user | — | self | — | — | self/admin; independent of item visibility | 200 |
| PUT | `/v1/members/{id}/availability` |  | user | — | other | — | — | self/admin; independent of item visibility | 403 |
| PUT | `/v1/members/{id}/availability` |  | super_admin_app | — | self | — | — | self/admin; independent of item visibility | 200 |
| PUT | `/v1/members/{id}/availability` |  | super_admin_app | — | other | — | — | self/admin; independent of item visibility | 200 |
| PUT | `/v1/members/{id}/availability` |  | agent | read | self | — | — | self/admin; independent of item visibility | 403 |
| PUT | `/v1/members/{id}/availability` |  | agent | read | other | — | — | self/admin; independent of item visibility | 403 |
| PUT | `/v1/members/{id}/availability` |  | agent | write | self | — | — | self/admin; independent of item visibility | 200 |
| PUT | `/v1/members/{id}/availability` |  | agent | write | other | — | — | self/admin; independent of item visibility | 200 |
| PUT | `/v1/members/{id}/availability` |  | agent | admin | self | — | — | self/admin; independent of item visibility | 200 |
| PUT | `/v1/members/{id}/availability` |  | agent | admin | other | — | — | self/admin; independent of item visibility | 200 |
| POST | `/v1/schedule/suggest` |  | user | — | self | — | — | self only; member agenda inputs, caller visibility filters response | 200 |
| POST | `/v1/schedule/suggest` |  | user | — | other | — | — | self only; member agenda inputs, caller visibility filters response | 403 |
| POST | `/v1/schedule/suggest` |  | super_admin_app | — | self | — | — | self only; member agenda inputs, caller visibility filters response | 200 |
| POST | `/v1/schedule/suggest` |  | super_admin_app | — | other | — | — | self only; member agenda inputs, caller visibility filters response | 403 |
| POST | `/v1/schedule/suggest` |  | agent | read | self | — | — | self only; member agenda inputs, caller visibility filters response | 403 |
| POST | `/v1/schedule/suggest` |  | agent | read | other | — | — | self only; member agenda inputs, caller visibility filters response | 403 |
| POST | `/v1/schedule/suggest` |  | agent | write | self | — | — | self only; member agenda inputs, caller visibility filters response | 200 |
| POST | `/v1/schedule/suggest` |  | agent | write | other | — | — | self only; member agenda inputs, caller visibility filters response | 403 |
| POST | `/v1/schedule/suggest` |  | agent | admin | self | — | — | self only; member agenda inputs, caller visibility filters response | 200 |
| POST | `/v1/schedule/suggest` |  | agent | admin | other | — | — | self only; member agenda inputs, caller visibility filters response | 403 |
| POST | `/v1/schedule/suggest` | dependent | user | — | self | — | — | super admin for dependent; member agenda inputs | 403 |
| POST | `/v1/schedule/suggest` | dependent | user | — | other | — | — | super admin for dependent; member agenda inputs | 403 |
| POST | `/v1/schedule/suggest` | dependent | super_admin_app | — | self | — | — | super admin for dependent; member agenda inputs | 200 |
| POST | `/v1/schedule/suggest` | dependent | super_admin_app | — | other | — | — | super admin for dependent; member agenda inputs | 200 |
| POST | `/v1/schedule/suggest` | dependent | agent | read | self | — | — | super admin for dependent; member agenda inputs | 403 |
| POST | `/v1/schedule/suggest` | dependent | agent | read | other | — | — | super admin for dependent; member agenda inputs | 403 |
| POST | `/v1/schedule/suggest` | dependent | agent | write | self | — | — | super admin for dependent; member agenda inputs | 403 |
| POST | `/v1/schedule/suggest` | dependent | agent | write | other | — | — | super admin for dependent; member agenda inputs | 403 |
| POST | `/v1/schedule/suggest` | dependent | agent | admin | self | — | — | super admin for dependent; member agenda inputs | 200 |
| POST | `/v1/schedule/suggest` | dependent | agent | admin | other | — | — | super admin for dependent; member agenda inputs | 200 |
| GET | `/v1/suggestions` |  | user | — | self | — | — | self/admin; item ACL filters rows | 200 |
| GET | `/v1/suggestions` |  | user | — | other | — | — | self/admin; item ACL filters rows | 403 |
| GET | `/v1/suggestions` |  | super_admin_app | — | self | — | — | self/admin; item ACL filters rows | 200 |
| GET | `/v1/suggestions` |  | super_admin_app | — | other | — | — | self/admin; item ACL filters rows | 200 |
| GET | `/v1/suggestions` |  | agent | read | self | — | — | self/admin; item ACL filters rows | 200 |
| GET | `/v1/suggestions` |  | agent | read | other | — | — | self/admin; item ACL filters rows | 200 |
| GET | `/v1/suggestions` |  | agent | write | self | — | — | self/admin; item ACL filters rows | 200 |
| GET | `/v1/suggestions` |  | agent | write | other | — | — | self/admin; item ACL filters rows | 200 |
| GET | `/v1/suggestions` |  | agent | admin | self | — | — | self/admin; item ACL filters rows | 200 |
| GET | `/v1/suggestions` |  | agent | admin | other | — | — | self/admin; item ACL filters rows | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | owner | — | self | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | owner | — | other | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | assignee | — | self | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | assignee | — | other | household | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | viewer | — | self | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | viewer | — | other | household | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | subject_only | — | self | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | subject_only | — | other | household | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | other_user | — | self | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | other_user | — | other | household | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | super_admin_app | — | self | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | super_admin_app | — | other | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | read | self | household | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | read | other | household | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | write | self | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | write | other | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | admin | self | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | admin | other | household | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | owner | — | self | private | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | owner | — | other | private | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | assignee | — | self | private | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | assignee | — | other | private | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | viewer | — | self | private | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | viewer | — | other | private | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | subject_only | — | self | private | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | subject_only | — | other | private | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | other_user | — | self | private | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | other_user | — | other | private | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | super_admin_app | — | self | private | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | super_admin_app | — | other | private | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | read | self | private | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | read | other | private | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | write | self | private | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | write | other | private | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | admin | self | private | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | admin | other | private | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | owner | — | self | selected | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | owner | — | other | selected | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | assignee | — | self | selected | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | assignee | — | other | selected | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | viewer | — | self | selected | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | viewer | — | other | selected | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | subject_only | — | self | selected | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | subject_only | — | other | selected | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | other_user | — | self | selected | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | other_user | — | other | selected | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | super_admin_app | — | self | selected | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | super_admin_app | — | other | selected | false | item edit OR own agenda member; future proposal | 404 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | read | self | selected | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | read | other | selected | false | item edit OR own agenda member; future proposal | 403 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | write | self | selected | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | write | other | selected | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | admin | self | selected | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/accept` |  | agent | admin | other | selected | false | item edit OR own agenda member; future proposal | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | owner | — | self | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | owner | — | other | household | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | assignee | — | self | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | assignee | — | other | household | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | viewer | — | self | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | viewer | — | other | household | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | subject_only | — | self | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | subject_only | — | other | household | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | other_user | — | self | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | other_user | — | other | household | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | super_admin_app | — | self | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | super_admin_app | — | other | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | read | self | household | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | read | other | household | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | write | self | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | write | other | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | admin | self | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | admin | other | household | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | owner | — | self | private | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | owner | — | other | private | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | assignee | — | self | private | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | assignee | — | other | private | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | viewer | — | self | private | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | viewer | — | other | private | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | subject_only | — | self | private | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | subject_only | — | other | private | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | other_user | — | self | private | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | other_user | — | other | private | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | super_admin_app | — | self | private | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | super_admin_app | — | other | private | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | read | self | private | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | read | other | private | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | write | self | private | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | write | other | private | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | admin | self | private | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | admin | other | private | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | owner | — | self | selected | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | owner | — | other | selected | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | assignee | — | self | selected | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | assignee | — | other | selected | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | viewer | — | self | selected | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | viewer | — | other | selected | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | subject_only | — | self | selected | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | subject_only | — | other | selected | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | other_user | — | self | selected | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | other_user | — | other | selected | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | super_admin_app | — | self | selected | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | super_admin_app | — | other | selected | false | self/admin + item view | 404 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | read | self | selected | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | read | other | selected | false | self/admin + item view | 403 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | write | self | selected | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | write | other | selected | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | admin | self | selected | false | self/admin + item view | 200 |
| POST | `/v1/suggestions/{id}/dismiss` |  | agent | admin | other | selected | false | self/admin + item view | 200 |
| GET | `/v1/items/{id}/candidates` |  | owner | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | assignee | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | viewer | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | subject_only | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | other_user | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | super_admin_app | — | — | household | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | agent | write | — | household | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | owner | — | — | private | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | assignee | — | — | private | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | viewer | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}/candidates` |  | subject_only | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}/candidates` |  | other_user | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}/candidates` |  | super_admin_app | — | — | private | false | view | 404 |
| GET | `/v1/items/{id}/candidates` |  | agent | write | — | private | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | owner | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | assignee | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | viewer | — | — | selected | false | view | 200 |
| GET | `/v1/items/{id}/candidates` |  | subject_only | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}/candidates` |  | other_user | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}/candidates` |  | super_admin_app | — | — | selected | false | view | 404 |
| GET | `/v1/items/{id}/candidates` |  | agent | write | — | selected | false | view | 200 |
