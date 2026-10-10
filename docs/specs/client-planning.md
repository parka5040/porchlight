# Client planning behavior (C6)

Implementation and controller gates: [client planning operations](../operations/client-planning.md).
No server contract, permissions, dependencies or routes change in fix round 2.

## Picker/PIN relock

- ICS import and member/household export are signed-in-caller operations. Their memory drafts are keyed by
  Settings section + signed-in member id, independently of the super admin's availability/planning member selector.
- A picker result received while PIN-locked is retained in the identity-owned session. Preview/download starts only
  after the gated composition returns. Successful requests run once; relocking/unlocking again does not replay them.
- A pending export destination expires after 60 seconds waiting for initiation. Expired/cancelled requests cannot
  later start, including with an old step-up token. Entering a different planning section cancels noncurrent exports.
  The unlock deadline does not cap an already-running download. Identity change still clears drafts and cancels I/O.
- Navigating away from an active export cancels it and reports “Export interrupted before completion. Try again.”
  Export publishing still checks the UI PIN/identity gate; R64's background-work allowance is unchanged.
- Same-frame import double taps are ignored while a picker is outstanding. File scopes preserve normalized short
  names; long names use a truncated prefix and stable full-name 64-bit FNV-1a suffix, at most 200 characters.

## Planning and feed feedback

- Dependent/household feed targets default to household visibility; ordinary member targets default to private.
  Visibility remains explicitly editable. Import commit resets the prior feed-refresh queued message to Saved.
- Expired/rejected proposal cards disable Accept, not Dismiss (unless busy). Both planning panels allow clearing them.
- Only proposal-acceptance 422s use the expiry/refetch message. Other planning mutations preserve server detail.

## Evidence

`C6AdminPickerRelockTest`: selected-dependent → same-composition section switch → import and member/household
export picker relock → unlock → exactly one request; active-navigation interruption wording.
`PlanningFix2Test`: orphan cancellation, bounded unlock wait, no late or duplicate start, long scope stability and
feed visibility defaults. `C6Fix2BehaviorTest`: same-frame taps, long scope on the wire, feed→import Saved feedback,
dependent visibility, unrelated 422 detail and actual dismissal of expired cards in both panels.
Real DocumentsUI/JFileChooser and emulator lifecycle verification remain controller gates.
