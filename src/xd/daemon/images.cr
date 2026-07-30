require "base64"
require "gtk4"
require "json"
require "uuid"
require "../app_paths"

module Xd
  module Daemon
    class Images
      MAX_IMAGES      = 4
      MAX_IMAGE_BYTES = 10 * 1024 * 1024
      MAX_TOTAL_BYTES = 20 * 1024 * 1024
      PNG_SIGNATURE   = Bytes[
        0x89_u8, 0x50_u8, 0x4e_u8, 0x47_u8,
        0x0d_u8, 0x0a_u8, 0x1a_u8, 0x0a_u8,
      ]

      class Error < Exception
      end

      def materialize(
        body : Hash(String, JSON::Any),
        text : String,
      ) : String
        node = body["attachments"]?
        return text unless node

        attachments = node.as_a?
        unless attachments
          raise Error.new("Message attachments must be an array.")
        end
        unless attachments.size.in?(1..MAX_IMAGES)
          raise Error.new("A message can contain between 1 and 4 images.")
        end

        directory = AppPaths.remote_pastes
        paths = [] of String
        total = 0
        message = text

        begin
          attachments.each do |node|
            attachment = node.as_h?
            mime = attachment.try(&.["mime"]?.try(&.as_s?))
            encoded = attachment.try(&.["data"]?.try(&.as_s?))
            encoded_limit = ((MAX_IMAGE_BYTES + 2) // 3) * 4
            unless mime == "image/png" &&
                   encoded &&
                   encoded.bytesize <= encoded_limit
              raise Error.new("Only PNG images up to 10 MiB can be sent.")
            end

            data = Base64.decode(encoded)
            unless png?(data) &&
                   data.size <= MAX_IMAGE_BYTES &&
                   total <= MAX_TOTAL_BYTES - data.size
              raise Error.new(
                "The attached images are invalid or too large."
              )
            end
            total += data.size

            path = File.join(directory, "paste-#{UUID.random}.png")
            File.open(path, "w", perm: 0o600) { |file| file.write(data) }
            File.chmod(path, 0o600)
            paths << path
            message += "\n" unless message.empty?
            message += "[image: #{path}]"
          end
        rescue error : Base64::Error
          remove(paths)
          raise Error.new("The attached images are invalid or too large.")
        rescue error
          remove(paths)
          raise error
        end

        message
      rescue error : File::Error | IO::Error
        raise Error.new(
          "Cannot create the remote image cache: #{error.message}"
        )
      end

      def read(
        path : String?,
        preview : Bool = false,
      ) : Hash(String, JSON::Any)
        unless path && Path[path].absolute?
          raise Error.new("image-read needs an image path.")
        end

        directory = File.realpath(AppPaths.remote_pastes)
        canonical = File.realpath(path)
        parent = File.dirname(canonical)
        info = File.info(path, follow_symlinks: false)
        unless parent == directory && info.type.file?
          raise Error.new("That image is not a remote paste.")
        end
        if info.size > MAX_IMAGE_BYTES
          raise Error.new("That remote paste is not a valid PNG.")
        end

        data = if preview
                 preview(path)
               else
                 File.read(path).to_slice
               end
        unless png?(data) && data.size <= MAX_IMAGE_BYTES
          raise Error.new("That remote paste is not a valid PNG.")
        end

        {
          "mime" => JSON::Any.new("image/png"),
          "data" => JSON::Any.new(Base64.strict_encode(data)),
        }
      rescue error : Error
        raise error
      rescue error : File::Error | IO::Error | GLib::Error
        raise Error.new("That image is not a remote paste.")
      end

      private def preview(path : String) : Bytes
        pixbuf = GdkPixbuf::Pixbuf.new_from_file_at_scale(
          path,
          640,
          360,
          true
        )
        unless pixbuf
          raise Error.new("That remote paste is not a valid PNG.")
        end

        texture = Gdk::Texture.new_for_pixbuf(pixbuf)
        texture.save_to_png_bytes.data ||
          raise Error.new("That remote paste is not a valid PNG.")
      end

      private def png?(data : Bytes) : Bool
        data.size >= PNG_SIGNATURE.size &&
          data[0, PNG_SIGNATURE.size] == PNG_SIGNATURE
      end

      private def remove(paths : Array(String)) : Nil
        paths.each { |path| File.delete?(path) }
      end
    end
  end
end
