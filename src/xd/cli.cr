require "option_parser"
require "./app_paths"
require "./daemon/certificate"
require "./daemon/client"
require "./daemon/server"
require "./native_bundle"
require "./workspace/service"

module Xd
  class CLI
    class UsageError < Exception
    end

    class ServeOptions
      property port = 4001
      property bind = "::"
      property pair = false
      property device_name : String? = nil
      property root = AppPaths.workspaces
      property database = AppPaths.database
      property socket = AppPaths.local_socket
      property certificate = AppPaths.certificate
      property private_key = AppPaths.private_key
    end

    def initialize(
      @output : IO = STDOUT,
      @error : IO = STDERR,
    )
    end

    def run(
      arguments : Array(String),
      desktop : Proc(Int32)? = nil,
    ) : Int32
      if arguments == ["--version"] || arguments == ["-v"]
        @output.puts "xd #{Xd.version_string}"
        return 0
      end

      # Native packagers use this private probe to reject non-Crystal binaries.
      # Keep it machine-readable; it is a bundle contract, not user-facing UI.
      if arguments == ["--bundle-runtime"]
        @output.puts "crystal"
        return 0
      end

      if arguments.size == 3 &&
         arguments.first? == "--validate-native-bundle"
        platform = NativeBundle.parse_platform(arguments[1])
        unless platform
          @error.puts "xd: native bundle platform must be macos or windows"
          return 2
        end
        missing = NativeBundle.validate(arguments[2], platform)
        if missing.empty?
          @output.puts "native bundle: ok"
          return 0
        end
        missing.each { |item| @error.puts "xd: native bundle missing #{item}" }
        return 1
      end

      if arguments.first? == "serve"
        return serve(parse_serve(arguments.skip(1)))
      end

      if desktop
        return desktop.call
      end

      @error.puts "xd desktop client is unavailable in this build"
      1
    rescue error : UsageError
      @error.puts error.message
      2
    rescue error : Daemon::DeviceStoreError | Daemon::Client::Error | Workspace::Error | Daemon::Certificate::Error | IO::Error | OpenSSL::Error
      @error.puts "xd: #{error.message}"
      1
    end

    def parse_serve(arguments : Array(String)) : ServeOptions
      options = ServeOptions.new
      parser = OptionParser.new do |command|
        command.banner = "usage: xd serve [options]"
        command.on("--port PORT", "TLS port (default 4001)") do |value|
          options.port = parse_port(value)
        end
        command.on("--bind ADDRESS", "TLS bind address") do |value|
          options.bind = value
        end
        command.on("--pair", "print a five-minute pairing code") do
          options.pair = true
        end
        command.on(
          "--device-name NAME",
          "owner name for the device paired by --pair"
        ) do |value|
          options.device_name = value
        end
        command.on("--root DIR", "workspace root") do |value|
          options.root = File.expand_path(value)
        end
        command.on("--database FILE", "chat database") do |value|
          options.database = File.expand_path(value)
        end
        command.on("--socket FILE", "local IPC endpoint") do |value|
          options.socket = File.expand_path(value)
        end
        command.on("--certificate FILE", "TLS certificate") do |value|
          options.certificate = File.expand_path(value)
        end
        command.on("--private-key FILE", "TLS private key") do |value|
          options.private_key = File.expand_path(value)
        end
        command.on("-h", "--help", "show this help") do
          raise UsageError.new(command.to_s)
        end
      end
      parser.parse(arguments)
      if options.pair && options.device_name.try(&.strip).to_s.empty?
        raise UsageError.new(
          "--pair requires --device-name so the owner can name the device"
        )
      end
      options
    rescue error : OptionParser::Exception
      raise UsageError.new("#{error.message}\n#{parser}")
    end

    private def serve(options : ServeOptions) : Int32
      return 0 if options.pair && pair_with_running_daemon(options)

      store = Storage::Store.new(options.database)
      workspaces = Workspace::Service.new(options.root, store)
      engine = Daemon::Engine.new(store, workspaces)
      server = Daemon::Server.new(engine)
      engine.peer_listener = ->(bind : String, port : Int32) {
        Daemon::Certificate.ensure_pair(
          options.certificate,
          options.private_key
        )
        actual_port = server.listen_remote(
          bind,
          port,
          options.certificate,
          options.private_key
        )
        store.save_remote_listener(bind, actual_port)
        actual_port
      }

      begin
        store.clear_daemon_working
        Daemon::Certificate.ensure_pair(
          options.certificate,
          options.private_key
        )
        server.listen_local(options.socket)
        port = server.listen_remote(
          options.bind,
          options.port,
          options.certificate,
          options.private_key
        )
        store.save_remote_listener(options.bind, port)

        @output.puts(
          "xd serve: #{Xd.version_string}, listening on #{port}, " \
          "workspaces at #{options.root}"
        )
        if options.pair
          @output.puts(
            "pairing code (5 minutes, one use): " \
            "#{engine.arm_pairing(5.minutes, options.device_name.not_nil!)}"
          )
        end
        @output.flush

        stopped = Channel(Nil).new
        Signal::INT.trap { stopped.send(nil) }
        Signal::TERM.trap { stopped.send(nil) }
        stopped.receive
        0
      ensure
        server.close
        engine.close
        store.close
      end
    end

    private def pair_with_running_daemon(options : ServeOptions) : Bool
      client = begin
        Daemon::Client.local(options.socket, request_timeout: 5.seconds)
      rescue Daemon::Client::Error
        return false
      end

      begin
        response = client.call({
          "op"   => JSON::Any.new("peer-pairing"),
          "bind" => JSON::Any.new(options.bind),
          "port" => JSON::Any.new(options.port.to_i64),
          "name" => JSON::Any.new(options.device_name.not_nil!),
        })
        @output.puts(
          "xd serve: attached to running daemon, listening on " \
          "#{response["port"].as_i64}"
        )
        @output.puts(
          "pairing code (5 minutes, one use): #{response["code"].as_s}"
        )
        @output.flush
        true
      ensure
        client.close
      end
    end

    private def parse_port(value : String) : Int32
      port = value.to_i?
      unless port && port >= 0 && port <= UInt16::MAX
        raise UsageError.new("Invalid port: #{value}")
      end
      port
    end
  end
end
