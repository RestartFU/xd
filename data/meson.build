gschema_file = 'com.restartfu.Hy.gschema.xml'

install_data(gschema_file,
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

# Compile the schema inside the build tree so the app can be run uninstalled
# with GSETTINGS_SCHEMA_DIR=<builddir>/data.
gnome.compile_schemas(depend_files: files(gschema_file))

desktop_file = configure_file(
  input: 'com.restartfu.Hy.desktop.in',
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
    'icons/hicolor/scalable/apps/com.restartfu.Hy.svg',
    'icons/hicolor/scalable/apps/hy-backend-claude.svg',
    'icons/hicolor/scalable/apps/hy-backend-codex.svg',
  ],
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
)
