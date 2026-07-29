# Backend / storage / settings logic that must stay free of GTK so it can be
# exercised by the headless tests in tests/.
xd_core_sources = files(
  'settings/folder-settings.c',
  'settings/agent-secrets.c',
  'settings/pane-state.c',
  'settings/settings-resolver.c',
  'util/markdown.c',
  'util/ask-block.c',
  'util/git-diff.c',
  'util/git-head-watch.c',
  'util/git-info.c',
  'util/subagent-tool.c',
  'util/syntax.c',
  'util/unified-diff.c',
  'util/branch-build.c',
  'util/update-channel.c',
  'util/workflow-run.c',
  'util/workspace-block.c',
  'util/worktree.c',
  'util/host-launch.c',
  'util/app-paths.c',
  'integrations/discord-presence.c',
  'remote/client.c',
  'remote/remote-tree.c',
  'tree/xd-node.c',
  'storage/storage.c',
  'backend/backend.c',
  'backend/claude-backend.c',
  'backend/codex-backend.c',
  'backend/cerebras-backend.c',
  # No GTK here either, so the subprocess plumbing can be tested headlessly.
  'chat/chat-session.c',
  'chat/handover.c',
  'chat/chat-title.c',
)

if has_server
  xd_core_sources += files(
    'remote/server.c',
    'remote/turn.c',
    'remote/terminal.c',
  )
endif

# Pango, but not GTK: the Markdown converter validates the markup it produces,
# which needs the parser and nothing else.
xd_core_deps = [glib_dep, gio_dep, json_dep, sqlite_dep, pango_dep, cmark_dep]
if has_server
  xd_core_deps += [util_dep, dependency('gdk-pixbuf-2.0')]
endif

xd_core_inc = include_directories('.')

if xd_core_sources.length() > 0
  xd_core = static_library('xd-core',
    xd_core_sources,
    dependencies: xd_core_deps,
    include_directories: xd_core_inc,
  )
  xd_core_lib = xd_core
else
  xd_core_lib = []
endif

xd_sources = files(
  'main.c',
  'xd-app.c',
  'xd-window.c',
  'tree/fs-tree.c',
  'tree/sidebar.c',
  'ui/dots.c',
  'ui/dir-browser.c',
  'ui/panel-style.c',
  'ui/updater.c',
  'ui/branch-build-dialog.c',
  'remote/pair-dialog.c',
  'settings/agent-secrets-dialog.c',
  'settings/folder-context-dialog.c',
  'settings/folder-settings-dialog.c',
  'chat/model-picker.c',
  'chat/file-pane.c',
  'chat/diff-text.c',
  'chat/diff-view.c',
  'chat/option-picker.c',
  'chat/diff-pane.c',
  'chat/git-actions.c',
  'chat/message-row.c',
  'chat/chat-view.c',
  'chat/search-dialog.c',
)

if is_windows
  xd_sources += files('chat/terminal-panel-stub.c')
else
  xd_sources += files('chat/terminal-panel.c')
endif

xd_app_deps = [glib_dep, gio_dep, gtk_dep, adw_dep, json_dep, sqlite_dep]
if not is_windows
  xd_app_deps += [gio_unix_dep, vte_dep]
endif

executable('xd',
  xd_sources,
  dependencies: xd_app_deps,
  include_directories: xd_core_inc,
  link_with: xd_core_lib,
  win_subsystem: is_windows ? 'windows' : 'console',
  install: true,
)
