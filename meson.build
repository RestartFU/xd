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
#
# A branch is tried by building it: the nightly builds it from source and
# installs the result over itself, so what comes out is a nightly like any
# other and there is no third kind of build to keep working.
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
is_windows = host_machine.system() == 'windows'
is_macos = host_machine.system() == 'darwin'

#
# Every platform hosts the daemon.
#
# The client is about to stop having a second way to do anything: it adopts or
# spawns a daemon and talks to it, locally as well as remotely, so a machine
# that cannot serve is a machine xd cannot run on. The daemon's own code is
# already portable -- server.c and turn.c use nothing Unix-only -- and a shell
# is the single exception, which Windows takes as a stub the way its client
# already does.
#
has_server = true
has_daemon_terminal = not is_windows
has_voice = not is_windows and not is_macos

glib_dep      = dependency('glib-2.0', version: '>= 2.72')
gio_dep       = dependency('gio-2.0')
gtk_dep       = dependency('gtk4', version: '>= 4.10')
adw_dep       = dependency('libadwaita-1', version: '>= 1.4')
json_dep      = dependency('json-glib-1.0')
pango_dep     = dependency('pango')
sqlite_dep    = dependency('sqlite3')
cmark_dep     = dependency('libcmark', version: '>= 0.30')

if has_voice
  pulse_simple_dep = dependency('libpulse-simple')
  soup_dep         = dependency('libsoup-3.0')
  whisper_dep      = dependency('whisper', version: '>= 1.9.1')
endif

if is_windows
  gio_unix_dep = declare_dependency()
  vte_dep      = declare_dependency()
else
  gio_unix_dep = dependency('gio-unix-2.0')
  vte_dep      = dependency('vte-2.91-gtk4')
endif

if has_daemon_terminal
  util_dep     = cc.find_library('util')
else
  util_dep     = declare_dependency()
endif

#
# What --version says.
#
# The project version alone cannot answer "is this the build I just
# installed?", which is the only question anyone asks a nightly -- so it
# carries the commit it was built from. Passed in rather than read here: the
# build happens in a container with the sources but no repository.
#
version_string = meson.project_version()

if profile == 'nightly'
  version_string += '-nightly'
endif

if get_option('commit') != ''
  version_string += ' (' + get_option('commit') + ')'
endif

add_project_arguments([
  '-DG_LOG_DOMAIN="xd"',
  '-DXD_VERSION_STRING="' + version_string + '"',
  '-DXD_COMMIT="' + get_option('commit') + '"',
  '-DXD_CHANNEL="' + (profile == 'default' ? 'release' : profile) + '"',
  '-DXD_REPO="RestartFU/xd"',
  '-DXD_APP_ID="' + app_id + '"',
  '-DXD_APP_NAME="' + app_name + '"',
  '-DXD_DATA_NAME="' + app_data_name + '"',
  '-DXD_VERSION="' + meson.project_version() + '"',
  '-DXD_HAS_TERMINAL=' + (is_windows ? '0' : '1'),
  '-DXD_HAS_SERVER=' + (has_server ? '1' : '0'),
  '-DXD_HAS_VOICE=' + (has_voice ? '1' : '0'),
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
