project('hy', 'c',
  version: '0.1.0',
  license: 'MIT',
  meson_version: '>= 1.0.0',
  default_options: [
    'c_std=gnu11',
    'warning_level=2',
  ],
)

app_id = 'com.restartfu.Hy'

gnome = import('gnome')
cc = meson.get_compiler('c')

glib_dep      = dependency('glib-2.0', version: '>= 2.72')
gio_dep       = dependency('gio-2.0')
gio_unix_dep  = dependency('gio-unix-2.0')
gtk_dep       = dependency('gtk4', version: '>= 4.10')
adw_dep       = dependency('libadwaita-1', version: '>= 1.4')
json_dep      = dependency('json-glib-1.0')
pango_dep     = dependency('pango')
sqlite_dep    = dependency('sqlite3')

add_project_arguments([
  '-DG_LOG_DOMAIN="hy"',
  '-DHY_APP_ID="' + app_id + '"',
  '-DHY_VERSION="' + meson.project_version() + '"',
  '-D_GNU_SOURCE',
], language: 'c')

# GObject/GTK callback signatures routinely carry parameters a given handler
# does not care about; silencing this keeps warning_level=2 useful elsewhere.
add_project_arguments(
  cc.get_supported_arguments(['-Wno-unused-parameter']),
  language: 'c',
)

subdir('data')
subdir('src')
subdir('tests')
