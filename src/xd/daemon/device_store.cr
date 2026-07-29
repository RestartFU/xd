module Xd
  module Daemon
    class DeviceStoreError < Exception
    end

    abstract class DeviceStore
      abstract def add_device(token_hash : String, name : String) : Nil
      abstract def device_name(token_hash : String) : String?
    end

    # Test and bootstrap store. SQLiteStore replaces this in the executable;
    # both implement the same daemon-owned interface.
    class MemoryDeviceStore < DeviceStore
      getter devices = {} of String => String

      def add_device(token_hash : String, name : String) : Nil
        @devices[token_hash] = name
      end

      def device_name(token_hash : String) : String?
        @devices[token_hash]?
      end
    end
  end
end
