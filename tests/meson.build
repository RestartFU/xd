# Headless tests. They deliberately avoid GTK so they run in a plain container.

test_env = environment()
test_env.set('G_TEST_SRCDIR', meson.current_source_dir())
test_env.set('G_DEBUG', 'gc-friendly')

hy_tests = {
  'storage': 'test-storage.c',
  'backend': 'test-backend.c',
  'session': 'test-session.c',
  'settings': 'test-settings.c',
  'markdown': 'test-markdown.c',
  'ask': 'test-ask.c',
}

foreach name, source : hy_tests
  exe = executable('test-' + name,
    files(source),
    dependencies: hy_core_deps,
    include_directories: hy_core_inc,
    link_with: hy_core_lib,
  )

  test(name, exe, env: test_env, args: ['--tap'], protocol: 'tap')
endforeach
