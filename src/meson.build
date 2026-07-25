# Backend / storage / settings logic that must stay free of GTK so it can be
# exercised by the headless tests in tests/.
hy_core_sources = files(
  'settings/folder-settings.c',
  'storage/storage.c',
)

hy_core_deps = [glib_dep, gio_dep, json_dep, sqlite_dep]

hy_core_inc = include_directories('.')

if hy_core_sources.length() > 0
  hy_core = static_library('hy-core',
    hy_core_sources,
    dependencies: hy_core_deps,
    include_directories: hy_core_inc,
  )
  hy_core_lib = hy_core
else
  hy_core_lib = []
endif

hy_sources = files(
  'main.c',
  'hy-app.c',
  'hy-window.c',
  'tree/hy-node.c',
  'tree/fs-tree.c',
  'tree/sidebar.c',
  'chat/message-row.c',
  'chat/chat-view.c',
)

executable('hy',
  hy_sources,
  dependencies: [glib_dep, gio_dep, gio_unix_dep, gtk_dep, adw_dep, json_dep, sqlite_dep],
  include_directories: hy_core_inc,
  link_with: hy_core_lib,
  install: true,
)
