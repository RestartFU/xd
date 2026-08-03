module Xd
  module UI
    # Reads curl's progress meter out of an installer's error stream.
    #
    # The installer is a shell script, so the only thing it can report through
    # is its output. curl draws its meter on stderr, redrawing one line with
    # carriage returns; the same stream also carries whatever went wrong. This
    # separates the two: the percentage for the button, the rest kept for the
    # failure message, so a meter never ends up quoted back as an error.
    module DownloadProgress
      extend self

      # A redraw of the meter: hash marks, spaces, and the percentage curl
      # prints at its right edge. Anything else on the stream is a message.
      BAR = /\A[#\s]*\d{1,3}(?:\.\d+)?%\s*\z/
      # curl draws these while it is still connecting, before it knows a size.
      SETUP   = /\A[#=\-O\s]*\z/
      PERCENT = /(\d{1,3})(?:\.\d+)?%/

      record Reading, percent : Int32?, text : String

      # The newest percentage in this chunk, if it has one, and the chunk with
      # the meter taken out. Whole redraws only: a chunk can end mid-redraw,
      # and half a bar is not a message.
      def read(chunk : String) : Reading
        percent : Int32? = nil
        text = String::Builder.new
        chunk.split('\r').each do |piece|
          if piece.matches?(BAR)
            if match = PERCENT.match(piece)
              value = match[1].to_i?
              percent = value.clamp(0, 100) if value
            end
          elsif !piece.matches?(SETUP)
            text << piece
          end
        end
        Reading.new(percent, text.to_s)
      end
    end
  end
end
