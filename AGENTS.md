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
