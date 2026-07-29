require "./xd/protocol/operation"
require "./xd/storage/workflow_state"
require "./xd/daemon/server"
require "./xd/cli"

module Xd
  VERSION = "0.1.0"
end

exit Xd::CLI.new.run(ARGV)
