require "uri"
require "../agent/environment"
require "./settings"

module Xd
  module Workspace
    # Cloning a repository into the folder a workspace just made.
    #
    # The clone goes *into* the folder rather than beside it, because a folder
    # that is itself a checkout is a shape the tree already understands: the
    # scan stops at a directory holding `.git`, and settings resolution runs
    # the assistant in the folder when nothing else is set. So a workspace made
    # from a URL needs no repository setting to work -- it gets one anyway, so
    # the repository pane has something to point at.
    module Clone
      extend self

      # A workspace error, so a mistyped address comes back to the caller the
      # way every other bad workspace request does.
      class Error < Workspace::Error
      end

      MAX_URL = 512
      SCHEMES = {"https", "http", "ssh", "git", "file"}
      # user@host:path, which git accepts and which is not a URL at all.
      SCP = /\A[A-Za-z0-9._~-]+@[A-Za-z0-9._-]+:[^\s]+\z/

      # The address to clone from, or nil when none was given. Raises on
      # anything git should not be handed.
      def normalize(url : String?) : String?
        text = url.try(&.strip)
        return nil if text.nil? || text.empty?

        if text.bytesize > MAX_URL
          raise Error.new("That repository address is too long.")
        end
        if text.each_char.any? { |character| character.control? || character.whitespace? }
          raise Error.new("A repository address cannot contain spaces.")
        end
        # A leading dash reaches git as an option rather than an address.
        if text.starts_with?('-')
          raise Error.new("A repository address cannot start with a dash.")
        end
        return text if SCP.matches?(text)

        uri = begin
          URI.parse(text)
        rescue URI::Error
          raise Error.new(unusable)
        end
        scheme = uri.scheme.try(&.downcase)
        raise Error.new(unusable) unless scheme && SCHEMES.includes?(scheme)
        if scheme == "file"
          raise Error.new(unusable) if uri.path.empty?
        elsif (uri.host || "").empty?
          raise Error.new(unusable)
        end
        text
      end

      # Clones into an existing empty directory. Blocking: callers run it off
      # whatever thread has to keep answering.
      def run(url : String, destination : String) : Nil
        unless Dir.exists?(destination)
          raise Error.new("The workspace folder is no longer there.")
        end
        unless Dir.empty?(destination)
          raise Error.new("That workspace folder already has something in it.")
        end

        errors = IO::Memory.new
        status = Process.run(
          "git",
          ["clone", "--", url, destination],
          env: environment,
          clear_env: true,
          input: Process::Redirect::Close,
          output: Process::Redirect::Close,
          error: errors
        )
        return if status.success?

        raise Error.new(reason(errors.to_s))
      rescue error : File::Error | IO::Error
        raise Error.new("Cannot run Git: #{error.message}")
      end

      private def environment : Hash(String, String)
        env = Agent::Environment.host
        # Nobody is at a terminal: a repository that wants credentials has to
        # fail and say so rather than wait forever for an answer nobody can
        # give. A host's own askpass or ssh settings still win if it has them.
        env["GIT_TERMINAL_PROMPT"] = "0"
        env["GIT_SSH_COMMAND"] ||= "ssh -o BatchMode=yes"
        env
      end

      # Git says what went wrong on its last line; the lines above it are the
      # progress it drew getting there.
      private def reason(errors : String) : String
        line = errors.lines.map(&.strip).reject(&.empty?).last?
        return "Git could not clone that repository." unless line
        line = line.lchop("fatal: ")
        line = "#{line[0, 200]}…" if line.size > 200
        line
      end

      private def unusable : String
        "Clone from an http, https, ssh, git or file address, or from " \
        "user@host:path."
      end
    end
  end
end
