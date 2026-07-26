# Headless tests. They deliberately avoid GTK so they run in a plain container.

test_env = environment()
test_env.set('G_TEST_SRCDIR', meson.current_source_dir())
test_env.set('G_DEBUG', 'gc-friendly')

xd_tests = {
  'storage': 'test-storage.c',
  'backend': 'test-backend.c',
  'session': 'test-session.c',
  'settings': 'test-settings.c',
  'markdown': 'test-markdown.c',
  'ask': 'test-ask.c',
  'worktree': 'test-worktree.c',
}

if not is_windows
  xd_tests += {'remote': 'test-remote.c'}
endif

foreach name, source : xd_tests
  exe = executable('test-' + name,
    files(source),
    dependencies: xd_core_deps,
    include_directories: xd_core_inc,
    link_with: xd_core_lib,
  )

  # The remote suite talks over real sockets with real TLS, mints certificates
  # and waits on file monitors; meson's 30 seconds is meant for unit tests.
  test(name, exe,
    env: test_env,
    args: ['--tap'],
    protocol: 'tap',
    timeout: name == 'remote' ? 180 : 30,
  )
endforeach
