gschema_file = configure_file(
  input: 'com.restartfu.Xd.gschema.xml.in',
  output: app_id + '.gschema.xml',
  configuration: {'APP_ID': app_id, 'SETTINGS_PATH': settings_path},
  install: true,
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

# Compile the schema inside the build tree so the app can be run uninstalled
# with GSETTINGS_SCHEMA_DIR=<builddir>/data.
gnome.compile_schemas(depend_files: gschema_file)

desktop_file = configure_file(
  input: 'com.restartfu.Xd.desktop.in',
  output: app_id + '.desktop',
  configuration: {'APP_ID': app_id, 'APP_NAME': app_name},
  install: true,
  install_dir: get_option('datadir') / 'applications',
)

desktop_validate = find_program('desktop-file-validate', required: false)
if desktop_validate.found()
  test('validate-desktop-file', desktop_validate, args: [desktop_file])
endif

# Named after the application id, which is what the desktop entry asks for --
# so a nightly's icon is its own file and the two never collide.
install_data('icons/hicolor/scalable/apps/com.restartfu.Xd.svg',
  rename: app_id + '.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
)

install_data('icons/hicolor/scalable/apps/xd-backend-claude.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
)

# The OpenAI mark is monochrome, so it ships symbolic and GTK paints it in the
# theme's foreground colour instead of it vanishing in light or dark.
install_data('icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)

install_data('icons/hicolor/symbolic/apps/xd-backend-cerebras-symbolic.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)
