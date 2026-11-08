# Completion notification expiry proof

These are actual native notification captures on an isolated Xvfb display,
1400 × 900, with the same Dunst 1.12.2 configuration and theme. The daemon is
configured with `override_dbus_timeout = 0`, so it ignores notification expiry
requests; cleanup must come from the application. A separate control
notification remains visible throughout each run.

The unmodified Linux `notify_turn_finished` functions from the PR base and
updated head were compiled into standalone senders using the same GIO 0.18.4
dependency. Each sender issues three completions (Build task, Review task,
Test task). These captures exercise the actual notification sender and desktop
notification service, rather than the complete GPUI application. They do not
claim live GNOME lock screen verification.

- `before-12s.png`: baseline sender; all three completions remain after 12 seconds.
- `after-visible.png`: updated sender; all three completions are initially visible.
- `after-12s.png`: updated sender; all three completions have been explicitly
  closed after 12 seconds, while the control remains.

The captures are unedited X11 root-window screenshots; no private chat data is used.
