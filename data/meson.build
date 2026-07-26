gschema_file = 'com.restartfu.Xd.gschema.xml'

install_data(gschema_file,
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

# Compile the schema inside the build tree so the app can be run uninstalled
# with GSETTINGS_SCHEMA_DIR=<builddir>/data.
gnome.compile_schemas(depend_files: files(gschema_file))

desktop_file = configure_file(
  input: 'com.restartfu.Xd.desktop.in',
  output: app_id + '.desktop',
  configuration: {'APP_ID': app_id},
  install: true,
  install_dir: get_option('datadir') / 'applications',
)

desktop_validate = find_program('desktop-file-validate', required: false)
if desktop_validate.found()
  test('validate-desktop-file', desktop_validate, args: [desktop_file])
endif

install_data(
  [
    'icons/hicolor/scalable/apps/com.restartfu.Xd.svg',
    'icons/hicolor/scalable/apps/xd-backend-claude.svg',
  ],
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
)

# The OpenAI mark is monochrome, so it ships symbolic and GTK paints it in the
# theme's foreground colour instead of it vanishing in light or dark.
install_data('icons/hicolor/symbolic/apps/xd-backend-codex-symbolic.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)
