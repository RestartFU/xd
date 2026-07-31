module Xd
  module Daemon
    # Keeps embedded daemon fibers off GTK's single-threaded default context.
    # Two schedulers preserve control requests while one worker is blocked in
    # filesystem, SQLite, Git, agent, or JSON work.
    class ExecutionHost
      WORKERS = 2

      def initialize(name : String = "xd embedded daemon")
        @context = Fiber::ExecutionContext::Parallel.new(name, WORKERS)
      end

      def spawn(*, name : String? = nil, &block : ->) : Fiber
        @context.spawn(name: name, &block)
      end
    end
  end
end
