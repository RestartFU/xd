require "option_parser"
require "./app_paths"
require "./daemon/certificate"
require "./daemon/server"
require "./workspace/service"

module Xd
  class CLI
    class UsageError < Exception
    end

    class ServeOptions
      property port = 4001
      property bind = "::"
      property pair = false
      property root = AppPaths.workspaces
      property database = AppPaths.database
      property socket = AppPaths.local_socket
      property certificate = AppPaths.certificate
      property private_key = AppPaths.private_key
      property auto_update = false
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
    rescue error : Daemon::DeviceStoreError | Workspace::Error | Daemon::Certificate::Error | IO::Error | OpenSSL::Error
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
        command.on("--root DIR", "workspace root") do |value|
          options.root = File.expand_path(value)
        end
        command.on("--database FILE", "chat database") do |value|
          options.database = File.expand_path(value)
        end
        command.on("--socket FILE", "local IPC socket") do |value|
          options.socket = File.expand_path(value)
        end
        command.on("--certificate FILE", "TLS certificate") do |value|
          options.certificate = File.expand_path(value)
        end
        command.on("--private-key FILE", "TLS private key") do |value|
          options.private_key = File.expand_path(value)
        end
        command.on("--auto-update", "update an installed daemon") do
          options.auto_update = true
        end
        command.on("-h", "--help", "show this help") do
          raise UsageError.new(command.to_s)
        end
      end
      parser.parse(arguments)
      options
    rescue error : OptionParser::Exception
      raise UsageError.new("#{error.message}\n#{parser}")
    end

    private def serve(options : ServeOptions) : Int32
      if options.auto_update
        raise UsageError.new(
          "xd serve: --auto-update is not migrated yet"
        )
      end

      store = Storage::Store.new(options.database)
      workspaces = Workspace::Service.new(options.root, store)
      engine = Daemon::Engine.new(store, workspaces)
      server = Daemon::Server.new(engine)

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

        @output.puts(
          "xd serve: #{Xd.version_string}, listening on #{port}, " \
          "workspaces at #{options.root}"
        )
        if options.pair
          @output.puts(
            "pairing code (5 minutes, one use): " \
            "#{engine.arm_pairing(5.minutes)}"
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

    private def parse_port(value : String) : Int32
      port = value.to_i?
      unless port && port >= 0 && port <= UInt16::MAX
        raise UsageError.new("Invalid port: #{value}")
      end
      port
    end
  end
end
