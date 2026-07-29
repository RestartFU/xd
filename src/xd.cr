require "./xd/version"
require "./xd/protocol/operation"
require "./xd/storage/workflow_state"
require "./xd/daemon/server"
require "./xd/cli"

exit Xd::CLI.new.run(ARGV)
