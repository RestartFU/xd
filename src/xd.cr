require "./xd/version"
require "./xd/protocol/operation"
require "./xd/storage/workflow_state"
require "./xd/daemon/server"
require "./xd/ui/application"
require "./xd/cli"

exit Xd::CLI.new.run(ARGV, -> { Xd::UI.run })
