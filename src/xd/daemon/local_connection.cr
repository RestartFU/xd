require "./client"

module Xd
  module Daemon
    # Durable platform-local endpoint.
    #
    # A desktop window keeps this object while the underlying Unix socket or
    # authenticated Windows loopback client reconnects. Event subscriptions
    # therefore survive transport replacement exactly like paired remotes.
    class LocalConnection < Endpoint
      alias Connector = Proc(String, Client)

      @connector : Connector
      @client : Client?
      @subscribers = {} of Int64 => Proc(Hash(String, JSON::Any), Nil)
      @next_subscriber = 0_i64
      @generation = 0_i64
      @closed = false
      @lock = Mutex.new

      def initialize(
        @path : String,
        initial_client : Client? = nil,
        @retry_delay : Time::Span = 250.milliseconds,
        connector : Connector? = nil,
      )
        @connector = connector || ->(path : String) { Client.local(path) }
        @client = initial_client
        if client = initial_client
          attach(client, @generation)
        else
          reconnect
        end
      end

      def call(
        request : Hash(String, JSON::Any),
      ) : Hash(String, JSON::Any)
        client = @lock.synchronize { @client }
        unless client
          raise Client::Error.new("Local daemon is reconnecting.")
        end
        client.call(request)
      end

      def subscribe(
        &subscriber : Hash(String, JSON::Any) ->
      ) : Int64
        @lock.synchronize do
          @next_subscriber += 1
          @subscribers[@next_subscriber] = subscriber
          @next_subscriber
        end
      end

      def unsubscribe(id : Int64) : Nil
        @lock.synchronize { @subscribers.delete(id) }
      end

      def connected? : Bool
        @lock.synchronize { !@client.nil? }
      end

      def close : Nil
        client = @lock.synchronize do
          return if @closed

          @closed = true
          @generation += 1
          current = @client
          @client = nil
          current
        end
        client.try(&.close)
      end

      private def reconnect : Nil
        generation = @lock.synchronize do
          return if @closed

          @generation += 1
          @generation
        end
        spawn connect_loop(generation)
      end

      private def connect_loop(generation : Int64) : Nil
        loop do
          active = @lock.synchronize do
            !@closed && @generation == generation
          end
          return unless active

          begin
            client = @connector.call(@path)
            unless attach(client, generation)
              client.close
            end
            return
          rescue
            sleep @retry_delay
          end
        end
      end

      private def attach(client : Client, generation : Int64) : Bool
        accepted = @lock.synchronize do
          next false if @closed || @generation != generation

          @client = client
          true
        end
        return false unless accepted

        client.subscribe { |event| publish(event) }
        client.on_disconnect do |_message|
          disconnected(client, generation)
        end
        true
      end

      private def disconnected(
        client : Client,
        generation : Int64,
      ) : Nil
        retry_generation : Int64? = nil
        active = @lock.synchronize do
          next false if @closed || @generation != generation
          next false unless @client.same?(client)

          @client = nil
          @generation += 1
          retry_generation = @generation
          true
        end
        return unless active

        spawn connect_loop(retry_generation.not_nil!)
      end

      private def publish(event : Hash(String, JSON::Any)) : Nil
        subscribers = @lock.synchronize { @subscribers.values.dup }
        subscribers.each do |subscriber|
          begin
            subscriber.call(event)
          rescue error
            STDERR.puts(
              "xd: local event subscriber failed: #{error.message}"
            )
          end
        end
      end
    end
  end
end
