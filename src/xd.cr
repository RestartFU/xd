require "./xd/protocol/operation"
require "./xd/storage/sessions"
require "./xd/daemon/engine"

module Xd
  VERSION = "0.1.0"
end

if ARGV.size == 1 && {"--version", "-v"}.includes?(ARGV[0])
  puts "xd #{Xd::VERSION}"
else
  STDERR.puts "Crystal migration executable is not wired yet"
  exit 1
end
