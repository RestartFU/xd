require "./xd/version"
require "./xd/bundle_environment"
require "./xd/native_bundle"
require "./xd/protocol/operation"
require "./xd/storage/workflow_state"
require "./xd/daemon/server"
require "./xd/ui/application"
require "./xd/cli"

Xd::BundleEnvironment.prepare
exit Xd::CLI.new.run(ARGV, -> { Xd::UI.run })
