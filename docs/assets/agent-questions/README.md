# Desktop question indicators

Actual GPUI application captures using Xvfb and lavapipe, 1400 × 900,
Dark theme, a synthetic completed reply containing a choice and free-text question.
The before binary was built before UI edits from base 30dc0d78; the after binary
includes this change. Both use the same isolated fixture and viewport.

- `before-dark.png`: the question card exists, but navigation looks idle.
- `after-dark.png`: session, header and chat tab flag the unanswered question.

Local verification covers the desktop suite and a storage regression for restoring
questions from history. Live authenticated agent end-to-end submission and other
operating systems are not covered by these captures.
