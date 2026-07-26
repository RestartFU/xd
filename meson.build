project('xd', 'c',
  version: '0.1.0',
  license: 'MIT',
  meson_version: '>= 1.0.0',
  default_options: [
    'c_std=gnu11',
    'warning_level=2',
  ],
)

#
# What this build is.
#
# A nightly installs beside a release rather than over it: its own application
# id, its own settings, its own data directory and therefore its own
# workspaces and chats. The settings path of the release build is left exactly
# as it was -- it is where existing installs keep everything, including the
# token a paired device is known by.
#
profile = get_option('profile')

if profile == 'nightly'
  app_id        = 'com.restartfu.Xd.Nightly'
  app_name      = 'xd (Nightly)'
  app_data_name = 'xd-nightly'
  settings_path = '/com/restartfu/XdNightly/'
else
  app_id        = 'com.restartfu.Xd'
  app_name      = 'xd'
  app_data_name = 'xd'
  settings_path = '/com/restartfu/Hy/'
endif

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
vte_dep       = dependency('vte-2.91-gtk4')

add_project_arguments([
  '-DG_LOG_DOMAIN="xd"',
  '-DXD_APP_ID="' + app_id + '"',
  '-DXD_APP_NAME="' + app_name + '"',
  '-DXD_DATA_NAME="' + app_data_name + '"',
  '-DXD_VERSION="' + meson.project_version() + '"',
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
