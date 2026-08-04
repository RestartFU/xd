require "digest/sha256"
require "http/client"
require "uri"
require "../app_paths"
require "./data"

module Xd
  module Voice
    MODEL_FILE = "ggml-base.en.bin"
    MODEL_URL  = "https://huggingface.co/ggerganov/whisper.cpp/resolve/" \
                 "98aa99a0a9db05ae2342309f5096248665f7cba3/" \
                 "#{MODEL_FILE}"

    class Cancelled < Exception
    end

    class Model
      MAX_REDIRECTS = 8

      getter progress : Int32

      @client : HTTP::Client?

      def initialize(
        @path : String = File.join(
          AppPaths.data_dir,
          "speech",
          MODEL_FILE
        ),
        @url : String = MODEL_URL,
        @expected_size : UInt64 = MODEL_SIZE,
        @expected_sha256 : String = MODEL_SHA256,
        @override_path : String? = ENV["XD_VOICE_MODEL_PATH"]?,
      )
        @progress = 0
        @reported_progress = -1
        @cancelled = Atomic(Bool).new(false)
        @client = nil
        @client_mutex = Mutex.new
      end

      def find : String?
        if path = @override_path
          return File.file?(path) ? path : nil unless path.empty?
        end
        verified?(@path) ? @path : nil
      end

      def ensure_available(
        &on_progress : Int32 -> Nil
      ) : String
        @reported_progress = -1
        check_cancelled
        set_progress(0, on_progress)
        if verified?(@path)
          set_progress(100, on_progress)
          return @path
        end

        directory = File.dirname(@path)
        Dir.mkdir_p(directory, 0o700)
        temporary = File.tempfile(
          "#{MODEL_FILE}.download-",
          dir: directory
        )
        temporary_path = temporary.path
        File.chmod(temporary_path, 0o600)

        begin
          download(temporary, on_progress)
          temporary.close
          check_cancelled
          File.rename(temporary_path, @path)
          File.open(marker_path, "w", perm: 0o600) do |marker|
            marker.puts(@expected_sha256)
          end
          set_progress(100, on_progress)
          @path
        ensure
          temporary.close unless temporary.closed?
          if File.exists?(temporary_path)
            File.delete(temporary_path)
          end
          clear_client
        end
      rescue error : Cancelled
        raise error
      rescue error
        raise Cancelled.new("Speech model download was cancelled.") if cancelled?
        raise Error.new(
          error.message || "Speech model download failed."
        )
      end

      def cancel : Nil
        @cancelled.set(true)
        @client_mutex.synchronize do
          @client.try(&.close)
        rescue IO::Error
        end
      end

      private def download(
        temporary : File,
        on_progress : Int32 -> Nil,
      ) : Nil
        uri = URI.parse(@url)
        digest = Digest::SHA256.new
        total = 0_u64
        redirects = 0

        loop do
          client = HTTP::Client.new(uri)
          client.connect_timeout = 30.seconds
          client.read_timeout = 30.seconds
          @client_mutex.synchronize do
            check_cancelled
            @client = client
          end
          check_cancelled

          location : String? = nil
          downloaded = false
          begin
            client.get(uri.request_target) do |response|
              if redirect?(response.status_code)
                location = response.headers["Location"]?
                unless location
                  raise Error.new(
                    "Speech model download redirect had no location."
                  )
                end
                next
              end

              unless response.status_code.in?(200..299)
                raise Error.new(
                  "Speech model download failed with HTTP status " \
                  "#{response.status_code}."
                )
              end

              buffer = Bytes.new(128 * 1024)
              loop do
                check_cancelled
                count = response.body_io.read(buffer)
                break if count == 0
                if total + count.to_u64 > @expected_size
                  raise Error.new(
                    "Speech model download is larger than expected."
                  )
                end

                chunk = buffer[0, count]
                temporary.write(chunk)
                digest.update(chunk)
                total += count.to_u64
                percent = Math.min(
                  99,
                  (total * 100 // @expected_size).to_i
                )
                set_progress(percent, on_progress)
                Fiber.yield
              end
              downloaded = true
            end
          ensure
            clear_client(client)
            client.close
          end

          break if downloaded

          redirects += 1
          if redirects > MAX_REDIRECTS
            raise Error.new(
              "Speech model download followed too many redirects."
            )
          end
          target = location.not_nil!
          redirected = uri.resolve(target)
          unless {"http", "https"}.includes?(redirected.scheme)
            raise Error.new(
              "Speech model download redirect used an unsupported URL."
            )
          end
          if uri.scheme == "https" && redirected.scheme != "https"
            raise Error.new(
              "Speech model download redirect tried to leave HTTPS."
            )
          end
          unless redirected.host
            raise Error.new(
              "Speech model download redirect had no host."
            )
          end
          uri = redirected
        end
        check_cancelled

        unless total == @expected_size &&
               digest.final.hexstring == @expected_sha256
          raise Error.new(
            "Speech model download failed its integrity check."
          )
        end
      end

      private def redirect?(status : Int32) : Bool
        status.in?({301, 302, 303, 307, 308})
      end

      private def verified?(path : String) : Bool
        info = File.info?(path)
        return false unless info &&
                            info.file? &&
                            info.size.to_u64 == @expected_size
        return false unless File.file?(marker_path)

        File.read(marker_path).strip == @expected_sha256
      rescue File::Error
        false
      end

      private def marker_path : String
        "#{@path}.sha256"
      end

      private def set_progress(
        value : Int32,
        callback : Int32 -> Nil,
      ) : Nil
        @progress = value
        return if @reported_progress == value

        @reported_progress = value
        callback.call(value)
      end

      private def cancelled? : Bool
        @cancelled.get
      end

      private def check_cancelled : Nil
        if cancelled?
          raise Cancelled.new("Speech model download was cancelled.")
        end
      end

      private def clear_client(client : HTTP::Client? = nil) : Nil
        @client_mutex.synchronize do
          @client = nil if client.nil? || @client.same?(client)
        end
      end
    end
  end
end
