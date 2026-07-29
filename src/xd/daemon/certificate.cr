require "uuid"

module Xd
  module Daemon
    module Certificate
      extend self

      class Error < Exception
      end

      def ensure_pair(
        certificate_path : String,
        private_key_path : String,
      ) : Nil
        temporary_certificate : String? = nil
        temporary_key : String? = nil

        if File.file?(certificate_path) && File.file?(private_key_path)
          File.chmod(private_key_path, 0o600)
          return
        end

        directory = File.dirname(certificate_path)
        Dir.mkdir_p(directory, 0o700)
        suffix = UUID.random.to_s
        temporary_certificate = "#{certificate_path}.#{suffix}.tmp"
        temporary_key = "#{private_key_path}.#{suffix}.tmp"
        errors = IO::Memory.new

        status = Process.run(
          openssl_executable,
          [
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            temporary_key,
            "-out",
            temporary_certificate,
            "-days",
            "3650",
            "-nodes",
            "-subj",
            "/CN=xd",
          ],
          output: Process::Redirect::Close,
          error: errors
        )
        unless status.success?
          raise Error.new(
            "Cannot create the server certificate: #{errors.to_s.strip}"
          )
        end

        File.chmod(temporary_key, 0o600)
        File.chmod(temporary_certificate, 0o644)
        File.delete?(certificate_path)
        File.delete?(private_key_path)
        File.rename(temporary_certificate, certificate_path)
        File.rename(temporary_key, private_key_path)
      rescue error : File::Error | IO::Error
        raise Error.new("Cannot create the server certificate: #{error.message}")
      ensure
        File.delete?(temporary_certificate) if temporary_certificate
        File.delete?(temporary_key) if temporary_key
      end

      private def openssl_executable : String
        if configured = ENV["XD_OPENSSL"]?
          return configured
        end

        if executable = Process.executable_path
          bundled = File.expand_path(
            File.join(File.dirname(executable), "..", "libexec", "openssl")
          )
          if info = File.info?(bundled)
            if info.type.file? && info.permissions.owner_execute?
              return bundled
            end
          end
        end
        "openssl"
      end
    end
  end
end
