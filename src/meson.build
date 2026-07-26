# Backend / storage / settings logic that must stay free of GTK so it can be
# exercised by the headless tests in tests/.
xd_core_sources = files(
  'settings/folder-settings.c',
  'settings/settings-resolver.c',
  'util/markdown.c',
  'util/ask-block.c',
  'util/git-info.c',
  'util/worktree.c',
  'util/host-launch.c',
  'util/app-paths.c',
  'remote/server.c',
  'remote/client.c',
  'remote/remote-tree.c',
  'remote/turn.c',
  'remote/terminal.c',
  'tree/xd-node.c',
  'storage/storage.c',
  'backend/backend.c',
  'backend/claude-backend.c',
  'backend/codex-backend.c',
  # No GTK here either, so the subprocess plumbing can be tested headlessly.
  'chat/chat-session.c',
  'chat/handover.c',
  'chat/chat-title.c',
)

# Pango, but not GTK: the Markdown converter validates the markup it produces,
# which needs the parser and nothing else.
xd_core_deps = [glib_dep, gio_dep, json_dep, sqlite_dep, pango_dep, util_dep]

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
  'ui/updater.c',
  'remote/pair-dialog.c',
  'settings/folder-settings-dialog.c',
  'chat/model-picker.c',
  'chat/option-picker.c',
  'chat/diff-pane.c',
  'chat/git-actions.c',
  'chat/terminal-panel.c',
  'chat/message-row.c',
  'chat/chat-view.c',
  'chat/search-dialog.c',
)

executable('xd',
  xd_sources,
  dependencies: [glib_dep, gio_dep, gio_unix_dep, gtk_dep, adw_dep, json_dep, sqlite_dep, vte_dep],
  include_directories: xd_core_inc,
  link_with: xd_core_lib,
  install: true,
)
