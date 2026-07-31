module Xd
  # Git for Windows may print MSYS paths while Crystal and GTK use native
  # drive paths. Keep conversion in one place so repository panes, inline
  # patches, and worktree selection cannot drift.
  module GitPath
    extend self

    WINDOWS = {{ flag?(:win32) }}

    def native(path : String, windows : Bool = WINDOWS) : String
      return path unless windows
      return path unless msys_drive_path?(path)

      drive = path.byte_at(1).chr.upcase
      "#{drive}:#{path.byte_slice(2)}"
    end

    def environment(path : String, windows : Bool = WINDOWS) : String
      windows ? path.gsub('\\', '/') : path
    end

    private def msys_drive_path?(path : String) : Bool
      return false if path.bytesize < 3
      return false unless path.byte_at(0) == '/'.ord &&
                          path.byte_at(2) == '/'.ord

      drive = path.byte_at(1)
      drive.in?('a'.ord..'z'.ord) || drive.in?('A'.ord..'Z'.ord)
    end
  end
end
