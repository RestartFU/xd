require "gtk4"

module Xd
  module UI
    # File attachment decoding stays outside GTK. Only copied pixels cross back
    # to the main loop, avoiding GObject ownership across worker threads.
    module ImageAttachment
      extend self

      record Pixels,
        width : Int32,
        height : Int32,
        stride : UInt64,
        format : Gdk::MemoryFormat,
        data : Bytes

      record Prepared,
        png : Bytes,
        preview : Pixels

      class Error < Exception
      end

      MAX_IMAGE_BYTES      = 10 * 1024 * 1024
      MAX_WIDTH            =            1920
      MAX_HEIGHT           =            1920
      PREVIEW_WIDTH        =             168
      PREVIEW_HEIGHT       =              96
      MAX_SOURCE_DIMENSION =          32_768
      MAX_SOURCE_PIXELS    = 100_000_000_i64
      READ_CHUNK_BYTES     = 64 * 1024

      def prepare_file(
        path : String,
        max_bytes : Int32 = MAX_IMAGE_BYTES,
      ) : Prepared
        source = read_bounded(path, max_bytes)
        prepare(source, max_bytes)
      rescue error : Error
        raise error
      rescue error : File::Error | IO::Error | GLib::Error
        raise Error.new(error.message || "Cannot attach that image.")
      end

      def prepare(
        source : Bytes,
        max_bytes : Int32 = MAX_IMAGE_BYTES,
      ) : Prepared
        raise Error.new("Image file is empty.") if source.empty?
        if source.size > max_bytes
          raise Error.new(
            "Each source image must be 10 MiB or smaller."
          )
        end

        pixbuf = decode(source, MAX_WIDTH, MAX_HEIGHT)
        oriented = pixbuf.apply_embedded_orientation || pixbuf
        png = encode_png(oriented)
        if png.size > max_bytes
          raise Error.new("Encoded image must be 10 MiB or smaller.")
        end

        preview_width, preview_height = scaled_size(
          oriented.width,
          oriented.height,
          PREVIEW_WIDTH,
          PREVIEW_HEIGHT
        )
        preview = if preview_width == oriented.width &&
                     preview_height == oriented.height
                    oriented
                  else
                    oriented.scale_simple(
                      preview_width,
                      preview_height,
                      GdkPixbuf::InterpType::Bilinear
                    ) || raise Error.new(
                      "Image preview could not be created."
                    )
                  end

        Prepared.new(png, copy_pixels(preview))
      rescue error : Error
        raise error
      rescue error : IO::Error | GLib::Error
        raise Error.new(error.message || "Cannot attach that image.")
      end

      def texture(pixels : Pixels) : Gdk::Texture
        bytes = GLib::Bytes.new(
          pixels.data.to_unsafe,
          pixels.data.size
        )
        Gdk::MemoryTexture.new(
          pixels.width,
          pixels.height,
          pixels.format,
          bytes,
          pixels.stride
        )
      end

      private def read_bounded(
        path : String,
        max_bytes : Int32,
      ) : Bytes
        output = IO::Memory.new
        buffer = Bytes.new(READ_CHUNK_BYTES)
        total = 0

        File.open(path) do |file|
          loop do
            count = file.read(buffer)
            break if count == 0
            total += count
            if total > max_bytes
              raise Error.new(
                "Each source image must be 10 MiB or smaller."
              )
            end
            output.write(buffer[0, count])
          end
        end
        raise Error.new("Image file is empty.") if total == 0
        output.to_slice.dup
      end

      private def decode(
        source : Bytes,
        max_width : Int32,
        max_height : Int32,
      ) : GdkPixbuf::Pixbuf
        invalid_dimensions = false
        loader = GdkPixbuf::PixbufLoader.new
        loader.size_prepared_signal.connect do |width, height|
          if invalid_size?(width, height)
            invalid_dimensions = true
            loader.set_size(1, 1)
          else
            target_width, target_height = scaled_size(
              width,
              height,
              max_width,
              max_height
            )
            loader.set_size(target_width, target_height)
          end
        end
        loader.write(source)
        loader.close
        if invalid_dimensions
          raise Error.new("Image dimensions are too large.")
        end
        loader.pixbuf ||
          raise Error.new("Image decoder returned no pixels.")
      end

      private def invalid_size?(width : Int32, height : Int32) : Bool
        width <= 0 ||
          height <= 0 ||
          width > MAX_SOURCE_DIMENSION ||
          height > MAX_SOURCE_DIMENSION ||
          width.to_i64 * height.to_i64 > MAX_SOURCE_PIXELS
      end

      private def scaled_size(
        width : Int32,
        height : Int32,
        max_width : Int32,
        max_height : Int32,
      ) : Tuple(Int32, Int32)
        scale = {
          1.0,
          max_width.to_f64 / width,
          max_height.to_f64 / height,
        }.min
        {
          Math.max(1, (width * scale).to_i),
          Math.max(1, (height * scale).to_i),
        }
      end

      private def encode_png(pixbuf : GdkPixbuf::Pixbuf) : Bytes
        stream = Gio::MemoryOutputStream.new_resizable
        pixbuf.save_to_streamv(stream, "png", nil, nil, nil)
        stream.close(nil)
        data = stream.steal_as_bytes.data ||
               raise Error.new("Image encoder returned no data.")
        data.dup
      end

      private def copy_pixels(pixbuf : GdkPixbuf::Pixbuf) : Pixels
        format = if pixbuf.has_alpha
                   Gdk::MemoryFormat::R8g8b8a8
                 else
                   Gdk::MemoryFormat::R8g8b8
                 end
        source = pixbuf.pixels
        data = Bytes.new(pixbuf.rowstride * pixbuf.height, 0_u8)
        data[0, source.size].copy_from(source)
        Pixels.new(
          pixbuf.width,
          pixbuf.height,
          pixbuf.rowstride.to_u64,
          format,
          data
        )
      end
    end
  end
end
