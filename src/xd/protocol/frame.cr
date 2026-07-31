module Xd
  module Protocol
    AUTH_FRAME_LIMIT = 64 * 1024
    FRAME_LIMIT      = 96 * 1024 * 1024

    class FrameTooLarge < IO::Error
    end

    def self.read_frame(input : IO, limit : Int = FRAME_LIMIT) : String?
      line = input.gets('\n', limit + 1, chomp: true)
      return unless line
      if line.bytesize > limit
        raise FrameTooLarge.new("Protocol frame exceeds #{limit} bytes.")
      end
      line
    end
  end
end
