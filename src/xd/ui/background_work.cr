module Xd
  module UI
    # Small shared pool for CPU-heavy UI preparation. Bounded queue prevents
    # refresh storms from creating one native thread per stale request.
    module BackgroundWork
      extend self

      WORKERS    =   3
      QUEUE_SIZE = 128

      @@queue = Channel(Proc(Nil)).new(QUEUE_SIZE)
      @@mutex = Mutex.new
      @@started = false

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
          WORKERS.times do
            Thread.new do
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
