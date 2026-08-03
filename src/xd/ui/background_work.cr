module Xd
  module UI
    # One shared worker for CPU-heavy UI preparation. UI rendering must never
    # consume every laptop core while GTK is also laying out a long transcript.
    # The bounded queue prevents refresh storms from retaining arbitrary data.
    module BackgroundWork
      extend self

      WORKERS = 1
      # Jobs may retain 8–10 MiB diff, image, or recording payloads. A deep
      # queue turns a brief refresh burst into hundreds of MiB of live memory.
      QUEUE_SIZE = 16

      @@queue = Channel(Proc(Nil)).new(QUEUE_SIZE)
      @@mutex = Mutex.new
      @@started = false
      @@context : Fiber::ExecutionContext::Parallel?

      def submit(&work : -> Nil) : Bool
        start
        select
        when @@queue.send(work)
          true
        else
          false
        end
      end

      private def start : Nil
        @@mutex.synchronize do
          return if @@started

          @@started = true
          context = Fiber::ExecutionContext::Parallel.new(
            "xd background UI",
            WORKERS
          )
          @@context = context
          WORKERS.times do |index|
            context.spawn(name: "xd background UI #{index + 1}") do
              loop do
                begin
                  @@queue.receive.call
                rescue Channel::ClosedError
                  break
                rescue error
                  STDERR.puts(
                    "xd: background UI work failed: #{error.message}"
                  )
                end
              end
            end
          end
        end
      end
    end
  end
end
