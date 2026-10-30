# Repository agent instructions

## Codex review loop

Every nontrivial change goes through a pull request into `master` whose final head
has a clean independent Codex review: no P0, P1, or P2 findings. Only trivial
changes limited to documentation, comments, or formatting may skip review.

1. Commit the change and request an independent Codex review of the complete PR
   diff against its base. Record the reviewed head commit.
2. Investigate each finding, fix valid issues in new commits, and run the checks
   appropriate to those fixes. Explain any rejected finding with concrete evidence.
3. Request a fresh Codex review after each head change. Repeat until the final
   head has no unresolved P0–P2 findings; a review of an earlier head does not count.
4. Run PR CI and review in parallel. Merge only when the final head has a clean
   review and required checks pass, and only when merging is authorized.

Report the reviewed head, review outcome, checks performed, and any validation
that remains incomplete. Never claim a review or check passed when it did not run.

## Commit and merge authorship

Codex and Claude must never add themselves, each other, or any other AI assistant
as a co-author. Do not add AI `Co-authored-by` trailers or equivalent attribution
when creating or amending commits, preparing PR messages, or merging, squashing,
or rebasing a PR. Inspect the final commit message, including any message generated
by GitHub, and remove AI co-author trailers before committing or merging. Use the
user's configured Git identity and preserve required license notices and attribution.

## UI screenshot proof

Every UI change must include before-and-after screenshot proof in the PR
description. Capture the affected UI before editing and again after the change,
using the same viewport, theme, and comparable application state so the difference
is clear. Label both screenshots and cover each affected surface or layout.
Use actual application captures and keep private or sensitive content out of the
images. Do not mark a UI change ready for review or merge until both screenshots
are included; report any capture blockers and incomplete validation explicitly.
