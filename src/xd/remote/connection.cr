require "../daemon/client"
require "./credentials"

module Xd
  module Remote
    enum ConnectionState
      Unconfigured
      Connecting
      Connected
      Offline
      Closed
    end

    record ConnectionSnapshot,
      state : ConnectionState,
      host : String?,
      port : Int32?,
      error : String?

    # One durable remote endpoint.
    #
    # It owns credentials, certificate-pinned connections and retries. Calls
    # and events still go through Daemon::Client and the server's shared Engine.
    class Connection < Daemon::Endpoint
      alias Connector = Proc(Credentials, Daemon::Client)
      alias Pairer = Proc(
        String,
        Int32,
        String,
        String,
        Daemon::RemotePairing,
      )

      @connector : Connector
      @pairer : Pairer
      @credentials : Credentials?
      @client : Daemon::Client?
      @state : ConnectionState
      @error : String?
      @generation = 0_i64
      @closed = false
      @event_subscribers =
        {} of Int64 => Proc(Hash(String, JSON::Any), Nil)
      @state_subscribers =
        {} of Int64 => Proc(ConnectionSnapshot, Nil)
      @next_subscriber = 0_i64
      @lock = Mutex.new

      def initialize(
        @credentials_file = CredentialsFile.new,
        @retry_delay = 5.seconds,
        connector : Connector? = nil,
        pairer : Pairer? = nil,
      )
        @connector = connector || ->(credentials : Credentials) {
          Daemon::Client.remote(
            credentials.host,
            credentials.port,
            credentials.token,
            credentials.fingerprint
          )
        }
        @pairer = pairer || ->(host : String, port : Int32, code : String, name : String) {
          Daemon::Client.pair_remote(host, port, code, name)
        }

        begin
          @credentials = @credentials_file.load
          @error = nil
        rescue error : CredentialsFile::Error
          @credentials = nil
          @error = error.message
        end
        @state = @credentials ? ConnectionState::Connecting : ConnectionState::Unconfigured

        if @credentials
          generation = next_generation
          spawn connect_loop(generation)
        end
      end

      def call(
        request : Hash(String, JSON::Any),
      ) : Hash(String, JSON::Any)
        client = @lock.synchronize { @client }
        unless client
          raise Daemon::Client::Error.new(
            "Remote daemon is not connected."
          )
        end
        client.call(request)
      end

      def subscribe(
        &subscriber : Hash(String, JSON::Any) ->
      ) : Int64
        @lock.synchronize do
          @next_subscriber += 1
          @event_subscribers[@next_subscriber] = subscriber
          @next_subscriber
        end
      end

      def on_state(&subscriber : ConnectionSnapshot ->) : Int64
        id, snapshot = @lock.synchronize do
          @next_subscriber += 1
          @state_subscribers[@next_subscriber] = subscriber
          {@next_subscriber, snapshot_locked}
        end
        subscriber.call(snapshot)
        id
      end

      def unsubscribe(id : Int64) : Nil
        @lock.synchronize do
          @event_subscribers.delete(id)
          @state_subscribers.delete(id)
        end
      end

      def snapshot : ConnectionSnapshot
        @lock.synchronize { snapshot_locked }
      end

      def configured? : Bool
        @lock.synchronize { !@credentials.nil? }
      end

      def connected? : Bool
        @lock.synchronize { !@client.nil? }
      end

      def pair(
        host : String,
        port : Int32,
        code : String,
        name : String = System.hostname,
        canceled : Proc(Bool)? = nil,
      ) : Credentials
        normalized_host = host.strip
        if normalized_host.empty?
          raise Credentials::Error.new("Remote host cannot be empty.")
        end
        unless 1 <= port <= 65_535
          raise Credentials::Error.new(
            "Remote port must be from 1 to 65535."
          )
        end
        normalized_code = code.gsub(/\s+/, "").upcase
        if normalized_code.empty?
          raise Daemon::Client::Error.new("Pairing code cannot be empty.")
        end

        paired = @pairer.call(
          normalized_host,
          port,
          normalized_code,
          name
        )
        credentials = Credentials.new(
          normalized_host,
          port,
          paired.token,
          paired.fingerprint
        )

        begin
          if canceled.try(&.call)
            raise Daemon::Client::Error.new("Pairing was cancelled.")
          end
          @credentials_file.save(credentials)
          replace(credentials, paired.client)
          credentials
        rescue error
          paired.client.close
          raise error
        end
      end

      def reconnect : Nil
        generation : Int64? = nil
        snapshot : ConnectionSnapshot? = nil

        @lock.synchronize do
          return if @closed || @credentials.nil? || @client

          @generation += 1
          generation = @generation
          @state = ConnectionState::Connecting
          @error = nil
          snapshot = snapshot_locked
        end
        publish_state(snapshot.not_nil!)
        spawn connect_loop(generation.not_nil!)
      end

      def forget : Nil
        @credentials_file.clear
        client : Daemon::Client? = nil
        snapshot : ConnectionSnapshot? = nil

        @lock.synchronize do
          return if @closed

          @generation += 1
          client = @client
          @client = nil
          @credentials = nil
          @state = ConnectionState::Unconfigured
          @error = nil
          snapshot = snapshot_locked
        end
        client.try(&.close)
        publish_state(snapshot.not_nil!)
      end

      def close : Nil
        client : Daemon::Client? = nil
        snapshot : ConnectionSnapshot? = nil

        @lock.synchronize do
          return if @closed

          @closed = true
          @generation += 1
          client = @client
          @client = nil
          @state = ConnectionState::Closed
          snapshot = snapshot_locked
        end
        client.try(&.close)
        publish_state(snapshot.not_nil!)
      end

      private def replace(
        credentials : Credentials,
        client : Daemon::Client,
      ) : Nil
        previous : Daemon::Client? = nil
        generation = @lock.synchronize do
          if @closed
            raise Daemon::Client::Error.new(
              "Remote connection is closed."
            )
          end

          @generation += 1
          previous = @client
          @credentials = credentials
          @generation
        end
        previous.try(&.close)
        accept(client, credentials, generation)
      end

      private def connect_loop(generation : Int64) : Nil
        loop do
          credentials : Credentials? = nil
          connecting : ConnectionSnapshot? = nil
          active = @lock.synchronize do
            next false if @closed || @generation != generation

            loaded = @credentials
            next false unless loaded

            credentials = loaded
            @state = ConnectionState::Connecting
            connecting = snapshot_locked
            true
          end
          return unless active
          publish_state(connecting.not_nil!)

          begin
            loaded = credentials.not_nil!
            client = @connector.call(loaded)
            unless accept(client, loaded, generation)
              client.close
            end
            return
          rescue error
            offline : ConnectionSnapshot? = nil
            active = @lock.synchronize do
              next false if @closed || @generation != generation

              @state = ConnectionState::Offline
              @error = error.message || error.class.name
              offline = snapshot_locked
              true
            end
            return unless active
            publish_state(offline.not_nil!)
            sleep @retry_delay
          end
        end
      end

      private def accept(
        client : Daemon::Client,
        credentials : Credentials,
        generation : Int64,
      ) : Bool
        snapshot : ConnectionSnapshot? = nil
        accepted = @lock.synchronize do
          next false if @closed || @generation != generation

          @credentials = credentials
          @client = client
          @state = ConnectionState::Connected
          @error = nil
          snapshot = snapshot_locked
          true
        end
        return false unless accepted

        client.subscribe { |event| publish_event(event) }
        client.on_disconnect do |message|
          disconnected(client, generation, message)
        end
        publish_state(snapshot.not_nil!)
        true
      end

      private def disconnected(
        client : Daemon::Client,
        generation : Int64,
        message : String,
      ) : Nil
        retry_generation : Int64? = nil
        snapshot : ConnectionSnapshot? = nil

        active = @lock.synchronize do
          next false if @closed || @generation != generation
          next false unless @client.same?(client)

          @client = nil
          @state = ConnectionState::Offline
          @error = message
          @generation += 1
          retry_generation = @generation if @credentials
          snapshot = snapshot_locked
          true
        end
        return unless active

        publish_state(snapshot.not_nil!)
        if retry = retry_generation
          spawn connect_loop(retry)
        end
      end

      private def publish_event(
        event : Hash(String, JSON::Any),
      ) : Nil
        subscribers = @lock.synchronize do
          @event_subscribers.values.dup
        end
        subscribers.each { |subscriber| subscriber.call(event) }
      end

      private def publish_state(snapshot : ConnectionSnapshot) : Nil
        subscribers = @lock.synchronize do
          @state_subscribers.values.dup
        end
        subscribers.each { |subscriber| subscriber.call(snapshot) }
      end

      private def next_generation : Int64
        @lock.synchronize do
          @generation += 1
          @generation
        end
      end

      private def snapshot_locked : ConnectionSnapshot
        ConnectionSnapshot.new(
          @state,
          @credentials.try(&.host),
          @credentials.try(&.port),
          @error
        )
      end
    end
  end
end
